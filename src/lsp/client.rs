//! Transporte JSON-RPC y servidor LSP vivo: arranque del proceso, framing
//! `Content-Length`, hilo lector que reenvía mensajes por un canal, y las
//! peticiones/notificaciones. Extraído de `lsp`.
//!
//! A diferencia de MCP (petición→respuesta), los diagnósticos LSP llegan como
//! NOTIFICACIONES asíncronas y a destiempo (el server indexa primero). Por eso
//! cada server tiene un HILO LECTOR que reenvía cada mensaje a un canal, y la
//! lógica drena ese canal con tiempos de espera (sin bloquear para siempre).

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use serde_json::Value;

use super::{
    INIT_TIMEOUT, REFERENCES_BACKOFF, REFERENCES_RETRIES, REFERENCES_TIMEOUT, ServerCmd,
    path_to_uri, resolve_server_cmd, uri_matches,
};

/// Un language server vivo, cacheado por lenguaje durante la sesión.
pub(super) struct LspServer {
    /// El padre lo necesita para matar el proceso en `shutdown`.
    pub(super) child: Child,
    stdin: ChildStdin,
    /// Mensajes (respuestas y notificaciones) que el hilo lector va entregando.
    rx: Receiver<Value>,
    next_id: u64,
    /// URIs ya abiertas con `didOpen` (para usar `didChange` la próxima vez).
    opened: HashSet<String>,
    version: i64,
}

impl LspServer {
    pub(super) fn start(root: &Path, key: &str) -> Result<Self> {
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
    pub(super) fn request(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
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
    pub(super) fn sync_doc(&mut self, uri: &str, lang_id: &str, text: &str) -> Result<()> {
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
    pub(super) fn request_until(
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
    pub(super) fn notify(&mut self, method: &str, params: Value) -> Result<()> {
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
    pub(super) fn collect_diagnostics(&mut self, uri: &str, overall: Duration, settle: Duration) -> Vec<Value> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
