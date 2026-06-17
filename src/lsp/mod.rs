//! Cliente LSP: diagnósticos + navegación (referencias, rename) sobre stdio con
//! framing Content-Length.
//!
//! dpx arranca un language server por lenguaje (rust-analyzer, etc.), hace el
//! handshake `initialize`/`initialized`, abre el archivo (`didOpen`) y consulta:
//! diagnósticos REALES (`publishDiagnostics`), referencias (`textDocument/
//! references`) y renombrado (`textDocument/rename`). Es grounding de calidad de
//! compilador SIN compilar el proyecto entero.
//!
//! Estructura: este módulo expone la API pública y la detección de lenguaje/URI;
//! [`client`] tiene el transporte JSON-RPC y el servidor vivo; [`workspace`]
//! traduce las respuestas LSP en ediciones/ubicaciones concretas.
//!
//! Los servers se cachean por lenguaje durante la sesión: el primer diagnóstico
//! paga el indexado; los siguientes reusan el server ya caliente.

mod client;
mod workspace;

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;
use serde_json::Value;

use client::LspServer;
use workspace::{apply_workspace_edit, format_locations, locate_symbol, workspace_edit_has_changes};

/// Tope para el handshake `initialize`: indexar un proyecto grande tarda.
const INIT_TIMEOUT: Duration = Duration::from_secs(45);
/// Tope total esperando diagnósticos de un archivo.
const DIAG_OVERALL: Duration = Duration::from_secs(45);
/// Tras el primer lote de diagnósticos, cuánto se espera por más antes de cerrar.
const DIAG_SETTLE: Duration = Duration::from_millis(1500);

/// Tope de cada petición `textDocument/references` o `rename`.
const REFERENCES_TIMEOUT: Duration = Duration::from_secs(20);
/// Reintentos: en frío rust-analyzer tarda en indexar el workspace (corre
/// `cargo metadata`, arranca el proc-macro server…) y devuelve vacío/error hasta
/// que termina. Se reintenta con pausa: ~40s de margen en el peor caso (la
/// primera consulta de la sesión); ya caliente responde al primer intento.
const REFERENCES_RETRIES: usize = 20;
const REFERENCES_BACKOFF: Duration = Duration::from_millis(2000);

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

// ── Detección de lenguaje / comando de server / URI ─────────────────

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
    fn uri_matches_ignora_caso_de_unidad_en_windows() {
        // rust-analyzer emite la unidad en minúscula; debemos matchear igual.
        let mine = "file:///C:/Users/x/main.rs";
        let theirs = "file:///c:/Users/x/main.rs";
        assert_eq!(uri_matches(mine, theirs), cfg!(windows));
        // Rutas distintas no matchean en ninguna plataforma.
        assert!(!uri_matches(mine, "file:///c:/Users/x/otro.rs"));
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
