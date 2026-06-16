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
    srv.sync_doc(&uri, lang_id, &text)?;
    let diags = srv.collect_diagnostics(&uri, DIAG_OVERALL, DIAG_SETTLE);
    Ok(format_diagnostics(rel_path, &diags))
}

/// Tope de cada petición `textDocument/references`.
const REFERENCES_TIMEOUT: Duration = Duration::from_secs(20);
/// Reintentos: en frío rust-analyzer tarda en indexar el workspace (corre
/// `cargo metadata`, arranca el proc-macro server…) y devuelve vacío hasta que
/// termina. Se reintenta con pausa: ~18s de margen total en el peor caso (la
/// primera consulta de la sesión); ya caliente responde al primer intento.
const REFERENCES_RETRIES: usize = 20;
const REFERENCES_BACKOFF: Duration = Duration::from_millis(2000);

/// Punto de entrada de la tool `find_references`: localiza el símbolo `symbol`
/// en la línea `line` (1-based, tal como la ve el modelo) de `rel_path` y pide
/// al language server TODAS sus referencias en el proyecto. Es ground truth del
/// compilador (no texto): a diferencia de un grep, no trae falsos positivos ni
/// se pierde usos calificados. Devuelve las ubicaciones `ruta:línea:col`, o un
/// mensaje claro si no hay server, no se encontró el símbolo, o no hay refs.
pub fn references(cwd: &Path, rel_path: &str, line: usize, symbol: &str) -> Result<String> {
    let abs = cwd.join(rel_path);
    if !abs.is_file() {
        return Err(anyhow!("no existe el archivo `{rel_path}`"));
    }
    let key = server_key(&abs).ok_or_else(|| {
        anyhow!("no hay language server para `{rel_path}` (soportados: .rs, .ts/.tsx, .js/.jsx, .py, .go)")
    })?;
    let lang_id = language_id(&abs).unwrap_or(key);
    let uri = path_to_uri(&abs);
    let text = std::fs::read_to_string(&abs).with_context(|| format!("no pude leer {rel_path}"))?;

    // Resuelve la posición exacta (línea, carácter) del símbolo dentro de la
    // línea indicada — el modelo razona en nombres, no en offsets UTF-16.
    let (line0, char0) = locate_symbol(&text, line, symbol)
        .ok_or_else(|| anyhow!("no encontré `{symbol}` en la línea {line} de `{rel_path}`"))?;

    let mut map = manager().lock().map_err(|e| anyhow!("lock LSP: {e}"))?;
    if !map.contains_key(key) {
        let srv = LspServer::start(cwd, key)?;
        map.insert(key.to_string(), srv);
    }
    let srv = map.get_mut(key).expect("recién insertado");
    srv.sync_doc(&uri, lang_id, &text)?;

    let params = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": line0, "character": char0 },
        "context": { "includeDeclaration": true },
    });
    // Reintenta mientras el server indexa (vacío o error de "not indexed").
    let (result, _err) = srv.request_until("textDocument/references", params, |r| {
        r.as_array().is_some_and(|a| !a.is_empty())
    });
    let locations = result.as_array().cloned().unwrap_or_default();
    Ok(format_locations(cwd, rel_path, symbol, &locations))
}

/// Punto de entrada de la tool `rename_symbol`: localiza el símbolo y pide al
/// language server (`textDocument/rename`) TODOS los cambios para renombrarlo en
/// el proyecto, ya resueltos como reescrituras de archivo completas
/// (`FileWrite`) listas para pasar por el flujo normal de confirmación/diff/
/// checkpoint. Es un refactor EXACTO (lo calcula el compilador), no un
/// find-and-replace textual. Devuelve `(writes, notas)`: las notas avisan de
/// operaciones que dpx no aplica solo (p.ej. renombrar el archivo de un módulo).
pub fn rename(
    cwd: &Path,
    rel_path: &str,
    line: usize,
    symbol: &str,
    new_name: &str,
) -> Result<(Vec<crate::fs::FileWrite>, Vec<String>)> {
    let abs = cwd.join(rel_path);
    if !abs.is_file() {
        return Err(anyhow!("no existe el archivo `{rel_path}`"));
    }
    let key = server_key(&abs).ok_or_else(|| {
        anyhow!("no hay language server para `{rel_path}` (soportados: .rs, .ts/.tsx, .js/.jsx, .py, .go)")
    })?;
    let lang_id = language_id(&abs).unwrap_or(key);
    let uri = path_to_uri(&abs);
    let text = std::fs::read_to_string(&abs).with_context(|| format!("no pude leer {rel_path}"))?;
    let (line0, char0) = locate_symbol(&text, line, symbol)
        .ok_or_else(|| anyhow!("no encontré `{symbol}` en la línea {line} de `{rel_path}`"))?;

    let mut map = manager().lock().map_err(|e| anyhow!("lock LSP: {e}"))?;
    if !map.contains_key(key) {
        let srv = LspServer::start(cwd, key)?;
        map.insert(key.to_string(), srv);
    }
    let srv = map.get_mut(key).expect("recién insertado");
    srv.sync_doc(&uri, lang_id, &text)?;

    let params = serde_json::json!({
        "textDocument": { "uri": uri },
        "position": { "line": line0, "character": char0 },
        "newName": new_name,
    });
    // Mismo cold-start que references: en frío rust-analyzer responde con error
    // ("No references found at position") hasta indexar → se reintenta.
    let (edit, err) = srv.request_until("textDocument/rename", params, workspace_edit_has_changes);
    if !workspace_edit_has_changes(&edit)
        && let Some(e) = err
    {
        return Err(anyhow!("textDocument/rename: {e}"));
    }
    apply_workspace_edit(cwd, &edit)
}

/// ¿El `WorkspaceEdit` trae cambios reales (en cualquiera de los dos formatos)?
fn workspace_edit_has_changes(edit: &Value) -> bool {
    edit.get("documentChanges").and_then(Value::as_array).is_some_and(|a| !a.is_empty())
        || edit.get("changes").and_then(Value::as_object).is_some_and(|m| !m.is_empty())
}

/// Traduce un `WorkspaceEdit` LSP a reescrituras de archivo completas. Soporta
/// los dos formatos (`documentChanges` —preferido por rust-analyzer— y `changes`)
/// y junta notas para las operaciones de recurso (renombrar/crear/borrar archivo)
/// que NO aplicamos automáticamente.
fn apply_workspace_edit(
    cwd: &Path,
    edit: &Value,
) -> Result<(Vec<crate::fs::FileWrite>, Vec<String>)> {
    let mut writes = Vec::new();
    let mut notes = Vec::new();
    if let Some(dcs) = edit.get("documentChanges").and_then(Value::as_array) {
        for dc in dcs {
            // Las operaciones de recurso traen `kind` (rename/create/delete).
            if let Some(kind) = dc.get("kind").and_then(Value::as_str) {
                notes.push(format!(
                    "[rename_symbol: el language server sugiere además una operación de archivo \
                     `{kind}` que dpx no aplica sola; hazla a mano si hace falta]"
                ));
                continue;
            }
            if let (Some(uri), Some(edits)) =
                (dc["textDocument"]["uri"].as_str(), dc["edits"].as_array())
            {
                writes.push(build_renamed_write(cwd, uri, edits)?);
            }
        }
    } else if let Some(map) = edit.get("changes").and_then(Value::as_object) {
        for (uri, edits) in map {
            if let Some(edits) = edits.as_array() {
                writes.push(build_renamed_write(cwd, uri, edits)?);
            }
        }
    }
    Ok((writes, notes))
}

/// Lee un archivo, aplica sus `TextEdit`s y devuelve la reescritura completa.
fn build_renamed_write(cwd: &Path, uri: &str, edits: &[Value]) -> Result<crate::fs::FileWrite> {
    let rel = uri_to_rel(cwd, uri).ok_or_else(|| anyhow!("URI no relativizable: {uri}"))?;
    let content = std::fs::read_to_string(cwd.join(&rel))
        .with_context(|| format!("no pude leer {rel} para el rename"))?;
    let new_content = apply_text_edits(&content, edits)?;
    Ok(crate::fs::FileWrite { path: rel, content: new_content })
}

/// Aplica una lista de `TextEdit` LSP a `content`. Convierte cada posición
/// (línea, carácter UTF-16) a offset de bytes y aplica los splices de ATRÁS
/// hacia DELANTE, para que los offsets aún sin tocar no se corran.
fn apply_text_edits(content: &str, edits: &[Value]) -> Result<String> {
    let pos = |e: &Value, which: &str| -> Option<(u32, u32)> {
        let l = e["range"][which]["line"].as_u64()? as u32;
        let c = e["range"][which]["character"].as_u64()? as u32;
        Some((l, c))
    };
    let mut spans: Vec<(usize, usize, String)> = Vec::with_capacity(edits.len());
    for e in edits {
        let (sl, sc) = pos(e, "start").ok_or_else(|| anyhow!("TextEdit sin range.start"))?;
        let (el, ec) = pos(e, "end").ok_or_else(|| anyhow!("TextEdit sin range.end"))?;
        let start = position_to_byte(content, sl, sc)
            .ok_or_else(|| anyhow!("posición start ({sl},{sc}) fuera de rango"))?;
        let end = position_to_byte(content, el, ec)
            .ok_or_else(|| anyhow!("posición end ({el},{ec}) fuera de rango"))?;
        let new_text = e["newText"].as_str().unwrap_or("").to_string();
        spans.push((start, end, new_text));
    }
    spans.sort_by_key(|s| std::cmp::Reverse(s.0)); // descendente por inicio
    let mut out = content.to_string();
    for (start, end, new_text) in spans {
        if start > end || end > out.len() {
            return Err(anyhow!("rango de edición inválido ({start}..{end} en {} bytes)", out.len()));
        }
        out.replace_range(start..end, &new_text);
    }
    Ok(out)
}

/// Convierte una posición LSP `(line0, char_utf16)` (0-based; carácter en
/// unidades UTF-16) a offset de BYTES en `content`. Clampa al fin de línea si el
/// carácter la rebasa, y al fin de archivo si la línea lo hace.
fn position_to_byte(content: &str, line0: u32, char_utf16: u32) -> Option<usize> {
    let mut offset = 0usize;
    let mut cur = 0u32;
    for seg in content.split_inclusive('\n') {
        if cur == line0 {
            let text = seg.strip_suffix('\n').unwrap_or(seg);
            let text = text.strip_suffix('\r').unwrap_or(text);
            let mut u16c = 0u32;
            for (b, ch) in text.char_indices() {
                if u16c >= char_utf16 {
                    return Some(offset + b);
                }
                u16c += ch.len_utf16() as u32;
            }
            return Some(offset + text.len()); // carácter rebasa la línea → fin del texto
        }
        offset += seg.len();
        cur += 1;
    }
    (cur == line0).then_some(content.len()) // línea una más allá de la última = fin del archivo
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

    /// Abre (`didOpen`) o sincroniza (`didChange`) un documento, subiendo la
    /// versión. La primera vez dispara `didOpen`; las siguientes `didChange` con
    /// el texto actual. Compartido por diagnósticos y referencias.
    fn sync_doc(&mut self, uri: &str, lang_id: &str, text: &str) -> Result<()> {
        self.version += 1;
        let version = self.version;
        if self.opened.contains(uri) {
            self.notify(
                "textDocument/didChange",
                serde_json::json!({
                    "textDocument": { "uri": uri, "version": version },
                    "contentChanges": [ { "text": text } ],
                }),
            )
        } else {
            self.notify(
                "textDocument/didOpen",
                serde_json::json!({
                    "textDocument": { "uri": uri, "languageId": lang_id, "version": version, "text": text },
                }),
            )?;
            self.opened.insert(uri.to_string());
            Ok(())
        }
    }

    /// Petición que puede venir VACÍA o ERROR mientras el server indexa en frío
    /// (rust-analyzer devuelve `[]`/`null` o un error tipo "No references found"
    /// hasta cargar el workspace). Reintenta hasta que `ready(&result)` sea true
    /// o se agoten los intentos. Devuelve `(resultado, último_error)`: si nunca
    /// hubo éxito, el resultado es `Null` y el error (si lo hubo) lo decide el
    /// llamador. Ver presupuesto en `REFERENCES_RETRIES`/`REFERENCES_BACKOFF`.
    fn request_until(
        &mut self,
        method: &str,
        params: Value,
        ready: impl Fn(&Value) -> bool,
    ) -> (Value, Option<String>) {
        let mut last_err = None;
        for attempt in 0..REFERENCES_RETRIES {
            match self.request(method, params.clone(), REFERENCES_TIMEOUT) {
                Ok(r) if ready(&r) => return (r, None),
                Ok(_) => last_err = None, // vacío: aún indexando, sin error
                Err(e) => last_err = Some(e.to_string()),
            }
            if attempt + 1 < REFERENCES_RETRIES {
                std::thread::sleep(REFERENCES_BACKOFF);
            }
        }
        (Value::Null, last_err)
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

/// Localiza un símbolo dentro de una línea (1-based) de `text` y devuelve su
/// posición LSP `(line0, character0)` 0-based, donde el carácter se cuenta en
/// unidades UTF-16 (lo que exige el protocolo). `None` si la línea no existe o
/// el símbolo no aparece en ella.
fn locate_symbol(text: &str, line_1based: usize, symbol: &str) -> Option<(u32, u32)> {
    if line_1based == 0 || symbol.is_empty() {
        return None;
    }
    let line = text.lines().nth(line_1based - 1)?;
    let byte_idx = line.find(symbol)?;
    let char0 = line[..byte_idx].encode_utf16().count() as u32;
    Some(((line_1based - 1) as u32, char0))
}

/// `file://` URI → ruta relativa al proyecto (`cwd`), con separadores `/`. Si la
/// URI cae fuera del proyecto, devuelve la ruta absoluta limpia (mejor mostrar
/// algo útil que nada). `None` si no es una `file://` URI.
fn uri_to_rel(cwd: &Path, uri: &str) -> Option<String> {
    let body = uri.strip_prefix("file://")?;
    // file:///C:/x → /C:/x en Windows: quita la barra inicial sobrante.
    let cleaned = if cfg!(windows) {
        body.strip_prefix('/').unwrap_or(body)
    } else {
        body
    };
    let decoded = cleaned.replace("%20", " ");
    let abs = std::path::PathBuf::from(decoded.replace('/', std::path::MAIN_SEPARATOR_STR));
    let rel = abs.strip_prefix(cwd).unwrap_or(&abs);
    Some(rel.to_string_lossy().replace('\\', "/"))
}

/// Formatea las ubicaciones de referencias para el modelo (rutas relativas,
/// líneas/cols 1-based, ordenadas y sin duplicados).
fn format_locations(cwd: &Path, rel_path: &str, symbol: &str, locations: &[Value]) -> String {
    if locations.is_empty() {
        return format!(
            "[find_references: el language server no reportó referencias a `{symbol}` \
             (puede seguir indexando, o el símbolo solo se usa en su declaración). \
             Si esperabas más, reintenta en unos segundos o usa search_project.]"
        );
    }
    let mut lines: Vec<String> = locations
        .iter()
        .map(|loc| {
            let uri = loc["uri"].as_str().unwrap_or("");
            let l = loc["range"]["start"]["line"].as_u64().unwrap_or(0) + 1;
            let c = loc["range"]["start"]["character"].as_u64().unwrap_or(0) + 1;
            let path = uri_to_rel(cwd, uri).unwrap_or_else(|| uri.to_string());
            format!("{path}:{l}:{c}")
        })
        .collect();
    lines.sort();
    lines.dedup();
    format!(
        "{} referencia(s) a `{symbol}` (declarado/usado desde `{rel_path}`):\n{}",
        lines.len(),
        lines.join("\n")
    )
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
    fn locate_symbol_resuelve_posicion_utf16() {
        let text = "fn main() {}\n    let run_turn = 1;\nfin\n";
        // Línea 2 (1-based), símbolo run_turn empieza tras 4 espacios + "let ".
        assert_eq!(locate_symbol(text, 2, "run_turn"), Some((1, 8)));
        // Línea inexistente o símbolo ausente → None.
        assert_eq!(locate_symbol(text, 99, "run_turn"), None);
        assert_eq!(locate_symbol(text, 1, "ausente"), None);
        assert_eq!(locate_symbol(text, 0, "main"), None);
        // Caracteres no-ASCII antes del símbolo cuentan en unidades UTF-16.
        let acc = "let café_x = 0;\n"; // 'é' = 1 unidad UTF-16
        assert_eq!(locate_symbol(acc, 1, "café_x"), Some((0, 4)));
    }

    #[test]
    fn format_locations_vacio_y_con_refs() {
        let cwd = PathBuf::from(if cfg!(windows) { r"C:\proj" } else { "/proj" });
        assert!(format_locations(&cwd, "src/a.rs", "foo", &[]).contains("no reportó referencias"));

        let uri = path_to_uri(&cwd.join("src").join("b.rs"));
        let locs = vec![serde_json::json!({
            "uri": uri,
            "range": { "start": { "line": 41, "character": 7 } }
        })];
        let out = format_locations(&cwd, "src/a.rs", "foo", &locs);
        assert!(out.contains("1 referencia"));
        assert!(out.contains("src/b.rs:42:8"), "0-based→1-based y ruta relativa, vi: {out}");
    }

    #[test]
    fn position_to_byte_offsets_y_clamps() {
        let c = "fn main() {}\n  let x = 1;\nfin\n";
        // Inicio del archivo.
        assert_eq!(position_to_byte(c, 0, 0), Some(0));
        // Línea 1 (0-based), carácter 6 → 'x' tras "  let ": 13 ("fn main() {}\n") + 6 = 19.
        assert_eq!(position_to_byte(c, 1, 6), Some(19));
        // Carácter que rebasa la línea → clampa al fin del texto de esa línea.
        assert_eq!(position_to_byte(c, 1, 999), Some(13 + "  let x = 1;".len()));
        // Línea una más allá de la última → fin del archivo.
        assert_eq!(position_to_byte(c, 99, 0), None);
        // Carácter en unidades UTF-16 con no-ASCII antes.
        let u = "let café = 1;\n"; // 'é' = 1 unidad UTF-16, 2 bytes
        // char 5 = tras "let c","a","f","é" → byte 4(let )+1+1+1+2 = 9? "let " =4, c=1,a=1,f=1,é=2 → 'é' es el char index 3 en utf16 (l,e,t,space=0..3? no)
        // "let café": l(0)e(1)t(2) (3)c(4)a(5)f(6)é(7). char_utf16=7 → 'é'. bytes: 'l e t space c a f' = 7 bytes, é empieza en byte 7.
        assert_eq!(position_to_byte(u, 0, 7), Some(7));
        // char 8 = justo después de 'é' (que ocupa 2 bytes) → byte 9.
        assert_eq!(position_to_byte(u, 0, 8), Some(9));
    }

    #[test]
    fn apply_text_edits_renombra_consistente() {
        let content = "let foo = 1;\nbar(foo, foo);\n";
        // Tres ocurrencias de `foo` → `baz`, en orden cualquiera (la fn ordena).
        let edits = vec![
            serde_json::json!({ "range": { "start": {"line":0,"character":4}, "end": {"line":0,"character":7} }, "newText": "baz" }),
            serde_json::json!({ "range": { "start": {"line":1,"character":4}, "end": {"line":1,"character":7} }, "newText": "baz" }),
            serde_json::json!({ "range": { "start": {"line":1,"character":9}, "end": {"line":1,"character":12} }, "newText": "baz" }),
        ];
        let out = apply_text_edits(content, &edits).unwrap();
        assert_eq!(out, "let baz = 1;\nbar(baz, baz);\n");
    }

    #[test]
    fn apply_workspace_edit_documentchanges_y_notas() {
        let cwd = std::env::temp_dir().join(format!("dpx-ws-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&cwd);
        std::fs::write(cwd.join("a.rs"), "let foo = 1;\n").unwrap();
        let uri = path_to_uri(&cwd.join("a.rs"));
        let edit = serde_json::json!({
            "documentChanges": [
                { "textDocument": { "uri": uri, "version": 1 },
                  "edits": [ { "range": { "start": {"line":0,"character":4}, "end": {"line":0,"character":7} }, "newText": "baz" } ] },
                { "kind": "rename", "oldUri": "x", "newUri": "y" }
            ]
        });
        let (writes, notes) = apply_workspace_edit(&cwd, &edit).unwrap();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].path, "a.rs");
        assert_eq!(writes[0].content, "let baz = 1;\n");
        assert_eq!(notes.len(), 1, "la op de recurso `rename` debe generar una nota");
        let _ = std::fs::remove_dir_all(&cwd);
    }

    #[test]
    fn uri_to_rel_relativiza_dentro_del_proyecto() {
        let cwd = PathBuf::from(if cfg!(windows) { r"C:\proj" } else { "/proj" });
        let uri = path_to_uri(&cwd.join("src").join("main.rs"));
        assert_eq!(uri_to_rel(&cwd, &uri).as_deref(), Some("src/main.rs"));
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

    #[test]
    #[ignore = "requiere rust-analyzer instalado en el PATH"]
    fn references_encuentra_usos_reales() {
        // Proyecto cargo con un símbolo declarado y usado dos veces.
        let dir = std::env::temp_dir().join(format!("dpx-lspref-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join("src"));
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("src").join("main.rs"),
            "fn saludar() {}\nfn main() {\n    saludar();\n    saludar();\n}\n",
        )
        .unwrap();

        // Símbolo `saludar` declarado en la línea 1.
        let out = match references(&dir, "src/main.rs", 1, "saludar") {
            Ok(o) => o,
            Err(e) => {
                shutdown();
                panic!("references falló: {e:#}");
            }
        };
        shutdown();
        assert!(
            out.contains("referencia(s) a `saludar`"),
            "esperaba referencias reales, salió: {out}"
        );
        // La declaración + 2 usos = al menos las líneas 3 y 4 deben aparecer.
        assert!(out.contains("main.rs:3:") && out.contains("main.rs:4:"), "salió: {out}");
    }

    #[test]
    #[ignore = "requiere rust-analyzer instalado en el PATH"]
    fn rename_renombra_todas_las_apariciones() {
        let dir = std::env::temp_dir().join(format!("dpx-lsprn-{}", std::process::id()));
        let _ = std::fs::create_dir_all(dir.join("src"));
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"t\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("src").join("main.rs"),
            "fn saludar() {}\nfn main() {\n    saludar();\n    saludar();\n}\n",
        )
        .unwrap();

        let (writes, _notes) = match rename(&dir, "src/main.rs", 1, "saludar", "hola") {
            Ok(r) => r,
            Err(e) => {
                shutdown();
                panic!("rename falló: {e:#}");
            }
        };
        shutdown();
        assert_eq!(writes.len(), 1, "esperaba reescribir un archivo, vi: {}", writes.len());
        let new = &writes[0].content;
        assert!(!new.contains("saludar"), "quedó una aparición vieja: {new}");
        assert_eq!(new.matches("hola").count(), 3, "decl + 2 usos → 3 `hola`: {new}");
    }
}
