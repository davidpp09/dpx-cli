//! Cliente LSP minimalista (diagnósticos), sobre stdio con framing Content-Length.
//!
//! dpx arranca un language server por lenguaje (rust-analyzer, etc.), hace el
//! handshake `initialize`/`initialized`, abre el archivo (`didOpen`) y recoge los
//! diagnósticos REALES (errores/warnings con línea y columna) que el server emite
//! por la notificación `textDocument/publishDiagnostics`. Es grounding de calidad
//! de compilador SIN tener que compilar el proyecto entero.
//!
//! A diferencia de MCP (petición→respuesta), los diagnósticos LSP llegan como
//! NOTIFICACIONES asíncronas y a destiempo (el server indexa primero). Por eso
//! cada server tiene un HILO LECTOR que reenvía cada mensaje a un canal, y la
//! lógica drena ese canal con tiempos de espera (sin bloquear para siempre).
//!
//! Los servers se cachean por lenguaje durante la sesión: el primer diagnóstico
//! paga el indexado; los siguientes reusan el server ya caliente.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::Value;

/// Tope para el handshake `initialize`: indexar un proyecto grande tarda.
const INIT_TIMEOUT: Duration = Duration::from_secs(45);
/// Tope total esperando diagnósticos de un archivo.
const DIAG_OVERALL: Duration = Duration::from_secs(45);
/// Tras el primer lote de diagnósticos, cuánto se espera por más antes de cerrar.
const DIAG_SETTLE: Duration = Duration::from_millis(1500);

// ── Configuración opcional `.dpx/lsp.toml` ──────────────────────────

/// Override de comando por lenguaje, p.ej. `[servers.rust] command = "..."`.
#[derive(Debug, Clone, Deserialize)]
struct ServerCmd {
    command: String,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct LspConfig {
    #[serde(default)]
    servers: HashMap<String, ServerCmd>,
}

// ── Manager global ──────────────────────────────────────────────────

static MANAGER: OnceLock<Mutex<HashMap<String, LspServer>>> = OnceLock::new();

fn manager() -> &'static Mutex<HashMap<String, LspServer>> {
    MANAGER.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Punto de entrada de la tool `lsp_diagnostics`: abre `rel_path` en el language
/// server correspondiente y devuelve sus diagnósticos formateados (o un mensaje
/// claro si no hay server para ese lenguaje, no está instalado, o no hubo nada).
pub fn diagnostics(cwd: &Path, rel_path: &str) -> Result<String> {
    let abs = cwd.join(rel_path);
    if !abs.is_file() {
        return Err(anyhow!("no existe el archivo `{rel_path}`"));
    }
    let key = server_key(&abs).ok_or_else(|| {
        anyhow!(
            "no hay language server para `{rel_path}` (soportados: .rs, .ts/.tsx, .js/.jsx, .py, .go)"
        )
    })?;
    let lang_id = language_id(&abs).unwrap_or(key);
    let uri = path_to_uri(&abs);
    let text = std::fs::read_to_string(&abs).with_context(|| format!("no pude leer {rel_path}"))?;

    let mut map = manager().lock().map_err(|e| anyhow!("lock LSP: {e}"))?;
    if !map.contains_key(key) {
        let srv = LspServer::start(cwd, key)?;
        map.insert(key.to_string(), srv);
    }
    let srv = map.get_mut(key).expect("recién insertado");

    // didChange si ya estaba abierto, didOpen si es la primera vez (cada uno
    // dispara una nueva tanda de diagnósticos para esa URI).
    let version = {
        srv.version += 1;
        srv.version
    };
    if srv.opened.contains(&uri) {
        srv.notify(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [ { "text": text } ],
            }),
        )?;
    } else {
        srv.notify(
            "textDocument/didOpen",
            serde_json::json!({
                "textDocument": { "uri": uri, "languageId": lang_id, "version": version, "text": text },
            }),
        )?;
        srv.opened.insert(uri.clone());
    }

    let diags = srv.collect_diagnostics(&uri, DIAG_OVERALL, DIAG_SETTLE);
    Ok(format_diagnostics(rel_path, &diags))
}

/// Apaga los language servers (handshake `shutdown`/`exit` + kill). Se llama al
/// terminar dpx para no dejar procesos pesados (rust-analyzer) huérfanos.
pub fn shutdown() {
    let Some(m) = MANAGER.get() else { return };
    let Ok(mut map) = m.lock() else { return };
    for srv in map.values_mut() {
        let _ = srv.request("shutdown", Value::Null, Duration::from_secs(2));
        let _ = srv.notify("exit", Value::Null);
        let _ = srv.child.kill();
    }
    map.clear();
}

// ── Servidor LSP vivo ───────────────────────────────────────────────

struct LspServer {
    child: Child,
    stdin: ChildStdin,
    /// Mensajes (respuestas y notificaciones) que el hilo lector va entregando.
    rx: Receiver<Value>,
    next_id: u64,
    /// URIs ya abiertas con `didOpen` (para usar `didChange` la próxima vez).
    opened: HashSet<String>,
    version: i64,
}

impl LspServer {
    fn start(root: &Path, key: &str) -> Result<Self> {
        let cmd_cfg = resolve_server_cmd(root, key).ok_or_else(|| {
            anyhow!("no sé qué language server usar para `{key}`; añádelo en .dpx/lsp.toml")
        })?;

        let mut child = spawn_child(&cmd_cfg).with_context(|| {
            format!("no pude arrancar `{}` (¿está instalado y en el PATH?)", cmd_cfg.command)
        })?;
        let stdin = child.stdin.take().ok_or_else(|| anyhow!("stdin LSP no disponible"))?;
        let stdout = child.stdout.take().ok_or_else(|| anyhow!("stdout LSP no disponible"))?;
        let rx = spawn_reader(stdout);

        let mut srv = LspServer {
            child,
            stdin,
            rx,
            next_id: 0,
            opened: HashSet::new(),
            version: 0,
        };

        srv.request(
            "initialize",
            serde_json::json!({
                "processId": std::process::id(),
                "rootUri": path_to_uri(root),
                "capabilities": {
                    "textDocument": {
                        "publishDiagnostics": { "relatedInformation": false },
                        "synchronization": { "dynamicRegistration": false }
                    }
                },
                "clientInfo": { "name": "dpx", "version": env!("CARGO_PKG_VERSION") }
            }),
            INIT_TIMEOUT,
        )
        .context("handshake initialize del language server falló")?;
        srv.notify("initialized", serde_json::json!({}))?;
        Ok(srv)
    }

    /// Petición JSON-RPC: escribe y drena el canal hasta la respuesta a SU id,
    /// ignorando notificaciones intermedias.
    fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        self.next_id += 1;
        let id = self.next_id;
        write_framed(
            &mut self.stdin,
            &serde_json::json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
        )?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .ok_or_else(|| anyhow!("timeout esperando respuesta LSP a `{method}`"))?;
            match self.rx.recv_timeout(remaining) {
                Ok(msg) => {
                    if msg.get("id").and_then(Value::as_u64) == Some(id) {
                        if let Some(err) = msg.get("error") {
                            return Err(anyhow!(
                                "error LSP en `{method}`: {}",
                                err["message"].as_str().unwrap_or("?")
                            ));
                        }
                        return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
                    }
                    // otra notificación o respuesta: se ignora aquí
                }
                Err(RecvTimeoutError::Timeout) => {
                    return Err(anyhow!("timeout LSP en `{method}`"));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(anyhow!("el language server cerró la conexión"));
                }
            }
        }
    }

    /// Notificación JSON-RPC (sin id, sin respuesta).
    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        write_framed(
            &mut self.stdin,
            &serde_json::json!({ "jsonrpc": "2.0", "method": method, "params": params }),
        )
    }

    /// Drena el canal recogiendo los `publishDiagnostics` de `uri`. Devuelve el
    /// ÚLTIMO lote recibido (el server reemite la lista completa cada vez). Para
    /// tras un periodo de calma (`settle`) después del primer lote, o al agotar
    /// `overall` (p.ej. el archivo no produce diagnósticos).
    // El loop calcula dos timeouts (con sus propios `break`) ANTES de leer del
    // canal, así que no es un `while let` simple como sugiere clippy.
    #[allow(clippy::while_let_loop)]
    fn collect_diagnostics(&mut self, uri: &str, overall: Duration, settle: Duration) -> Vec<Value> {
        let mut latest: Option<Vec<Value>> = None;
        let overall_deadline = Instant::now() + overall;
        // Solo un `publishDiagnostics` de NUESTRA uri arranca/reinicia el reloj de
        // calma; el server emite `$/progress` y otras notificaciones sin parar
        // mientras indexa, y si esas resetearan el reloj nunca cerraríamos.
        let mut settle_deadline: Option<Instant> = None;
        loop {
            let overall_left = match overall_deadline.checked_duration_since(Instant::now()) {
                Some(d) => d,
                None => break, // tope total alcanzado
            };
            let wait = match settle_deadline {
                Some(sd) => match sd.checked_duration_since(Instant::now()) {
                    Some(d) => d.min(overall_left),
                    None => break, // periodo de calma cumplido tras el último lote
                },
                None => overall_left, // aún sin diagnósticos: esperamos el primero
            };
            match self.rx.recv_timeout(wait) {
                Ok(msg) => {
                    if msg.get("method").and_then(Value::as_str)
                        == Some("textDocument/publishDiagnostics")
                        && msg["params"]["uri"].as_str().is_some_and(|u| uri_matches(u, uri))
                    {
                        latest = Some(
                            msg["params"]["diagnostics"].as_array().cloned().unwrap_or_default(),
                        );
                        settle_deadline = Some(Instant::now() + settle);
                    }
                    // otras notificaciones (progreso, logs) no tocan el reloj
                }
                Err(_) => break, // timeout (calma o tope) o desconexión
            }
        }
        latest.unwrap_or_default()
    }
}

/// Arranca el proceso del language server con stdio en pipes. Intenta primero
/// el binario directo (rust-analyzer, gopls = `.exe`, stdio limpio) y, si no se
/// encuentra en Windows, reintenta vía `cmd /C` (los servers de npm —
/// typescript-language-server, pyright— son shims `.cmd` que `Command::new` no
/// resuelve directamente). NO se envuelve siempre en `cmd /C`: hacerlo rompía
/// el stdio de rust-analyzer.
fn spawn_child(cfg: &ServerCmd) -> std::io::Result<Child> {
    let direct = Command::new(&cfg.command)
        .args(&cfg.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn();
    match direct {
        Err(e) if cfg!(windows) && e.kind() == std::io::ErrorKind::NotFound => Command::new("cmd")
            .arg("/C")
            .arg(&cfg.command)
            .args(&cfg.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn(),
        other => other,
    }
}

// ── Hilo lector + framing Content-Length ────────────────────────────

/// Arranca un hilo que lee mensajes framed de `stdout` y los reenvía por un
/// canal. Termina solo al cerrarse stdout (EOF) o al caer el receptor.
fn spawn_reader(stdout: ChildStdout) -> Receiver<Value> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut r = BufReader::new(stdout);
        // Termina al cerrarse stdout (read_framed → Err) o al caer el receptor.
        while let Ok(v) = read_framed(&mut r) {
            if tx.send(v).is_err() {
                break;
            }
        }
    });
    rx
}

/// Lee UN mensaje con framing `Content-Length` (el estándar LSP): cabeceras
/// hasta una línea en blanco, luego exactamente `Content-Length` bytes de cuerpo.
fn read_framed<R: BufRead>(r: &mut R) -> Result<Value> {
    let mut content_len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Err(anyhow!("el language server cerró stdout"));
        }
        let t = line.trim_end();
        if t.is_empty() {
            let len = content_len.ok_or_else(|| anyhow!("mensaje LSP sin Content-Length"))?;
            let mut buf = vec![0u8; len];
            r.read_exact(&mut buf)?;
            return serde_json::from_slice(&buf).context("cuerpo LSP inválido");
        }
        if let Some(rest) = strip_ci_prefix(t, "content-length:") {
            content_len = rest.trim().parse().ok();
        }
        // otras cabeceras (Content-Type) se ignoran
    }
}

/// Escribe un mensaje con framing `Content-Length`.
fn write_framed(stdin: &mut ChildStdin, msg: &Value) -> Result<()> {
    let body = serde_json::to_string(msg)?;
    write!(stdin, "Content-Length: {}\r\n\r\n{body}", body.len())?;
    stdin.flush()?;
    Ok(())
}

/// `strip_prefix` insensible a mayúsculas.
fn strip_ci_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

// ── Helpers de lenguaje / URI / formato ─────────────────────────────

/// Qué server (clave de config) usar según la extensión del archivo.
fn server_key(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => "rust",
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => "typescript",
        "py" => "python",
        "go" => "go",
        _ => return None,
    })
}

/// `languageId` LSP exacto (distingue tsx/jsx para tsserver).
fn language_id(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "rs" => "rust",
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "py" => "python",
        "go" => "go",
        _ => return None,
    })
}

/// Comando del server: override de `.dpx/lsp.toml` si existe, si no el default.
fn resolve_server_cmd(root: &Path, key: &str) -> Option<ServerCmd> {
    if let Some(cfg) = load_config(root)
        && let Some(c) = cfg.servers.get(key)
    {
        return Some(c.clone());
    }
    default_server(key)
}

fn load_config(root: &Path) -> Option<LspConfig> {
    let path = root.join(".dpx").join("lsp.toml");
    let data = std::fs::read_to_string(path).ok()?;
    toml::from_str(&data).ok()
}

/// Comandos por defecto de los language servers más comunes.
fn default_server(key: &str) -> Option<ServerCmd> {
    let (command, args): (&str, &[&str]) = match key {
        "rust" => ("rust-analyzer", &[]),
        "typescript" => ("typescript-language-server", &["--stdio"]),
        "python" => ("pyright-langserver", &["--stdio"]),
        "go" => ("gopls", &[]),
        _ => return None,
    };
    Some(ServerCmd {
        command: command.to_string(),
        args: args.iter().map(|s| s.to_string()).collect(),
    })
}

/// ¿Dos `file://` URIs apuntan al mismo archivo? rust-analyzer (y otros) emiten
/// la letra de unidad en minúscula (`file:///c:/…`) y normalizan la ruta; en
/// Windows el sistema de archivos es case-insensitive, así que comparamos sin
/// distinguir mayúsculas. En Unix la comparación es exacta.
fn uri_matches(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// Ruta absoluta → `file://` URI. En Windows: `C:\x` → `file:///C:/x`.
fn path_to_uri(abs: &Path) -> String {
    let s = abs.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

/// Formatea los diagnósticos para devolvérselos al modelo (líneas/cols 1-based).
fn format_diagnostics(rel_path: &str, diags: &[Value]) -> String {
    if diags.is_empty() {
        return format!(
            "Sin diagnósticos del language server para `{rel_path}` (no hay errores ni warnings)."
        );
    }
    let mut out = format!(
        "Diagnósticos del language server para `{rel_path}` ({}):\n",
        diags.len()
    );
    for d in diags {
        let sev = match d["severity"].as_u64() {
            Some(1) => "error",
            Some(2) => "warning",
            Some(3) => "info",
            Some(4) => "hint",
            _ => "diag",
        };
        let line = d["range"]["start"]["line"].as_u64().unwrap_or(0) + 1;
        let col = d["range"]["start"]["character"].as_u64().unwrap_or(0) + 1;
        let msg = d["message"].as_str().unwrap_or("").trim();
        let code = d["code"]
            .as_str()
            .map(|c| format!(" [{c}]"))
            .or_else(|| d["code"].as_u64().map(|c| format!(" [{c}]")))
            .unwrap_or_default();
        out.push_str(&format!("  {sev} {line}:{col}{code} — {msg}\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn server_key_por_extension() {
        assert_eq!(server_key(&PathBuf::from("a.rs")), Some("rust"));
        assert_eq!(server_key(&PathBuf::from("a.tsx")), Some("typescript"));
        assert_eq!(server_key(&PathBuf::from("a.jsx")), Some("typescript"));
        assert_eq!(server_key(&PathBuf::from("a.py")), Some("python"));
        assert_eq!(server_key(&PathBuf::from("a.txt")), None);
    }

    #[test]
    fn language_id_distingue_react() {
        assert_eq!(language_id(&PathBuf::from("a.tsx")), Some("typescriptreact"));
        assert_eq!(language_id(&PathBuf::from("a.jsx")), Some("javascriptreact"));
        assert_eq!(language_id(&PathBuf::from("a.ts")), Some("typescript"));
        assert_eq!(language_id(&PathBuf::from("a.rs")), Some("rust"));
    }

    #[test]
    fn default_server_conocidos_y_desconocidos() {
        assert_eq!(default_server("rust").unwrap().command, "rust-analyzer");
        assert!(default_server("typescript").unwrap().args.contains(&"--stdio".to_string()));
        assert!(default_server("cobol").is_none());
    }

    #[test]
    fn path_to_uri_windows_y_unix() {
        // Estilo Windows con backslashes y letra de unidad.
        let uri = path_to_uri(&PathBuf::from(r"C:\Users\x\main.rs"));
        assert_eq!(uri, "file:///C:/Users/x/main.rs");
        // Estilo Unix absoluto (file:// + autoridad vacía + /ruta = tres slashes).
        let uri = path_to_uri(&PathBuf::from("/home/x/main.rs"));
        assert_eq!(uri, "file:///home/x/main.rs");
    }

    #[test]
    fn format_diagnostics_vacio_y_con_errores() {
        assert!(format_diagnostics("a.rs", &[]).contains("Sin diagnósticos"));

        let diags = vec![serde_json::json!({
            "severity": 1,
            "range": { "start": { "line": 9, "character": 4 } },
            "message": "cannot find value `x`",
            "code": "E0425"
        })];
        let out = format_diagnostics("src/a.rs", &diags);
        assert!(out.contains("error 10:5")); // 0-based → 1-based
        assert!(out.contains("[E0425]"));
        assert!(out.contains("cannot find value"));
    }

    #[test]
    fn uri_matches_ignora_caso_de_unidad_en_windows() {
        // rust-analyzer emite la unidad en minúscula; debemos matchear igual.
        let mine = "file:///C:/Users/x/main.rs";
        let theirs = "file:///c:/Users/x/main.rs";
        assert_eq!(uri_matches(mine, theirs), cfg!(windows));
        // Rutas distintas no matchean en ninguna plataforma.
        assert!(!uri_matches(mine, "file:///c:/Users/x/otro.rs"));
    }

    #[test]
    fn read_framed_lee_content_length() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        let framed = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let mut r = BufReader::new(framed.as_bytes());
        let msg = read_framed(&mut r).unwrap();
        assert_eq!(msg["id"].as_u64(), Some(1));
        assert_eq!(msg["result"]["ok"], serde_json::json!(true));
    }

    #[test]
    fn read_framed_eof_es_error() {
        let empty: &[u8] = b"";
        let mut r = BufReader::new(empty);
        assert!(read_framed(&mut r).is_err());
    }

    #[test]
    #[ignore = "requiere rust-analyzer instalado en el PATH"]
    fn diagnostics_detecta_un_error_real() {
        // Proyecto cargo temporal con un error de compilación deliberado.
        let dir = std::env::temp_dir().join(format!("dpx-lsp-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join("src"));
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        // `x` no existe → rust-analyzer debe reportar un error.
        std::fs::write(dir.join("src").join("main.rs"), "fn main() { let _ = x; }\n").unwrap();

        let out = match diagnostics(&dir, "src/main.rs") {
            Ok(o) => o,
            Err(e) => {
                shutdown();
                panic!("diagnostics falló: {e:#}");
            }
        };
        shutdown();
        // Debe traer diagnósticos REALES (no el mensaje de "sin diagnósticos", que
        // contiene la subcadena "errores" y daría un falso positivo).
        assert!(
            !out.contains("Sin diagnósticos"),
            "rust-analyzer no devolvió diagnósticos del archivo: {out}"
        );
        assert!(
            out.contains("error "),
            "esperaba al menos un error de rust-analyzer, salió: {out}"
        );
    }
}
