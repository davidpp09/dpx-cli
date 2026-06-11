//! Escritura de archivos al proyecto, con seguridad y bajo confirmación.
//!
//! El mentor propone archivos en bloques con la forma:
//!
//! ````text
//! ```dpx:write path=src/main/java/com/app/User.java
//! <contenido del archivo>
//! ```
//! ````
//!
//! Este módulo extrae esas propuestas del texto de la respuesta. El REPL muestra
//! un preview y pide confirmación antes de escribir nada. Nunca se escribe fuera
//! del directorio del proyecto.

pub mod safety;

use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, anyhow};

/// Una propuesta de archivo a escribir.
pub struct FileWrite {
    pub path: String,
    pub content: String,
}

impl FileWrite {
    /// Número de líneas del contenido propuesto.
    pub fn line_count(&self) -> usize {
        self.content.lines().count()
    }
}

/// Extrae el marcador `dpx:write ... path=<ruta>` de una línea, si lo tiene.
pub fn parse_path_marker(s: &str) -> Option<String> {
    let rest = s.trim().strip_prefix("dpx:write")?;
    rest.split_whitespace()
        .find_map(|tok| tok.strip_prefix("path="))
        .map(|p| p.trim_matches('"').to_string())
        .filter(|p| !p.is_empty())
}

/// Extrae el marcador `dpx:read ... path=<ruta>` de una línea, si lo tiene.
pub fn parse_read_marker(s: &str) -> Option<String> {
    let rest = s.trim().strip_prefix("dpx:read")?;
    rest.split_whitespace()
        .find_map(|tok| tok.strip_prefix("path="))
        .map(|p| p.trim_matches('"').to_string())
        .filter(|p| !p.is_empty())
}

/// Extrae las peticiones de lectura `dpx:read path=...` de una respuesta.
/// Acepta el marcador en el fence o como primera línea del bloque.
pub fn parse_reads(text: &str) -> Vec<String> {
    let mut reads = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(info) = line.trim_start().strip_prefix("```") else {
            continue;
        };
        let mut path = parse_read_marker(info);
        if path.is_none() {
            if let Some(first) = lines.peek() {
                if let Some(p) = parse_read_marker(first) {
                    path = Some(p);
                    lines.next();
                }
            }
        }
        if let Some(p) = path {
            reads.push(p);
            for body in lines.by_ref() {
                if body.trim_start().starts_with("```") {
                    break;
                }
            }
        }
    }
    reads
}

/// ¿La info-string de un fence es una petición de ejecución `dpx:run`?
fn is_run_fence(s: &str) -> bool {
    let t = s.trim();
    t == "dpx:run" || t.starts_with("dpx:run ")
}

/// Extrae las peticiones de ejecución `dpx:run` (el contenido del bloque es el comando).
pub fn parse_runs(text: &str) -> Vec<String> {
    let mut runs = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(info) = line.trim_start().strip_prefix("```") else {
            continue;
        };
        let mut skip_marker = false;
        let is_run = if is_run_fence(info) {
            true
        } else if lines.peek().is_some_and(|n| is_run_fence(n)) {
            skip_marker = true;
            true
        } else {
            false
        };
        if !is_run {
            continue;
        }
        if skip_marker {
            lines.next();
        }
        let mut cmd = String::new();
        for body in lines.by_ref() {
            if body.trim_start().starts_with("```") {
                break;
            }
            if !cmd.is_empty() {
                cmd.push('\n');
            }
            cmd.push_str(body);
        }
        let cmd = cmd.trim().to_string();
        if !cmd.is_empty() {
            runs.push(cmd);
        }
    }
    runs
}

/// ¿La info-string de un fence es un plan `dpx:plan`?
fn is_plan_fence(s: &str) -> bool {
    let t = s.trim();
    t == "dpx:plan" || t.starts_with("dpx:plan ")
}

/// Extrae el plan de un bloque `dpx:plan` (una tarea por línea, `[ ]` pendiente /
/// `[x]` hecha). Devuelve `(hecha, texto)` por ítem, o `None` si no hay plan.
pub fn parse_plan(text: &str) -> Option<Vec<(bool, String)>> {
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(info) = line.trim_start().strip_prefix("```") else {
            continue;
        };
        let mut is_plan = is_plan_fence(info);
        if !is_plan && lines.peek().is_some_and(|n| is_plan_fence(n)) {
            is_plan = true;
            lines.next();
        }
        if !is_plan {
            continue;
        }
        let mut items = Vec::new();
        for body in lines.by_ref() {
            if body.trim_start().starts_with("```") {
                break;
            }
            let t = body.trim().trim_start_matches(['-', '*', ' ']);
            if t.is_empty() {
                continue;
            }
            if let Some(rest) = t.strip_prefix("[x]").or_else(|| t.strip_prefix("[X]")) {
                items.push((true, rest.trim().to_string()));
            } else if let Some(rest) = t.strip_prefix("[ ]").or_else(|| t.strip_prefix("[]")) {
                items.push((false, rest.trim().to_string()));
            } else {
                items.push((false, t.to_string()));
            }
        }
        if !items.is_empty() {
            return Some(items);
        }
    }
    None
}

/// Tiempo máximo de ejecución de un comando (`dpx:run` y búsquedas). Un proceso
/// que no termina (servidor, watch) se corta y se le explica al modelo.
pub const RUN_TIMEOUT_SECS: u64 = 180;

/// Resultado de ejecutar un comando.
pub struct RunResult {
    pub output: String,
    /// True si el usuario lo interrumpió (Ctrl-C): el turno debería abortarse.
    pub cancelled: bool,
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
    RunResult { output: cap_tail(&s, 200), cancelled }
}

/// Ejecuta un comando y devuelve su salida acotada (sin streaming ni cancelación;
/// para tareas internas rápidas como las búsquedas).
pub fn run_command(cwd: &Path, cmd: &str) -> String {
    run_command_streaming(cwd, cmd, RUN_TIMEOUT_SECS, &mut |_| {}, &|| false).output
}

/// Conserva solo las últimas `max_lines` líneas (los errores de build suelen estar al final).
fn cap_tail(s: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() > max_lines {
        format!(
            "…[salida truncada, últimas {max_lines} líneas]\n{}",
            lines[lines.len() - max_lines..].join("\n")
        )
    } else {
        s.to_string()
    }
}

/// Detecta el comando de build del proyecto (Maven, Gradle o Cargo) para la
/// verificación automática tras escribir código. `None` si no hay build
/// reconocible.
pub fn detect_build(cwd: &Path) -> Option<String> {
    if cwd.join("pom.xml").exists() {
        // Prefiere el Maven Wrapper del proyecto (no requiere Maven global instalado).
        let wrapper = if cfg!(windows) { "mvnw.cmd" } else { "mvnw" };
        if cwd.join(wrapper).exists() {
            let invoke = if cfg!(windows) { "mvnw.cmd" } else { "./mvnw" };
            return Some(format!("{invoke} -q -DskipTests compile"));
        }
        return Some("mvn -q -DskipTests compile".to_string());
    }
    if cwd.join("build.gradle").exists() || cwd.join("build.gradle.kts").exists() {
        let wrapper = if cfg!(windows) { "gradlew.bat" } else { "gradlew" };
        if cwd.join(wrapper).exists() {
            let invoke = if cfg!(windows) { "gradlew.bat" } else { "./gradlew" };
            return Some(format!("{invoke} compileJava -q"));
        }
        return Some("gradle compileJava -q".to_string());
    }
    if cwd.join("Cargo.toml").exists() {
        return Some("cargo check --quiet".to_string());
    }
    None
}

/// Detecta el stack del proyecto mirando solo los archivos de la raíz.
/// `None` si no se reconoce ninguno (mentor genérico, sin skills de dominio).
pub fn detect_stack(cwd: &Path) -> Option<&'static str> {
    if cwd.join("pom.xml").exists() || cwd.join("mvnw").exists() || cwd.join("mvnw.cmd").exists() {
        return Some("spring-boot");
    }
    if cwd.join("package.json").exists() {
        return Some(if package_json_has_react(cwd) { "react" } else { "node" });
    }
    if cwd.join("Cargo.toml").exists() {
        return Some("rust");
    }
    if cwd.join("build.gradle").exists() || cwd.join("build.gradle.kts").exists() {
        return Some("gradle");
    }
    if cwd.join("requirements.txt").exists() || cwd.join("pyproject.toml").exists() {
        return Some("python");
    }
    None
}

/// ¿El `package.json` declara `react` entre sus dependencias?
fn package_json_has_react(cwd: &Path) -> bool {
    let Ok(data) = fs::read_to_string(cwd.join("package.json")) else {
        return false;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) else {
        return false;
    };
    ["dependencies", "devDependencies"]
        .iter()
        .any(|key| json.get(*key).and_then(|deps| deps.get("react")).is_some())
}

/// El manifiesto de build (`pom.xml` o `build.gradle[.kts]`) acotado, para
/// fundamentar al modelo en las dependencias y versiones REALES del proyecto.
pub fn build_manifest(cwd: &Path) -> Option<(String, String)> {
    const MAX_LINES: usize = 160;
    for name in ["pom.xml", "build.gradle", "build.gradle.kts"] {
        let path = cwd.join(name);
        if !path.exists() {
            continue;
        }
        if let Ok(data) = fs::read_to_string(&path) {
            let total = data.lines().count();
            let content = if total > MAX_LINES {
                let head: String = data.lines().take(MAX_LINES).collect::<Vec<_>>().join("\n");
                format!("{head}\n…[truncado: {total} líneas en total]")
            } else {
                data
            };
            return Some((name.to_string(), content));
        }
    }
    None
}

/// ¿La ruta es código fuente compilable o el manifiesto de build?
fn is_build_source(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".java")
        || p.ends_with(".kt")
        || p.ends_with(".rs")
        || p.ends_with("pom.xml")
        || p.ends_with("build.gradle")
        || p.ends_with("build.gradle.kts")
        || p.ends_with("cargo.toml")
}

/// ¿Alguna de las escrituras toca código fuente o el build (para disparar la
/// verificación automática de compilación)?
pub fn touches_build(writes: &[FileWrite]) -> bool {
    writes.iter().any(|w| is_build_source(&w.path))
}

/// Igual que [`touches_build`] pero para ediciones quirúrgicas.
pub fn edits_touch_build(edits: &[FileEdit]) -> bool {
    edits.iter().any(|e| is_build_source(&e.path))
}

/// Quita del texto los bloques de acción (`dpx:read` / `dpx:write` / `dpx:run`) para
/// poder renderizar solo la prosa como Markdown. Los bloques normales se conservan.
pub fn strip_action_blocks(text: &str) -> String {
    let mut out = String::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        if let Some(info) = line.trim_start().strip_prefix("```") {
            let on_fence = parse_path_marker(info).is_some()
                || parse_read_marker(info).is_some()
                || parse_edit_marker(info).is_some()
                || parse_search_marker(info).is_some()
                || parse_delete_marker(info).is_some()
                || is_run_fence(info)
                || is_plan_fence(info);
            let on_next = lines.peek().is_some_and(|n| {
                parse_path_marker(n).is_some()
                    || parse_read_marker(n).is_some()
                    || parse_edit_marker(n).is_some()
                    || parse_search_marker(n).is_some()
                    || parse_delete_marker(n).is_some()
                    || is_run_fence(n)
                    || is_plan_fence(n)
            });
            if on_fence || on_next {
                // Saltar el bloque entero hasta el cierre ```.
                for body in lines.by_ref() {
                    if body.trim_start().starts_with("```") {
                        break;
                    }
                }
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Detecta intentos de acción malformados, para avisar al modelo en vez de
/// ignorarlos en silencio (el fallo silencioso deja la conversación sobre un
/// estado falso): marcadores `dpx:*` fuera de un bloque ```, bloques `dpx:edit`
/// sin el trío SEARCH/`=======`/REPLACE completo, y SEARCH/REPLACE sueltos.
pub fn detect_malformed_actions(text: &str) -> Vec<String> {
    const MARKERS: [&str; 5] = [
        "dpx:write path=",
        "dpx:edit path=",
        "dpx:read path=",
        "dpx:delete path=",
        "dpx:search pattern=",
    ];
    let mut warnings = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim();
        if let Some(info) = t.strip_prefix("```") {
            let is_edit = parse_edit_marker(info).is_some()
                || lines.peek().is_some_and(|n| parse_edit_marker(n).is_some());
            let mut edit_path = parse_edit_marker(info);
            let (mut has_search, mut has_sep, mut has_replace) = (false, false, false);
            for body in lines.by_ref() {
                let bt = body.trim();
                if bt.starts_with("```") {
                    break;
                }
                if edit_path.is_none() {
                    edit_path = parse_edit_marker(bt);
                }
                match bt {
                    "<<<<<<< SEARCH" => has_search = true,
                    "=======" => has_sep = true,
                    ">>>>>>> REPLACE" => has_replace = true,
                    _ => {}
                }
            }
            if is_edit && !(has_search && has_sep && has_replace) {
                let target = edit_path.map(|p| format!(" de `{p}`")).unwrap_or_default();
                warnings.push(format!(
                    "[AVISO: el bloque dpx:edit{target} está malformado (faltan los marcadores exactos \
                     `<<<<<<< SEARCH`, `=======` o `>>>>>>> REPLACE`) — NO se aplicó nada. Re-emítelo completo.]"
                ));
            }
            continue;
        }
        if MARKERS.iter().any(|m| t.starts_with(m)) {
            let shown: String = t.chars().take(60).collect();
            warnings.push(format!(
                "[AVISO: emitiste `{shown}` FUERA de un bloque ``` — la acción NO se ejecutó. \
                 Re-emítela dentro de un bloque ```dpx:...```.]"
            ));
        } else if t == "<<<<<<< SEARCH" || t == ">>>>>>> REPLACE" {
            warnings.push(
                "[AVISO: hay marcadores SEARCH/REPLACE fuera de un bloque ```dpx:edit — la edición \
                 NO se aplicó. Re-emítela dentro del bloque.]"
                    .to_string(),
            );
        }
    }
    warnings
}

/// Árbol de archivos del proyecto (para que el mentor sepa qué existe).
pub fn project_tree(root: &Path) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    tree_walk(root, "", 0, &mut out, &mut count);
    if out.is_empty() {
        out.push_str("(proyecto vacío)\n");
    }
    out
}

fn tree_walk(dir: &Path, prefix: &str, depth: usize, out: &mut String, count: &mut usize) {
    // Profundo a propósito: los paquetes Java anidan mucho (src/main/java/com/app/…),
    // y si cortamos pronto el modelo no ve los archivos .java reales.
    const MAX_DEPTH: usize = 10;
    const MAX_ENTRIES: usize = 300;
    if depth > MAX_DEPTH {
        return;
    }
    let mut entries: Vec<_> = match fs::read_dir(dir) {
        Ok(e) => e.flatten().collect(),
        Err(_) => return,
    };
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || matches!(name.as_str(), "target" | "node_modules" | "build" | "dist") {
            continue;
        }
        *count += 1;
        if *count > MAX_ENTRIES {
            out.push_str(&format!("{prefix}… (truncado)\n"));
            return;
        }
        let is_dir = entry.path().is_dir();
        out.push_str(&format!("{prefix}{name}{}\n", if is_dir { "/" } else { "" }));
        if is_dir {
            tree_walk(&entry.path(), &format!("{prefix}  "), depth + 1, out, count);
        }
    }
}

/// Extrae las propuestas `dpx:write` del texto de una respuesta del mentor.
///
/// Acepta el marcador de dos formas (los modelos varían): en el propio fence
/// (```` ```dpx:write path=… ````) o como primera línea dentro de un bloque de
/// código normal (p.ej. tras ```` ```xml ````).
pub fn parse_writes(text: &str) -> Vec<FileWrite> {
    let mut writes = Vec::new();
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
        let Some(info) = line.trim_start().strip_prefix("```") else {
            continue;
        };

        // ¿El marcador está en el fence, o en la primera línea de dentro?
        let mut path = parse_path_marker(info);
        let mut skip_marker_line = false;
        if path.is_none() {
            if let Some(first) = lines.peek() {
                if let Some(p) = parse_path_marker(first) {
                    path = Some(p);
                    skip_marker_line = true;
                }
            }
        }

        match path {
            Some(path) => {
                if skip_marker_line {
                    lines.next(); // descartar la línea del marcador
                }
                let mut content = String::new();
                for body in lines.by_ref() {
                    if body.trim_start().starts_with("```") {
                        break;
                    }
                    content.push_str(body);
                    content.push('\n');
                }
                writes.push(FileWrite { path, content });
            }
            // Bloque de código normal: consumir hasta su cierre para no confundir
            // el ``` final con la apertura de otro bloque.
            None => {
                for body in lines.by_ref() {
                    if body.trim_start().starts_with("```") {
                        break;
                    }
                }
            }
        }
    }

    writes
}

/// Una edición quirúrgica propuesta: reemplaza `search` (texto literal del
/// archivo) por `replace`, sin reescribir el archivo entero.
pub struct FileEdit {
    pub path: String,
    pub search: String,
    pub replace: String,
}

/// Extrae el marcador `dpx:edit ... path=<ruta>` de una línea, si lo tiene.
pub fn parse_edit_marker(s: &str) -> Option<String> {
    let rest = s.trim().strip_prefix("dpx:edit")?;
    rest.split_whitespace()
        .find_map(|tok| tok.strip_prefix("path="))
        .map(|p| p.trim_matches('"').to_string())
        .filter(|p| !p.is_empty())
}

/// Extrae las ediciones `dpx:edit` de una respuesta. Dentro de cada bloque, un
/// par `<<<<<<< SEARCH` / `=======` / `>>>>>>> REPLACE` define la edición (se
/// admiten varios pares por bloque, todos sobre el mismo archivo).
pub fn parse_edits(text: &str) -> Vec<FileEdit> {
    let mut edits = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(info) = line.trim_start().strip_prefix("```") else {
            continue;
        };
        let mut path = parse_edit_marker(info);
        if path.is_none() {
            if let Some(first) = lines.peek() {
                if let Some(p) = parse_edit_marker(first) {
                    path = Some(p);
                    lines.next();
                }
            }
        }
        let Some(path) = path else { continue };

        let mut search = String::new();
        let mut replace = String::new();
        let mut state = EditState::Outside;
        for body in lines.by_ref() {
            if body.trim_start().starts_with("```") {
                break;
            }
            match body.trim() {
                "<<<<<<< SEARCH" => {
                    search.clear();
                    replace.clear();
                    state = EditState::InSearch;
                }
                "=======" if state == EditState::InSearch => state = EditState::InReplace,
                ">>>>>>> REPLACE" => {
                    if state == EditState::InReplace && !search.trim().is_empty() {
                        edits.push(FileEdit {
                            path: path.clone(),
                            search: take_block(&mut search),
                            replace: take_block(&mut replace),
                        });
                    }
                    search.clear();
                    replace.clear();
                    state = EditState::Outside;
                }
                _ => match state {
                    EditState::InSearch => {
                        search.push_str(body);
                        search.push('\n');
                    }
                    EditState::InReplace => {
                        replace.push_str(body);
                        replace.push('\n');
                    }
                    EditState::Outside => {}
                },
            }
        }
    }
    edits
}

#[derive(PartialEq)]
enum EditState {
    Outside,
    InSearch,
    InReplace,
}

/// Vacía el acumulador quitando el `\n` final que añade el parser línea a línea
/// (así la búsqueda literal no exige un salto de línea tras el fragmento).
fn take_block(s: &mut String) -> String {
    let mut out = std::mem::take(s);
    if out.ends_with('\n') {
        out.pop();
    }
    out
}

/// Aplica una edición sobre el contenido actual: busca `search` de forma LITERAL
/// (sin regex) y reemplaza su primera aparición. Error claro si no aparece.
pub fn apply_edit(current: &str, edit: &FileEdit) -> Result<String> {
    let Some(idx) = current.find(&edit.search) else {
        return Err(anyhow!(
            "no encontré el bloque SEARCH en `{}`: el texto no coincide con el archivo actual \
             (¿cambió el archivo o la indentación es distinta?)",
            edit.path
        ));
    };
    let mut out = String::with_capacity(current.len() + edit.replace.len());
    out.push_str(&current[..idx]);
    out.push_str(&edit.replace);
    out.push_str(&current[idx + edit.search.len()..]);
    Ok(out)
}

/// Resuelve la ruta destino DENTRO del proyecto, rechazando rutas absolutas o
/// que escapen del directorio con `..`.
pub fn safe_target(project_root: &Path, rel: &str) -> Result<PathBuf> {
    let rel_path = Path::new(rel);
    if rel_path.is_absolute() {
        return Err(anyhow!("ruta absoluta no permitida: {rel}"));
    }
    for comp in rel_path.components() {
        match comp {
            Component::ParentDir => return Err(anyhow!("la ruta no puede salir del proyecto: {rel}")),
            Component::Prefix(_) | Component::RootDir => {
                return Err(anyhow!("ruta no permitida: {rel}"));
            }
            _ => {}
        }
    }
    Ok(project_root.join(rel_path))
}

/// Escribe el archivo en disco, creando los directorios padres necesarios.
pub fn apply(project_root: &Path, write: &FileWrite) -> Result<PathBuf> {
    let target = safe_target(project_root, &write.path)?;
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| anyhow!("no pude crear {}: {e}", parent.display()))?;
    }
    fs::write(&target, &write.content)
        .map_err(|e| anyhow!("no pude escribir {}: {e}", target.display()))?;
    Ok(target)
}

/// ¿El archivo destino ya existe? (para avisar de sobrescritura en el preview)
pub fn exists(project_root: &Path, rel: &str) -> bool {
    safe_target(project_root, rel).map(|p| p.exists()).unwrap_or(false)
}

/// Contenido actual del archivo (sin tope de líneas), o `None` si no existe o no
/// se puede leer. Sirve de base para el diff del preview de escritura.
pub fn current_content(project_root: &Path, rel: &str) -> Option<String> {
    let target = safe_target(project_root, rel).ok()?;
    fs::read_to_string(target).ok()
}

/// Lee un archivo del proyecto (para referencias `@archivo`), con las mismas
/// reglas de seguridad y un tope de líneas para no inundar el contexto. El tope
/// es generoso a propósito: un archivo fuente que el modelo va a EDITAR debe
/// verse entero (si no, propone ediciones sobre código que no leyó).
pub fn read_file(project_root: &Path, rel: &str) -> Result<String> {
    const MAX_LINES: usize = 2500;
    let target = safe_target(project_root, rel)?;
    let data = fs::read_to_string(&target)
        .map_err(|e| anyhow!("no pude leer {}: {e}", target.display()))?;

    let total = data.lines().count();
    if total > MAX_LINES {
        let head: String = data.lines().take(MAX_LINES).collect::<Vec<_>>().join("\n");
        Ok(format!(
            "{head}\n…[truncado: {total} líneas en total, mostradas {MAX_LINES}]"
        ))
    } else {
        Ok(data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marcador_en_el_fence() {
        let text = "intro\n```dpx:write path=src/App.java\nclase\n```\nfin";
        let w = parse_writes(text);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].path, "src/App.java");
        assert_eq!(w[0].content, "clase\n");
    }

    #[test]
    fn marcador_tras_lenguaje() {
        // Caso real: el modelo pone ```xml y el marcador en la línea siguiente.
        let text = "ejemplo\n```xml\ndpx:write path=pom.xml\n<project/>\n```";
        let w = parse_writes(text);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].path, "pom.xml");
        assert_eq!(w[0].content, "<project/>\n");
    }

    #[test]
    fn bloque_normal_se_ignora() {
        let text = "mira:\n```java\nint x = 1;\n```\nnada que escribir";
        assert!(parse_writes(text).is_empty());
    }

    #[test]
    fn varios_archivos() {
        let text = "```dpx:write path=a.txt\nA\n```\ny\n```dpx:write path=b.txt\nB\n```";
        let w = parse_writes(text);
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].path, "a.txt");
        assert_eq!(w[1].path, "b.txt");
    }

    #[test]
    fn touches_build_detecta_fuente_y_manifiesto() {
        let java = vec![FileWrite { path: "src/main/java/com/app/Foo.java".into(), content: String::new() }];
        let pom = vec![FileWrite { path: "pom.xml".into(), content: String::new() }];
        let rust = vec![FileWrite { path: "src/cli/chat.rs".into(), content: String::new() }];
        let doc = vec![FileWrite { path: "README.md".into(), content: String::new() }];
        assert!(touches_build(&java));
        assert!(touches_build(&pom));
        assert!(touches_build(&rust));
        assert!(!touches_build(&doc));
    }

    #[test]
    fn rechaza_rutas_inseguras() {
        let root = Path::new("/proj");
        assert!(safe_target(root, "../escape.txt").is_err());
        assert!(safe_target(root, "ok/inside.txt").is_ok());
    }

    #[test]
    fn parse_edit_basico() {
        let text = "cambio:\n```dpx:edit path=src/A.java\n<<<<<<< SEARCH\nfoo\n=======\nbar\n>>>>>>> REPLACE\n```";
        let e = parse_edits(text);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].path, "src/A.java");
        assert_eq!(e[0].search, "foo");
        assert_eq!(e[0].replace, "bar");
    }

    #[test]
    fn parse_edit_preserva_indentacion() {
        let text = "```dpx:edit path=a.py\n<<<<<<< SEARCH\n    def x():\n        pass\n=======\n    def x():\n        return 1\n>>>>>>> REPLACE\n```";
        let e = parse_edits(text);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].search, "    def x():\n        pass");
        assert_eq!(e[0].replace, "    def x():\n        return 1");
    }

    #[test]
    fn parse_edit_varios_bloques() {
        let text = "```dpx:edit path=a.txt\n<<<<<<< SEARCH\nuno\n=======\n1\n>>>>>>> REPLACE\n```\ny\n```dpx:edit path=b.txt\n<<<<<<< SEARCH\ndos\n=======\n2\n>>>>>>> REPLACE\n```";
        let e = parse_edits(text);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].path, "a.txt");
        assert_eq!(e[1].path, "b.txt");
    }

    #[test]
    fn parse_edit_ignora_bloques_normales() {
        let text = "```java\nint x = 1;\n```";
        assert!(parse_edits(text).is_empty());
    }

    #[test]
    fn apply_edit_reemplaza_primera_aparicion() {
        let edit = FileEdit {
            path: "x".into(),
            search: "fn a() {}".into(),
            replace: "fn b(n: u8) {}".into(),
        };
        let out = apply_edit("antes\nfn a() {}\ndespués\n", &edit).unwrap();
        assert_eq!(out, "antes\nfn b(n: u8) {}\ndespués\n");
    }

    #[test]
    fn apply_edit_falla_si_no_encuentra() {
        let edit = FileEdit { path: "x".into(), search: "no está".into(), replace: "y".into() };
        assert!(apply_edit("contenido real", &edit).is_err());
    }

    #[test]
    fn run_command_basico_entrega_lineas_y_exit_code() {
        let dir = std::env::temp_dir();
        let mut lines = Vec::new();
        let res = run_command_streaming(
            &dir,
            "echo hola",
            30,
            &mut |l| lines.push(l.to_string()),
            &|| false,
        );
        assert!(!res.cancelled);
        assert!(res.output.contains("exit code: 0"));
        assert!(lines.iter().any(|l| l.contains("hola")));
    }

    #[test]
    fn run_command_corta_por_timeout() {
        let dir = std::env::temp_dir();
        let slow = if cfg!(windows) { "ping -n 10 127.0.0.1 > NUL" } else { "sleep 10" };
        let t = std::time::Instant::now();
        let res = run_command_streaming(&dir, slow, 1, &mut |_| {}, &|| false);
        assert!(res.output.contains("TIMEOUT"));
        assert!(!res.cancelled);
        assert!(t.elapsed().as_secs() < 8);
    }

    #[test]
    fn run_command_cancelado_mata_el_proceso() {
        let dir = std::env::temp_dir();
        let slow = if cfg!(windows) { "ping -n 10 127.0.0.1 > NUL" } else { "sleep 10" };
        let t = std::time::Instant::now();
        let res = run_command_streaming(&dir, slow, 30, &mut |_| {}, &|| true);
        assert!(res.cancelled);
        assert!(res.output.contains("interrumpido"));
        assert!(t.elapsed().as_secs() < 8);
    }

    #[test]
    fn malformado_marcador_fuera_de_bloque() {
        let text = "voy a escribir:\ndpx:write path=a.txt\ncontenido suelto\n";
        let w = detect_malformed_actions(text);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("FUERA"));
    }

    #[test]
    fn malformado_edit_sin_replace() {
        let text = "```dpx:edit path=a.txt\n<<<<<<< SEARCH\nfoo\n=======\nbar\n```";
        let w = detect_malformed_actions(text);
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("malformado"));
    }

    #[test]
    fn bloques_correctos_no_generan_avisos() {
        let text = "```dpx:write path=a.txt\nhola\n```\n\
                    ```dpx:edit path=b.txt\n<<<<<<< SEARCH\nx\n=======\ny\n>>>>>>> REPLACE\n```\n\
                    prosa normal con `dpx:read` mencionado inline";
        assert!(detect_malformed_actions(text).is_empty());
    }

    #[test]
    fn ejemplo_dentro_de_bloque_normal_no_avisa() {
        let text = "```text\ndpx:write path=ejemplo.txt\n```";
        assert!(detect_malformed_actions(text).is_empty());
    }

    #[test]
    fn detect_stack_prioridades() {
        let dir = std::env::temp_dir().join(format!("dpx-detect-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        assert_eq!(detect_stack(&dir), None);

        fs::write(dir.join("requirements.txt"), "").unwrap();
        assert_eq!(detect_stack(&dir), Some("python"));

        fs::write(dir.join("package.json"), r#"{"dependencies":{"express":"^4"}}"#).unwrap();
        assert_eq!(detect_stack(&dir), Some("node"));

        fs::write(dir.join("package.json"), r#"{"dependencies":{"react":"^18"}}"#).unwrap();
        assert_eq!(detect_stack(&dir), Some("react"));

        fs::write(dir.join("pom.xml"), "<project/>").unwrap();
        assert_eq!(detect_stack(&dir), Some("spring-boot"));

        fs::remove_dir_all(&dir).unwrap();
    }
}

/// Extrae el marcador `dpx:delete path=<ruta>`
pub fn parse_delete_marker(s: &str) -> Option<String> {
    let rest = s.trim().strip_prefix("dpx:delete")?;
    rest.split_whitespace()
        .find_map(|tok| tok.strip_prefix("path="))
        .map(|p| p.trim_matches('"').to_string())
        .filter(|p| !p.is_empty())
}

/// Extrae las peticiones de borrado `dpx:delete path=...`
pub fn parse_deletes(text: &str) -> Vec<String> {
    let mut deletes = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(info) = line.trim_start().strip_prefix("```") else { continue; };
        let mut path = parse_delete_marker(info);
        if path.is_none() {
            if let Some(first) = lines.peek() {
                if let Some(p) = parse_delete_marker(first) {
                    path = Some(p);
                    lines.next();
                }
            }
        }
        if let Some(p) = path {
            deletes.push(p);
            for body in lines.by_ref() {
                if body.trim_start().starts_with("```") { break; }
            }
        }
    }
    deletes
}

/// Extrae el marcador `dpx:search pattern=<patron>`
pub fn parse_search_marker(s: &str) -> Option<String> {
    let rest = s.trim().strip_prefix("dpx:search")?;
    // split_once para permitir espacios en el patr�n si no tiene comillas, o usar trim_matches
    let pat = rest.split_whitespace().find_map(|tok| tok.strip_prefix("pattern="));
    if let Some(p) = pat {
        let cleaned = p.trim_matches('"').to_string();
        if !cleaned.is_empty() { return Some(cleaned); }
    }
    // Fallback: si el patr�n tiene espacios y comillas
    if let Some(idx) = rest.find("pattern=\"") {
        let start = idx + 9;
        if let Some(end) = rest[start..].find('"') {
            return Some(rest[start..start+end].to_string());
        }
    }
    None
}

/// Extrae las peticiones de b�squeda `dpx:search pattern=...`
pub fn parse_searches(text: &str) -> Vec<String> {
    let mut searches = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(info) = line.trim_start().strip_prefix("```") else { continue; };
        let mut pat = parse_search_marker(info);
        if pat.is_none() {
            if let Some(first) = lines.peek() {
                if let Some(p) = parse_search_marker(first) {
                    pat = Some(p);
                    lines.next();
                }
            }
        }
        if let Some(p) = pat {
            searches.push(p);
            for body in lines.by_ref() {
                if body.trim_start().starts_with("```") { break; }
            }
        }
    }
    searches
}

/// Borra un archivo de forma segura.
pub fn delete_file(project_root: &Path, rel: &str) -> Result<()> {
    let target = safe_target(project_root, rel)?;
    if target.exists() {
        fs::remove_file(&target)
            .map_err(|e| anyhow::anyhow!("no pude borrar {}: {}", target.display(), e))?;
    }
    Ok(())
}

/// Busca una cadena o patrón (grep simple) en los archivos del proyecto.
pub fn search_in_project(cwd: &Path, pattern: &str) -> String {
    let cmd = if cwd.join(".git").exists() {
        format!("git grep -i -n \"{}\" -- \":!target\" \":!node_modules\" \":!build\"", pattern)
    } else if cfg!(windows) {
        format!("findstr /s /i /n /c:\"{}\" *.java *.xml *.yml *.properties *.rs *.md *.toml", pattern)
    } else {
        format!("grep -r -i -n \"{}\" --exclude-dir={{target,node_modules,build,.git}} .", pattern)
    };

    let out = run_command(cwd, &cmd);
    cap_tail(&out, 100) // Limitar a 100 líneas para no inundar el contexto
}

