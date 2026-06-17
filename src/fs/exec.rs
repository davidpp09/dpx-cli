//! Ejecución de comandos del proyecto: streaming línea a línea, timeout,
//! cancelación por Ctrl-C, kill del árbol de procesos y captura del código de
//! salida REAL. Extraído de `fs`.

use std::path::Path;

use super::cap_tail;

/// Tiempo máximo de ejecución de un comando (`dpx:run` y búsquedas). Un proceso
/// que no termina (servidor, watch) se corta y se le explica al modelo.
pub const RUN_TIMEOUT_SECS: u64 = 180;

/// Resultado de ejecutar un comando.
pub struct RunResult {
    pub output: String,
    /// True si el usuario lo interrumpió (Ctrl-C): el turno debería abortarse.
    pub cancelled: bool,
    /// Código de salida REAL del proceso (`Some(0)` = éxito determinista). `None`
    /// si ni siquiera arrancó. Es la fuente de verdad para el green-gate: el texto
    /// `exit code: N` puede perderse si `cap_tail` recorta una salida muy larga.
    pub exit_code: Option<i32>,
}

enum StreamLine {
    Out(String),
    Err(String),
}

/// Bombea un pipe del hijo al canal, línea a línea y tolerante a no-UTF-8
/// (la salida de Maven/Gradle en Windows trae acentos en la codepage local).
fn pump_lines<R: std::io::Read + Send + 'static>(
    reader: R,
    tx: std::sync::mpsc::Sender<StreamLine>,
    wrap: fn(String) -> StreamLine,
) {
    use std::io::BufRead;
    std::thread::spawn(move || {
        let mut reader = std::io::BufReader::new(reader);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match reader.read_until(b'\n', &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let line = String::from_utf8_lossy(&buf)
                        .trim_end_matches(['\r', '\n'])
                        .to_string();
                    if tx.send(wrap(line)).is_err() {
                        break;
                    }
                }
            }
        }
    });
}

/// Mata el proceso y todo su árbol. En Windows hace falta `taskkill /T`: matar
/// solo el `cmd` dejaría vivo al build/servidor que lanzó.
fn kill_tree(child: &mut std::process::Child) {
    if cfg!(windows) {
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .output();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Ejecuta un comando en la raíz del proyecto entregando cada línea de salida
/// por `on_line` según llega. Se corta por `timeout_secs` o cuando
/// `should_cancel` devuelve true (Ctrl-C), matando el árbol de procesos.
/// El stdin va a null: un comando que espere entrada no congela el REPL.
pub fn run_command_streaming(
    cwd: &Path,
    cmd: &str,
    timeout_secs: u64,
    on_line: &mut dyn FnMut(&str),
    should_cancel: &dyn Fn() -> bool,
) -> RunResult {
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let (shell, flag) = if cfg!(windows) { ("cmd", "/C") } else { ("sh", "-c") };
    let spawned = Command::new(shell)
        .args([flag, cmd])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(c) => c,
        Err(e) => {
            return RunResult {
                output: format!("error al ejecutar el comando: {e}"),
                cancelled: false,
                exit_code: None,
            };
        }
    };

    let (tx, rx) = mpsc::channel();
    if let Some(out) = child.stdout.take() {
        pump_lines(out, tx.clone(), StreamLine::Out);
    }
    if let Some(err) = child.stderr.take() {
        pump_lines(err, tx.clone(), StreamLine::Err);
    }
    drop(tx);

    let start = Instant::now();
    let mut stdout_buf = String::new();
    let mut stderr_buf = String::new();
    let (mut cancelled, mut timed_out) = (false, false);

    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                let (text, buf) = match line {
                    StreamLine::Out(t) => (t, &mut stdout_buf),
                    StreamLine::Err(t) => (t, &mut stderr_buf),
                };
                on_line(&text);
                buf.push_str(&text);
                buf.push('\n');
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // Ambos pipes cerrados: el proceso terminó.
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if should_cancel() {
            kill_tree(&mut child);
            cancelled = true;
            break;
        }
        if start.elapsed().as_secs() >= timeout_secs {
            kill_tree(&mut child);
            timed_out = true;
            break;
        }
    }

    // Lo que quedara encolado tras cortar.
    while let Ok(line) = rx.try_recv() {
        match line {
            StreamLine::Out(t) => {
                stdout_buf.push_str(&t);
                stdout_buf.push('\n');
            }
            StreamLine::Err(t) => {
                stderr_buf.push_str(&t);
                stderr_buf.push('\n');
            }
        }
    }

    let code = child.wait().ok().and_then(|s| s.code()).unwrap_or(-1);
    let mut s = format!("exit code: {code}\n");
    if !stdout_buf.trim().is_empty() {
        s.push_str(&format!("--- stdout ---\n{}\n", stdout_buf.trim_end()));
    }
    if !stderr_buf.trim().is_empty() {
        s.push_str(&format!("--- stderr ---\n{}\n", stderr_buf.trim_end()));
    }
    if timed_out {
        s.push_str(&format!(
            "[TIMEOUT: el comando superó {timeout_secs}s y fue terminado. Si es un proceso de larga \
             duración (servidor, watch), NO lo ejecutes con dpx:run: pídele al usuario que lo corra \
             en su propia terminal.]\n"
        ));
    }
    if cancelled {
        s.push_str("[interrumpido por el usuario con Ctrl-C]\n");
    }
    RunResult { output: cap_tail(&s, 200), cancelled, exit_code: Some(code) }
}

/// Ejecuta un comando y devuelve su salida acotada (sin streaming ni cancelación;
/// para tareas internas rápidas como las búsquedas).
pub fn run_command(cwd: &Path, cmd: &str) -> String {
    run_command_streaming(cwd, cmd, RUN_TIMEOUT_SECS, &mut |_| {}, &|| false).output
}
