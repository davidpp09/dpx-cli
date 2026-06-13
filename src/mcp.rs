//! Cliente MCP (Model Context Protocol) — minimalista, sobre stdio.
//!
//! dpx actúa como cliente MCP: arranca servidores externos (procesos), hace el
//! handshake JSON-RPC 2.0, obtiene sus tools, y las fusiona con las nativas
//! para que el LLM las vea como propias.
//!
//! Transporte: stdin/stdout con framing Content-Length (el estándar MCP).
//! JSON-RPC 2.0 sobre ese transporte, sin dependencias externas.
//!
//! Namespacing: las tools MCP se anuncian como `mcp__<server>__<tool>`
//! para que no colisionen con las nativas de dpx.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Tope de mensajes a saltar (notificaciones, logs del server) mientras se
/// espera la respuesta a una petición concreta, antes de rendirse.
const MAX_SKIP: usize = 64;

/// Tope de tiempo para arrancar un server y completar su handshake. Si se
/// excede, dpx descarta ese server y sigue: un server colgado NUNCA debe
/// congelar el arranque. Generoso porque `npx`/`uvx` pueden descargar el
/// paquete la primera vez.
const SPAWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

use anyhow::{Context, Result, anyhow};
use serde::Deserialize;

// ── Configuración ──────────────────────────────────────────────────

/// Una entrada en `[[servers]]` de `.dpx/mcp.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
struct McpConfig {
    #[serde(default)]
    servers: Vec<ServerConfig>,
}

// ── Cache de una tool ──────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct CachedTool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

// ── Servidor MCP vivo ──────────────────────────────────────────────

/// Pipes del server protegidos con `Mutex` para acceso interior-mutable
/// desde `&McpManager` (que viene de `OnceLock` y no puede moverse).
struct Server {
    name: String,
    #[allow(dead_code)]
    child: Mutex<Child>,
    stdin: Mutex<std::process::ChildStdin>,
    stdout: Mutex<BufReader<std::process::ChildStdout>>,
    tools: Vec<CachedTool>,
}

// ── Manager global ─────────────────────────────────────────────────

static MANAGER: OnceLock<McpManager> = OnceLock::new();

pub struct McpManager {
    servers: Vec<Server>,
}

impl McpManager {
    /// Inicializa el manager: lee `.dpx/mcp.toml`, arranca los servidores,
    /// hace el handshake y cachea sus tools. Si falla, dpx sigue sin MCP.
    pub fn init(cwd: &Path) {
        let _ = Self::try_init(cwd).inspect_err(|e| {
            eprintln!("[dpx] MCP: {e}");
        });
    }

    fn try_init(cwd: &Path) -> Result<()> {
        let cfg = Self::load_config(cwd)?;
        if cfg.servers.is_empty() {
            return Ok(());
        }
        let mut servers = Vec::new();
        for cfg in &cfg.servers {
            match Server::spawn_with_timeout(cfg, SPAWN_TIMEOUT) {
                Ok(s) => {
                    eprintln!("[dpx] MCP server `{}` listo — {} tools", s.name, s.tools.len());
                    servers.push(s);
                }
                Err(e) => {
                    eprintln!("[dpx] MCP server `{}` falló: {e}. dpx sigue sin él.", cfg.name);
                }
            }
        }
        let _ = MANAGER.set(McpManager { servers });
        Ok(())
    }

    fn load_config(cwd: &Path) -> Result<McpConfig> {
        let path = cwd.join(".dpx").join("mcp.toml");
        if !path.exists() {
            return Ok(McpConfig { servers: Vec::new() });
        }
        let data = fs::read_to_string(&path)
            .with_context(|| format!("No se pudo leer {}", path.display()))?;
        toml::from_str(&data)
            .with_context(|| format!("No se pudo parsear {}", path.display()))
    }

    /// Todas las tools cacheadas de todos los servidores.
    pub fn cached_tools() -> Vec<CachedTool> {
        let Some(mgr) = MANAGER.get() else { return Vec::new() };
        mgr.servers.iter().flat_map(|s| s.tools.clone()).collect()
    }

    /// Llama a una tool MCP: `mcp__<server>__<tool>` con `args`.
    /// Devuelve `content[0].text` del resultado JSON-RPC.
    pub fn call_tool(full_name: &str, args: &serde_json::Value) -> Result<String> {
        let (server_name, tool_name) = split_tool_name(full_name)
            .ok_or_else(|| anyhow!("formato de tool MCP inválido: {full_name}"))?;

        let Some(mgr) = MANAGER.get() else {
            return Err(anyhow!("MCP no inicializado"))?;
        };
        let server = mgr
            .servers
            .iter()
            .find(|s| s.name == server_name)
            .ok_or_else(|| anyhow!("servidor MCP `{server_name}` no encontrado"))?;

        let params = serde_json::json!({
            "name": tool_name,
            "arguments": args,
        });

        let resp = send_request(&server.stdin, &server.stdout, "tools/call", params)?;

        resp["content"]
            .as_array()
            .and_then(|items| items.first())
            .and_then(|item| item["text"].as_str())
            .map(str::to_string)
            .ok_or_else(|| anyhow!("respuesta MCP sin content[0].text: {resp}"))
    }

    /// Apaga todos los servidores (envía la notificación `exit`).
    #[allow(dead_code)]
    pub fn shutdown() {
        let Some(mgr) = MANAGER.get() else { return };
        for s in &mgr.servers {
            // `exit` es una notificación: no espera respuesta.
            let _ = send_notification(&s.stdin, "notifications/exit", serde_json::json!({}));
        }
    }
}

impl Server {
    /// Arranca el server y completa el handshake con un tope de tiempo. El
    /// trabajo corre en un hilo aparte: si no termina dentro de `timeout`, se
    /// reporta error y dpx sigue sin ese server (en vez de quedarse colgado
    /// esperando un `initialize`/`tools/list` que nunca llega).
    fn spawn_with_timeout(cfg: &ServerConfig, timeout: std::time::Duration) -> Result<Self> {
        let cfg = cfg.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            // El receptor pudo rendirse por timeout; ignoramos el error de envío.
            let _ = tx.send(Server::spawn(&cfg));
        });
        match rx.recv_timeout(timeout) {
            Ok(result) => result,
            Err(_) => Err(anyhow!(
                "timeout ({}s) arrancando/handshaking el server; no respondió a tiempo",
                timeout.as_secs()
            )),
        }
    }

    fn spawn(cfg: &ServerConfig) -> Result<Self> {
        // En Windows, los servidores MCP suelen lanzarse vía npx/uvx/npm, que
        // son shims `.cmd` que `Command::new` NO resuelve directamente (falla con
        // "programa no encontrado"). Envolverlos en `cmd /C` los busca en el PATH
        // como lo haría una shell. En Unix se ejecuta el binario tal cual.
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.arg("/C").arg(&cfg.command).args(&cfg.args);
            c
        } else {
            let mut c = Command::new(&cfg.command);
            c.args(&cfg.args);
            c
        };
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        // El stderr del server (logs, trazas) se descarta para no corromper el
        // REPL; sus errores de protocolo igual se detectan al leer stdout.
        cmd.stderr(Stdio::null());
        for (k, v) in &cfg.env {
            cmd.env(k, resolve_env_var(v));
        }

        let mut child = cmd
            .spawn()
            .with_context(|| format!("No se pudo arrancar `{}`", cfg.command))?;

        let stdin = child.stdin.take().ok_or_else(|| anyhow!("stdin no disponible"))?;
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("stdout no disponible"))?);

        // Movemos los pipes dentro de Mutex ANTES del handshake.
        let stdin = Mutex::new(stdin);
        let stdout = Mutex::new(stdout);

        // ── Handshake MCP ───────────────────────────────────────────
        send_request(
            &stdin,
            &stdout,
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": { "tools": {} },
                "clientInfo": {
                    "name": "dpx",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }),
        )
        .context("handshake initialize falló")?;

        // `initialized` es una NOTIFICACIÓN: no recibe respuesta. Mandarla por la
        // vía de petición (y quedarse leyendo) colgaría el handshake.
        send_notification(&stdin, "notifications/initialized", serde_json::json!({}))?;

        let tools_resp = send_request(&stdin, &stdout, "tools/list", serde_json::json!({}))
            .context("tools/list falló")?;

        let tools: Vec<CachedTool> = tools_resp["tools"]
            .as_array()
            .ok_or_else(|| anyhow!("tools/list no devolvió array: {tools_resp}"))?
            .iter()
            .map(|t| {
                let name = format!(
                    "mcp__{}__{}",
                    cfg.name,
                    t["name"].as_str().unwrap_or("?")
                );
                let desc = t["description"]
                    .as_str()
                    .unwrap_or("tool MCP sin descripción");
                let schema = t.get("inputSchema").cloned().unwrap_or(serde_json::json!({
                    "type": "object",
                    "properties": {}
                }));
                CachedTool { name, description: desc.to_string(), input_schema: schema }
            })
            .collect();

        Ok(Server {
            name: cfg.name.clone(),
            child: Mutex::new(child),
            stdin,
            stdout,
            tools,
        })
    }
}

// ── JSON-RPC 2.0 sobre stdio (newline-delimited) ────────────────────
//
// El transporte stdio de MCP delimita los mensajes por salto de línea: cada
// mensaje es UNA línea de JSON sin saltos embebidos. El lector tolera además el
// framing `Content-Length` (estilo LSP) por si algún servidor lo emite.

/// Contador global de ids JSON-RPC. Las peticiones se atienden de forma
/// secuencial (un `Mutex` por server), así que basta un id monótono.
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Envía una petición JSON-RPC 2.0 y espera la respuesta a SU id, saltando
/// notificaciones o mensajes intermedios que el servidor pueda emitir (p.ej.
/// `notifications/message` de logging). `stdin`/`stdout` son `Mutex` porque se
/// comparten entre `spawn` (handshake) y `call_tool`.
fn send_request(
    stdin: &Mutex<std::process::ChildStdin>,
    stdout: &Mutex<BufReader<std::process::ChildStdout>>,
    method: &str,
    params: serde_json::Value,
) -> Result<serde_json::Value> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    write_line(
        stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        }),
    )?;

    let mut r = stdout.lock().map_err(|e| anyhow!("stdout lock: {e}"))?;
    for _ in 0..MAX_SKIP {
        let msg = read_message(&mut *r)?;
        // Solo nos interesa la respuesta a NUESTRO id; lo demás (notificaciones
        // sin id, o respuestas a otras peticiones) se ignora.
        if msg.get("id").and_then(serde_json::Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(err) = msg.get("error") {
            return Err(anyhow!(
                "error MCP: {} (código {})",
                err["message"].as_str().unwrap_or("?"),
                err["code"].as_i64().unwrap_or(-1),
            ));
        }
        return Ok(msg["result"].clone());
    }
    Err(anyhow!(
        "no llegó respuesta MCP a `{method}` (id {id}) tras {MAX_SKIP} mensajes"
    ))
}

/// Envía una notificación JSON-RPC 2.0 (sin `id`). Una notificación NO recibe
/// respuesta: no se debe leer tras enviarla o el cliente se quedaría colgado.
fn send_notification(
    stdin: &Mutex<std::process::ChildStdin>,
    method: &str,
    params: serde_json::Value,
) -> Result<()> {
    write_line(
        stdin,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        }),
    )
}

/// Serializa un mensaje y lo escribe como una línea (`\n` al final).
fn write_line(stdin: &Mutex<std::process::ChildStdin>, msg: &serde_json::Value) -> Result<()> {
    let body = serde_json::to_string(msg)?;
    let mut w = stdin.lock().map_err(|e| anyhow!("stdin lock: {e}"))?;
    writeln!(w, "{body}")?;
    w.flush()?;
    Ok(())
}

/// Lee un mensaje JSON-RPC. Acepta el caso normal (una línea = un mensaje) y,
/// por tolerancia, el framing `Content-Length` (LSP). Salta líneas en blanco.
fn read_message<R: BufRead>(r: &mut R) -> Result<serde_json::Value> {
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Err(anyhow!("el servidor MCP cerró stdout"));
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue; // separadores entre mensajes
        }

        // Tolerancia a framing Content-Length: leemos cabeceras hasta la línea
        // en blanco y luego exactamente `len` bytes de cuerpo.
        if let Some(rest) = strip_ci_prefix(trimmed, "content-length:") {
            let len: usize = rest
                .trim()
                .parse()
                .map_err(|_| anyhow!("Content-Length inválido: '{trimmed}'"))?;
            loop {
                let mut hdr = String::new();
                if r.read_line(&mut hdr)? == 0 {
                    return Err(anyhow!("el servidor MCP cerró stdout en cabeceras"));
                }
                if hdr.trim().is_empty() {
                    break;
                }
            }
            let mut buf = vec![0u8; len];
            r.read_exact(&mut buf)?;
            return serde_json::from_slice(&buf).context("cuerpo JSON-RPC inválido");
        }

        // Caso normal: la línea ES el mensaje JSON.
        return serde_json::from_str(trimmed).context("línea JSON-RPC inválida");
    }
}

/// `strip_prefix` insensible a mayúsculas, devolviendo el resto tras el prefijo.
fn strip_ci_prefix<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn split_tool_name(full: &str) -> Option<(&str, &str)> {
    let rest = full.strip_prefix("mcp__")?;
    let (server, tool) = rest.split_once("__")?;
    if server.is_empty() || tool.is_empty() {
        return None;
    }
    Some((server, tool))
}

fn resolve_env_var(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' && chars.peek() == Some(&'{') {
            chars.next();
            let mut var = String::new();
            while let Some(&c2) = chars.peek() {
                if c2 == '}' {
                    chars.next();
                    break;
                }
                var.push(chars.next().unwrap());
            }
            result.push_str(&std::env::var(&var).unwrap_or_default());
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_tool_name_valido() {
        assert_eq!(
            split_tool_name("mcp__gh__create_issue"),
            Some(("gh", "create_issue"))
        );
    }

    #[test]
    fn split_tool_name_sin_mcp_prefix() {
        assert_eq!(split_tool_name("read_file"), None);
    }

    #[test]
    fn split_tool_name_vacio() {
        assert_eq!(split_tool_name("mcp____tool"), None);
    }

    #[test]
    fn resolve_env_var_substituye() {
        unsafe { std::env::set_var("DPX_TEST_VAR", "hola"); }
        assert_eq!(resolve_env_var("${DPX_TEST_VAR}"), "hola");
        assert_eq!(resolve_env_var("token-${DPX_TEST_VAR}-fin"), "token-hola-fin");
    }

    #[test]
    fn resolve_env_var_sin_var() {
        assert_eq!(resolve_env_var("sin_variable"), "sin_variable");
    }

    #[test]
    fn strip_ci_prefix_insensible_a_mayusculas() {
        assert_eq!(
            strip_ci_prefix("Content-Length: 12", "content-length:"),
            Some(" 12")
        );
        assert_eq!(
            strip_ci_prefix("CONTENT-LENGTH:5", "content-length:"),
            Some("5")
        );
        assert_eq!(strip_ci_prefix("otra cosa", "content-length:"), None);
    }

    #[test]
    fn read_message_newline_delimited() {
        use std::io::BufReader;
        // Línea en blanco inicial + un mensaje JSON por línea.
        let data = b"\n{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n";
        let mut r = BufReader::new(&data[..]);
        let msg = read_message(&mut r).unwrap();
        assert_eq!(msg["id"].as_u64(), Some(1));
        assert_eq!(msg["result"]["ok"], serde_json::json!(true));
    }

    #[test]
    fn read_message_tolera_content_length() {
        use std::io::BufReader;
        let body = r#"{"jsonrpc":"2.0","id":2,"result":42}"#;
        let framed = format!("Content-Length: {}\r\n\r\n{body}", body.len());
        let mut r = BufReader::new(framed.as_bytes());
        let msg = read_message(&mut r).unwrap();
        assert_eq!(msg["result"].as_i64(), Some(42));
    }

    #[test]
    fn read_message_stdout_cerrado_es_error() {
        use std::io::BufReader;
        let empty: &[u8] = b"";
        let mut r = BufReader::new(empty);
        assert!(read_message(&mut r).is_err());
    }
}
