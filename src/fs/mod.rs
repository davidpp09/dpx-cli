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
mod detect;
mod exec;
mod tree;

// Submódulos extraídos; se re-exportan para conservar las rutas públicas
// `crate::fs::*` que usan los call sites (sin cambios fuera de aquí).
pub use detect::{
    build_manifest, detect_build, detect_stack, detect_test, edits_touch_build, touches_build,
};
pub use exec::{RUN_TIMEOUT_SECS, run_command, run_command_streaming};
pub use tree::{project_tree, symbol_map};

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
        if path.is_none()
            && let Some(first) = lines.peek()
                && let Some(p) = parse_read_marker(first) {
                    path = Some(p);
                    lines.next();
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

/// Convierte un plan parseado (lista de pares hecho/texto) a formato Markdown
/// para persistencia entre sesiones.
pub fn plan_to_markdown(plan: &[(bool, String)]) -> String {
    let mut md = String::from("# Plan pendiente\n\n```dpx:plan\n");
    for (done, text) in plan {
        let marker = if *done { "[x]" } else { "[ ]" };
        md.push_str(&format!("{marker} {text}\n"));
    }
    md.push_str("```\n");
    md
}

/// Extrae el último plan de una lista de turnos de sesión (el más reciente
/// bloque `dpx:plan` emitido por el asistente).
pub fn extract_last_plan(turns: &[crate::session::Turn]) -> Option<Vec<(bool, String)>> {
    for turn in turns.iter().rev() {
        if turn.role == "assistant"
            && let Some(plan) = parse_plan(&turn.text) {
                return Some(plan);
            }
    }
    None
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
                || is_plan_fence(info)
                || crate::skill::is_skill_fence(info)
                || crate::agent_skill::is_learned_fence(info);
            let on_next = lines.peek().is_some_and(|n| {
                parse_path_marker(n).is_some()
                    || parse_read_marker(n).is_some()
                    || parse_edit_marker(n).is_some()
                    || parse_search_marker(n).is_some()
                    || parse_delete_marker(n).is_some()
                    || is_run_fence(n)
                    || is_plan_fence(n)
                    || crate::skill::is_skill_fence(n)
                    || crate::agent_skill::is_learned_fence(n)
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
        if path.is_none()
            && let Some(first) = lines.peek()
                && let Some(p) = parse_path_marker(first) {
                    path = Some(p);
                    skip_marker_line = true;
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
        if path.is_none()
            && let Some(first) = lines.peek()
                && let Some(p) = parse_edit_marker(first) {
                    path = Some(p);
                    lines.next();
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
/// (sin regex) y reemplaza su primera aparición.
///
/// Tolera diferencias de final de línea (CRLF vs LF), la causa nº 1 de fallos de
/// `edit_file` en Windows: el LLM emite el SEARCH con `\n` pero el archivo en
/// disco tiene `\r\n` (o una mezcla de ambos, p.ej. tras `git checkout` con
/// `core.autocrlf`). El reemplazo PRESERVA los finales de línea originales del
/// archivo, porque solo se sustituye el tramo coincidente.
///
/// Estrategia en capas, de la más estricta a la más tolerante (la primera que
/// acierta gana, así nunca degradamos un match exacto):
/// 1. **Exacto** — `str::find` literal; conserva el comportamiento previo.
/// 2. **Finales de línea** — normaliza CRLF↔LF en ambos lados y mapea el offset
///    de vuelta al original (cubre finales mixtos).
/// 3. **Indentación/espacios (fuzzy por línea)** — localiza el bloque comparando
///    líneas por su contenido SIN espacios de borde, así un SEARCH con la
///    indentación mal puesta (la causa nº 1 de fallos que quedaba) igual ubica
///    el tramo. Solo aplica si hay UNA coincidencia: si es ambigua, prefiere
///    fallar con pista a editar el lugar equivocado.
///
/// El reemplazo PRESERVA lo que hay alrededor: solo se sustituye el tramo
/// coincidente. Error claro con pista verbatim si ninguna capa acierta.
pub fn apply_edit(current: &str, edit: &FileEdit) -> Result<String> {
    // 1. Intento exacto.
    if let Some(idx) = current.find(&edit.search) {
        return Ok(splice(current, idx, idx + edit.search.len(), &edit.replace));
    }

    // 2. Tolerante a finales de línea: normaliza CRLF→LF en ambos lados y busca
    //    en el espacio normalizado; el `map` traduce el offset normalizado al
    //    byte original para cortar exactamente el tramo coincidente.
    let needle_norm = edit.search.replace("\r\n", "\n");
    let (current_norm, map) = normalize_lf_with_map(current);
    if let Some(n_idx) = current_norm.find(&needle_norm) {
        let start = map[n_idx];
        let end = map[n_idx + needle_norm.len()];
        return Ok(splice(current, start, end, &edit.replace));
    }

    // 3. Tolerante a indentación/espacios: match línea-a-línea por contenido
    //    recortado, solo si es inequívoco.
    if let Some((start, end)) = fuzzy_line_span(current, &edit.search) {
        return Ok(splice(current, start, end, &edit.replace));
    }

    Err(anyhow!(
        "no encontré el bloque SEARCH en `{}`: el texto no coincide con el archivo actual.{}",
        edit.path,
        search_hint(current, &edit.search)
    ))
}

/// Divide `s` en líneas devolviendo `(offset_de_byte_del_inicio, contenido)`,
/// donde `contenido` excluye el terminador (`\n` o `\r\n`). Sirve para mapear
/// una coincidencia por líneas de vuelta a offsets de byte exactos.
fn line_offsets(s: &str) -> Vec<(usize, &str)> {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut out = Vec::new();
    let mut i = 0;
    loop {
        let start = i;
        let mut j = i;
        while j < n && bytes[j] != b'\n' {
            j += 1;
        }
        // Excluir el `\r` de un `\r\n` del contenido.
        let mut content_end = j;
        if content_end > start && bytes[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        out.push((start, &s[start..content_end]));
        if j >= n {
            break;
        }
        i = j + 1;
    }
    out
}

/// Localiza el bloque `search` en `current` comparando líneas por su contenido
/// recortado (ignora indentación y espacios de borde). Devuelve el rango de
/// bytes `[inicio, fin]` del tramo en `current` (de la 1ª línea a la última, sin
/// el salto final) SOLO si hay exactamente UNA coincidencia; `None` si hay cero
/// o varias (ambiguo → mejor no tocar). Requiere al menos una línea con
/// contenido real, para no "encontrar" un bloque de puras líneas en blanco.
fn fuzzy_line_span(current: &str, search: &str) -> Option<(usize, usize)> {
    let needle: Vec<&str> = search.lines().map(str::trim).collect();
    if needle.is_empty() || needle.iter().all(|l| l.is_empty()) {
        return None;
    }
    let lines = line_offsets(current);
    let n = needle.len();
    if lines.len() < n {
        return None;
    }
    let mut found: Option<usize> = None;
    for i in 0..=(lines.len() - n) {
        let matches = (0..n).all(|k| lines[i + k].1.trim() == needle[k]);
        if matches {
            if found.is_some() {
                return None; // ambiguo: ≥2 coincidencias
            }
            found = Some(i);
        }
    }
    let i = found?;
    let last = i + n - 1;
    let start = lines[i].0;
    let end = lines[last].0 + lines[last].1.len();
    Some((start, end))
}

/// Reemplaza `original[start..end]` por `replace`, devolviendo la cadena nueva.
fn splice(original: &str, start: usize, end: usize, replace: &str) -> String {
    let mut out = String::with_capacity(original.len() + replace.len());
    out.push_str(&original[..start]);
    out.push_str(replace);
    out.push_str(&original[end..]);
    out
}

/// Normaliza `\r\n` → `\n` y devuelve `(normalizado, map)`, donde `map[i]` es el
/// índice de byte en `original` del que proviene el byte `i` del normalizado.
/// `map` tiene `normalizado.len() + 1` entradas y `map[normalizado.len()]` apunta
/// a `original.len()`, de modo que cualquier rango `[a, b]` del normalizado se
/// traduce a `[map[a], map[b]]` en el original. Solo toca ASCII (`\r`/`\n`), así
/// que preserva la validez UTF-8 y los límites de carácter.
fn normalize_lf_with_map(original: &str) -> (String, Vec<usize>) {
    let bytes = original.as_bytes();
    let mut norm: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut map: Vec<usize> = Vec::with_capacity(bytes.len() + 1);
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\r' && i + 1 < bytes.len() && bytes[i + 1] == b'\n' {
            // El '\n' del normalizado nace en el '\r' y consume 2 bytes.
            map.push(i);
            norm.push(b'\n');
            i += 2;
        } else {
            map.push(i);
            norm.push(bytes[i]);
            i += 1;
        }
    }
    map.push(bytes.len());
    // `norm` solo difiere de `original` en ASCII \r\n → \n: sigue siendo UTF-8
    // válido. El fallback a `original` jamás debería dispararse.
    let norm = String::from_utf8(norm).unwrap_or_else(|_| original.to_string());
    (norm, map)
}

/// Pista para un SEARCH fallido, pensada para que el MODELO se autocorrija en
/// vez de reintentar a ciegas: localiza en el archivo la zona más parecida
/// (anclando por la primera línea con contenido del search, comparada
/// ignorando espacios) y la devuelve VERBATIM para copiarla tal cual. La
/// causa nº 1 de estos fallos son diferencias invisibles: indentación,
/// escapes `\"` y los `\` de continuación en string literals.
fn search_hint(current: &str, search: &str) -> String {
    fn normalize(s: &str) -> String {
        s.split_whitespace().collect::<Vec<_>>().join(" ")
    }
    let Some(anchor) = search.lines().find(|l| !l.trim().is_empty()) else {
        return String::new();
    };
    let anchor_norm = normalize(anchor);
    let lines: Vec<&str> = current.lines().collect();
    let hit = lines.iter().position(|l| {
        let ln = normalize(l);
        ln == anchor_norm
            || (anchor_norm.chars().count() > 12
                && (ln.contains(&anchor_norm) || anchor_norm.contains(&ln) && !ln.is_empty()))
    });
    match hit {
        Some(i) => {
            let span = search.lines().count().clamp(3, 10);
            let end = (i + span).min(lines.len());
            let excerpt = lines[i..end].join("\n");
            format!(
                " La zona más parecida del archivo es esta — tu SEARCH difiere de ella en algo \
                 (espacios, `\\` de continuación o escapes). Copia este texto EXACTAMENTE:\n\
                 ---\n{excerpt}\n---"
            )
        }
        None => " Ni siquiera la primera línea de tu SEARCH aparece en el archivo (¿archivo o \
                 sección equivocada?). Relee el archivo fresco antes de reintentar."
            .to_string(),
    }
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
    // Snapshot del estado anterior (para `/undo`) ANTES de sobrescribir.
    crate::checkpoint::record_before(&target);
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

/// Tope de líneas por lectura para no inundar el contexto. Generoso a propósito:
/// un archivo que el modelo va a EDITAR debe verse entero.
const READ_MAX_LINES: usize = 2500;

/// Lee un archivo del proyecto (para referencias `@archivo` y el subagente),
/// desde el principio y con el tope por defecto.
pub fn read_file(project_root: &Path, rel: &str) -> Result<String> {
    read_file_range(project_root, rel, None, None)
}

/// Lee un archivo con soporte de RANGO: `offset` = línea inicial (1-based),
/// `limit` = nº máximo de líneas. Para archivos más largos que el tope, en vez
/// de cortar a ciegas le dice al modelo cuántas líneas faltan y con qué `offset`
/// pedirlas — así puede leer el FINAL de un archivo grande con su propia
/// herramienta (read_file), sin inventar scripts para hacer `tail`.
pub fn read_file_range(
    project_root: &Path,
    rel: &str,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<String> {
    let target = safe_target(project_root, rel)?;
    let data = fs::read_to_string(&target)
        .map_err(|e| anyhow!("no pude leer {}: {e}", target.display()))?;

    let all: Vec<&str> = data.lines().collect();
    let total = all.len();
    let start = offset.unwrap_or(1).max(1); // 1-based
    let limit = limit.unwrap_or(READ_MAX_LINES);

    // Lectura por defecto de un archivo que cabe entero: devuelve el contenido
    // EXACTO (preserva el salto final), como antes.
    if start == 1 && total <= limit {
        return Ok(data);
    }
    if start > total {
        return Ok(format!(
            "[el archivo `{rel}` tiene {total} líneas; pediste desde la {start}, que no existe]"
        ));
    }

    let start_idx = start - 1;
    let end_idx = (start_idx + limit).min(total);
    let slice = all[start_idx..end_idx].join("\n");
    let mut out = format!("[`{rel}` líneas {start}–{end_idx} de {total}]\n{slice}");
    if end_idx < total {
        out.push_str(&format!(
            "\n…[faltan {} líneas; para verlas vuelve a llamar a read_file con offset={}]",
            total - end_idx,
            end_idx + 1
        ));
    }
    Ok(out)
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
        if path.is_none()
            && let Some(first) = lines.peek()
                && let Some(p) = parse_delete_marker(first) {
                    path = Some(p);
                    lines.next();
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
    let pat = rest.split_whitespace().find_map(|tok| tok.strip_prefix("pattern="));
    if let Some(p) = pat {
        let cleaned = p.trim_matches('"').to_string();
        if !cleaned.is_empty() { return Some(cleaned); }
    }
    if let Some(idx) = rest.find("pattern=\"") {
        let start = idx + 9;
        if let Some(end) = rest[start..].find('"') {
            return Some(rest[start..start+end].to_string());
        }
    }
    None
}

/// Extrae las peticiones de búsqueda `dpx:search pattern=...`
pub fn parse_searches(text: &str) -> Vec<String> {
    let mut searches = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(info) = line.trim_start().strip_prefix("```") else { continue; };
        let mut pat = parse_search_marker(info);
        if pat.is_none()
            && let Some(first) = lines.peek()
                && let Some(p) = parse_search_marker(first) {
                    pat = Some(p);
                    lines.next();
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
        // Snapshot del contenido ANTES de borrar (para `/undo`).
        crate::checkpoint::record_before(&target);
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
    cap_tail(&out, 100)
}

/// Stems demasiado genéricos para el orphan-sweep: aparecen en medio proyecto,
/// así que avisar por ellos sería ruido constante. Mejor callar que dar falsos.
const GENERIC_STEMS: &[&str] = &[
    "mod", "lib", "main", "index", "types", "type", "utils", "util", "app", "config",
    "init", "test", "tests", "helpers", "helper", "common", "core", "api", "client",
    "server", "model", "models", "view", "views", "style", "styles", "router", "routes",
];

/// ¿Una línea de `git grep` es un match real (`ruta:NNN:contenido`)? Filtra
/// cabeceras (`--- stdout ---`, `exit code: N`) y posibles líneas de error.
fn looks_like_grep_hit(l: &str) -> bool {
    let mut parts = l.splitn(3, ':');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(_path), Some(num), Some(_rest)) => num.trim().parse::<u32>().is_ok(),
        _ => false,
    }
}

/// ¿El hit cae en un directorio de build/artefactos que no interesa? Como el
/// orphan-sweep usa `git grep --no-index` (no respeta `.gitignore`), filtramos
/// estas rutas a mano para no avisar por copias en target/node_modules/etc.
fn in_build_dir(hit: &str) -> bool {
    let path = hit.split(':').next().unwrap_or("").replace('\\', "/");
    const SKIP: &[&str] = &["target/", "node_modules/", "build/", "dist/", ".git/", ".dpx/"];
    SKIP.iter().any(|p| path.starts_with(p) || path.contains(&format!("/{p}")))
}

/// ORPHAN-SWEEP: tras borrar `deleted_rel`, busca referencias que quedaron
/// colgando a su nombre de módulo (el *stem* del archivo) como PALABRA completa,
/// para que el agente las limpie y no deje imports/usos rotos — el fallo nº1 de
/// dpx (borra/renombra y deja referencias). El archivo ya fue borrado, así que
/// no se cuenta a sí mismo. Usa `git grep --untracked` para ver también los
/// archivos que dpx acaba de escribir sin commitear. Devuelve las líneas
/// encontradas (acotadas), o `None` si el stem es genérico (ruido), no hay git,
/// o no quedó ninguna referencia.
pub fn orphan_refs(cwd: &Path, deleted_rel: &str) -> Option<String> {
    let stem = Path::new(deleted_rel).file_stem()?.to_str()?.to_string();
    if stem.len() < 3
        || stem.chars().any(|c| c.is_whitespace()) // evita líos de quoting en cmd
        || GENERIC_STEMS.contains(&stem.to_lowercase().as_str())
    {
        return None;
    }
    if !cwd.join(".git").exists() {
        return None; // el sweep determinista se apoya en git grep
    }
    // `--no-index` busca también lo NO trackeado (lo que dpx acaba de escribir
    // sin commitear) — `--untracked` resultó poco fiable entre versiones de git.
    // `-F` literal, `-w` palabra completa (recorta el ruido de subcadenas). No
    // respeta .gitignore, así que `in_build_dir` filtra target/node_modules/etc.
    let out = run_command(cwd, &format!("git grep --no-index -F -w -n {stem}"));
    let refs: Vec<&str> = out
        .lines()
        .filter(|l| looks_like_grep_hit(l) && !in_build_dir(l))
        .collect();
    if refs.is_empty() {
        return None;
    }
    Some(refs.into_iter().take(20).collect::<Vec<_>>().join("\n"))
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
    fn apply_edit_tolera_crlf_en_archivo_con_search_lf() {
        // Caso real en Windows: el archivo tiene \r\n, el SEARCH del LLM usa \n.
        let current = "línea uno\r\nlínea dos\r\nlínea tres\r\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "línea uno\nlínea dos".into(),  // solo LF
            replace: "reemplazada".into(),
        };
        let out = apply_edit(current, &edit).unwrap();
        // El contenido se reemplaza respetando los \r\n originales.
        assert_eq!(out, "reemplazada\r\nlínea tres\r\n");
    }

    #[test]
    fn apply_edit_tolera_crlf_en_search_con_archivo_lf() {
        // Caso inverso (raro pero posible): archivo LF, SEARCH con CRLF.
        let current = "fn foo() {\n    bar();\n}\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "fn foo() {\r\n    bar();".into(),
            replace: "fn baz() {".into(),
        };
        let out = apply_edit(current, &edit).unwrap();
        assert_eq!(out, "fn baz() {\n}\n");
    }

    #[test]
    fn apply_edit_exacto_sigue_funcionando() {
        // Sin diferencias CRLF, el comportamiento no cambia.
        let edit = FileEdit {
            path: "x".into(),
            search: "línea uno\nlínea dos".into(),
            replace: "reemplazada".into(),
        };
        let out = apply_edit("línea uno\nlínea dos\nlínea tres\n", &edit).unwrap();
        assert_eq!(out, "reemplazada\nlínea tres\n");
    }

    #[test]
    fn apply_edit_solo_lf_con_crlf_en_medio_del_contenido() {
        // El archivo mezcla CRLF y LF (pasa en la práctica: git checkout en
        // Windows con core.autocrlf=true puede dejar mezclas).
        let current = "cabecera\ncuerpo\r\npie\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "cuerpo".into(), // sin newlines, exact match basta
            replace: "nuevo".into(),
        };
        let out = apply_edit(current, &edit).unwrap();
        assert_eq!(out, "cabecera\nnuevo\r\npie\n");
    }

    #[test]
    fn apply_edit_tolera_finales_mixtos_multilinea() {
        // El bloque a editar ABARCA líneas con finales distintos: la primera
        // termina en \r\n y la segunda en \n. El SEARCH del LLM viene todo en
        // \n. El heurístico simple de \n↔\r\n NO resolvía esto (normalizaba a
        // todo-CRLF y no encontraba); el mapeo normalizado sí.
        let current = "uno\r\ndos\ntres\r\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "uno\ndos\ntres".into(), // todo LF, cruza CRLF y LF reales
            replace: "fusionado".into(),
        };
        let out = apply_edit(current, &edit).unwrap();
        // Reemplaza el tramo coincidente y preserva el \r\n final del archivo.
        assert_eq!(out, "fusionado\r\n");
    }

    #[test]
    fn apply_edit_finales_mixtos_preserva_cola_intacta() {
        // Verifica que el mapeo de offsets corta exactamente: el texto antes y
        // después del match queda byte-a-byte intacto, con sus finales propios.
        let current = "head\r\nfn f() {\n    body();\r\n}\ntail\r\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "fn f() {\n    body();\n}".into(), // LF, pero el archivo mezcla
            replace: "fn f() { done(); }".into(),
        };
        let out = apply_edit(current, &edit).unwrap();
        assert_eq!(out, "head\r\nfn f() { done(); }\ntail\r\n");
    }

    #[test]
    fn apply_edit_fuzzy_tolera_indentacion_mal_puesta() {
        // El modelo emitió el SEARCH SIN indentación; el archivo la tiene.
        // Ni el exacto ni el de CRLF lo encuentran; el fuzzy por línea sí.
        let current = "fn main() {\n    let x = 1;\n    println!(\"{}\", x);\n}\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "let x = 1;\nprintln!(\"{}\", x);".into(), // sin los 4 espacios
            replace: "    let y = 2;\n    dbg!(y);".into(),
        };
        let out = apply_edit(current, &edit).unwrap();
        assert_eq!(out, "fn main() {\n    let y = 2;\n    dbg!(y);\n}\n");
    }

    #[test]
    fn apply_edit_fuzzy_una_sola_linea_con_espacios_de_sobra() {
        // El SEARCH trae espacios al final que el archivo no tiene → no es
        // substring literal (exacto/CRLF fallan), pero el fuzzy lo ubica. Como
        // el fuzzy reemplaza la LÍNEA completa (con su indentación), el REPLACE
        // debe traer la indentación deseada.
        let current = "a = 0\n    value = 1\nb = 2\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "value = 1   ".into(), // espacios sobrantes al final
            replace: "    value = 99".into(),
        };
        assert!(current.find(&edit.search).is_none(), "no debe ser match exacto");
        let out = apply_edit(current, &edit).unwrap();
        assert_eq!(out, "a = 0\n    value = 99\nb = 2\n");
    }

    #[test]
    fn apply_edit_fuzzy_ambiguo_no_toca_y_da_error() {
        // El MISMO bloque (salvo indentación) aparece dos veces. El SEARCH sin
        // indentar no es substring literal (hay espacios entre las líneas en el
        // archivo), así que exacto/CRLF fallan y se llega al fuzzy; al haber dos
        // coincidencias, se abstiene y devuelve error en vez de editar a ciegas.
        let current =
            "fn a() {\n    do_x();\n    do_y();\n}\nfn b() {\n        do_x();\n        do_y();\n}\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "do_x();\ndo_y();".into(),
            replace: "done();".into(),
        };
        assert!(fuzzy_line_span(current, &edit.search).is_none(), "debe ser ambiguo");
        assert!(apply_edit(current, &edit).is_err());
    }

    #[test]
    fn apply_edit_fuzzy_no_inventa_match_con_lineas_en_blanco() {
        // Un SEARCH de puras líneas en blanco no debe "encontrar" cualquier hueco.
        let current = "a\n\n\nb\n";
        assert!(fuzzy_line_span(current, "   \n  ").is_none());
        assert!(fuzzy_line_span(current, "").is_none());
    }

    #[test]
    fn search_fallido_da_pista_con_el_texto_real() {
        // La primera línea del search existe (con otra indentación), pero la
        // segunda no coincide: la pista debe traer el texto REAL del archivo
        // desde esa ancla, para que el modelo lo copie.
        let current =
            "fn x() {\n        let valor = calcular_total(precios); // exacto\n        valor + 1\n}\n";
        let edit = FileEdit {
            path: "x".into(),
            search: "let valor = calcular_total(precios); // exacto\n    return valor;".into(),
            replace: "y".into(),
        };
        let err = apply_edit(current, &edit).unwrap_err().to_string();
        assert!(err.contains("Copia este texto EXACTAMENTE"));
        assert!(err.contains("        let valor = calcular_total(precios); // exacto"));
    }

    #[test]
    fn search_sin_relacion_pide_releer() {
        let edit = FileEdit {
            path: "x".into(),
            search: "esta línea no existe en ninguna forma".into(),
            replace: "y".into(),
        };
        let err = apply_edit("contenido\ncompletamente distinto\n", &edit).unwrap_err().to_string();
        assert!(err.contains("Relee el archivo fresco"));
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
        // Fuente de verdad del green-gate: el código de salida REAL, no el texto.
        assert_eq!(res.exit_code, Some(0));
        assert!(lines.iter().any(|l| l.contains("hola")));
    }

    #[test]
    fn run_command_fallido_expone_exit_code_distinto_de_cero() {
        // El green-gate distingue verde (Some(0)) de rojo por el código REAL,
        // que sobrevive aunque `cap_tail` recorte la línea `exit code:` en una
        // salida larga. Un comando que sale con error debe reportarse en rojo.
        let dir = std::env::temp_dir();
        let fail = if cfg!(windows) { "cmd /c exit 1" } else { "sh -c 'exit 1'" };
        let res = run_command_streaming(&dir, fail, 30, &mut |_| {}, &|| false);
        assert!(!res.cancelled);
        assert_ne!(res.exit_code, Some(0), "un fallo no puede pasar por verde");
        assert_eq!(res.exit_code, Some(1));
    }

    #[test]
    fn orphan_refs_detecta_referencias_colgando() {
        let dir = std::env::temp_dir().join(format!("dpx-orphan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Repo git mínimo (el sweep se apoya en git grep --untracked).
        run_command(&dir, "git init");
        std::fs::write(dir.join("app.rs"), "mod widget;\nfn main() { widget::run(); }\n").unwrap();
        std::fs::write(dir.join("readme.md"), "nada que ver aquí\n").unwrap();
        // Tras 'borrar' widget.rs, su módulo sigue referenciado en app.rs.
        let refs = orphan_refs(&dir, "src/widget.rs").expect("debería detectar la referencia colgando");
        assert!(refs.contains("app.rs"), "el orphan-sweep debe señalar app.rs, vi: {refs}");
        // Un stem genérico NO dispara el sweep (sería puro ruido).
        assert!(orphan_refs(&dir, "src/mod.rs").is_none());
        // Un módulo sin referencias tampoco avisa.
        assert!(orphan_refs(&dir, "src/inexistente_xyz.rs").is_none());
        let _ = std::fs::remove_dir_all(&dir);
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

    #[test]
    fn detect_build_y_test_por_stack() {
        let dir = std::env::temp_dir().join(format!("dpx-verify-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // Sin manifiesto reconocible: nada que verificar.
        assert_eq!(detect_build(&dir), None);
        assert_eq!(detect_test(&dir), None);

        // Cargo: build = clippy estricto (check + deniega lints/dead-code),
        // test = la suite completa.
        fs::write(dir.join("Cargo.toml"), "[package]\nname=\"x\"\n").unwrap();
        let build = detect_build(&dir).unwrap();
        assert!(build.contains("clippy") && build.contains("-D warnings"), "build: {build}");
        assert_eq!(detect_test(&dir).as_deref(), Some("cargo test --quiet"));
        fs::remove_file(dir.join("Cargo.toml")).unwrap();

        // Maven sin wrapper: compile salta tests; test los corre.
        fs::write(dir.join("pom.xml"), "<project/>").unwrap();
        assert!(detect_build(&dir).unwrap().contains("-DskipTests"));
        assert!(detect_test(&dir).unwrap().contains("test"));
        assert!(!detect_test(&dir).unwrap().contains("-DskipTests"));

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn read_file_range_lee_el_final_de_un_archivo_grande() {
        let dir = std::env::temp_dir().join(format!("dpx-read-range-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // Archivo de 3000 líneas: "linea N" en cada una.
        let body: String = (1..=3000).map(|n| format!("linea {n}\n")).collect();
        fs::write(dir.join("big.txt"), &body).unwrap();

        // Lectura por defecto (sin rango): trunca al tope y dice cómo seguir.
        let head = read_file_range(&dir, "big.txt", None, None).unwrap();
        assert!(head.contains("linea 1\n"));
        assert!(!head.contains("linea 3000"), "no debe llegar al final por defecto");
        assert!(head.contains("offset="), "debe indicar con qué offset seguir");

        // Lectura del FINAL con offset: lo que dpx necesitaba (en vez de un .py).
        let tail = read_file_range(&dir, "big.txt", Some(2900), Some(500)).unwrap();
        assert!(tail.contains("linea 2900"));
        assert!(tail.contains("linea 3000"), "el final debe estar accesible por offset");
        assert!(tail.contains("líneas 2900–3000 de 3000"));

        // Archivo pequeño leído entero: contenido exacto, sin cabecera de rango.
        fs::write(dir.join("small.txt"), "uno\ndos\n").unwrap();
        let small = read_file_range(&dir, "small.txt", None, None).unwrap();
        assert_eq!(small, "uno\ndos\n");

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_maven_spring_boot_por_parent_pom() {
        let dir = std::env::temp_dir().join(format!("dpx-mvn-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("pom.xml"),
            r#"<project>
  <parent>
    <groupId>org.springframework.boot</groupId>
    <artifactId>spring-boot-starter-parent</artifactId>
    <version>3.2.0</version>
  </parent>
</project>"#,
        )
        .unwrap();
        assert_eq!(detect_stack(&dir), Some("spring-boot"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_maven_spring_boot_por_plugin() {
        let dir = std::env::temp_dir().join(format!("dpx-mvn-pl-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("pom.xml"),
            r#"<project>
  <build>
    <plugins>
      <plugin>
        <groupId>org.springframework.boot</groupId>
        <artifactId>spring-boot-maven-plugin</artifactId>
      </plugin>
    </plugins>
  </build>
</project>"#,
        )
        .unwrap();
        assert_eq!(detect_stack(&dir), Some("spring-boot"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_maven_simple_xml_sin_spring_cae_a_shallow() {
        // Nombre único: este dir NO puede colisionar con el de otro test que
        // corre en paralelo (mismo pid) — la causa del fallo flaky original.
        let dir = std::env::temp_dir().join(format!("dpx-mvn-simple-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("pom.xml"),
            r#"<project>
  <groupId>com.test</groupId>
  <artifactId>simple</artifactId>
</project>"#,
        )
        .unwrap();
        // shallow detecta pom.xml → spring-boot (fallback genérico para JVM)
        assert_eq!(detect_stack(&dir), Some("spring-boot"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_gradle_spring_boot_por_plugin() {
        let dir = std::env::temp_dir().join(format!("dpx-gradle-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("build.gradle"),
            r#"plugins {
  id 'org.springframework.boot' version '3.2.0'
}
dependencies {
  implementation 'org.springframework.boot:spring-boot-starter-web'
}"#,
        )
        .unwrap();
        assert_eq!(detect_stack(&dir), Some("spring-boot"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_gradle_sin_spring_cae_a_shallow() {
        let dir = std::env::temp_dir().join(format!("dpx-gradle-2-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("build.gradle"), "apply plugin: 'java'").unwrap();
        // shallow detecta build.gradle → gradle
        assert_eq!(detect_stack(&dir), Some("gradle"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_npm_next_js_como_react() {
        let dir = std::env::temp_dir().join(format!("dpx-next-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"dependencies":{"next":"^14","react":"^18"}}"#,
        )
        .unwrap();
        assert_eq!(detect_stack(&dir), Some("react"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_npm_gatsby_como_react() {
        let dir = std::env::temp_dir().join(format!("dpx-gatsby-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"dependencies":{"gatsby":"^5"}}"#,
        )
        .unwrap();
        assert_eq!(detect_stack(&dir), Some("react"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_npm_react_por_devdeps() {
        let dir = std::env::temp_dir().join(format!("dpx-rdep-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"devDependencies":{"react":"^18"}}"#,
        )
        .unwrap();
        assert_eq!(detect_stack(&dir), Some("react"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn detect_npm_peer_deps_reconocidas() {
        let dir = std::env::temp_dir().join(format!("dpx-peer-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("package.json"),
            r#"{"peerDependencies":{"react":"^18"}}"#,
        )
        .unwrap();
        assert_eq!(detect_stack(&dir), Some("react"));
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn plan_to_markdown_redondea_correctamente() {
        let plan = vec![
            (true, "Migrar a jakarta.*".to_string()),
            (false, "Añadir validación".to_string()),
            (false, "Escribir tests".to_string()),
        ];
        let md = plan_to_markdown(&plan);
        assert!(md.contains("[x] Migrar a jakarta.*"));
        assert!(md.contains("[ ] Añadir validación"));
        assert!(md.contains("[ ] Escribir tests"));
        assert!(md.contains("```dpx:plan"));
    }

    #[test]
    fn extract_last_plan_encuentra_el_mas_reciente() {
        let turns = vec![
            crate::session::Turn { role: "assistant", text: "```dpx:plan\n[x] Tarea 1\n```".into() },
            crate::session::Turn { role: "user", text: "ok".into() },
            crate::session::Turn { role: "assistant", text: "```dpx:plan\n[ ] Tarea 2\n```".into() },
        ];
        let plan = extract_last_plan(&turns).unwrap();
        assert_eq!(plan.len(), 1);
        assert!(!plan[0].0);
        assert_eq!(plan[0].1, "Tarea 2");
    }

    #[test]
    fn extract_last_plan_sin_plan_devuelve_none() {
        let turns = vec![
            crate::session::Turn { role: "assistant", text: "texto normal".into() },
            crate::session::Turn { role: "user", text: "hola".into() },
        ];
        assert!(extract_last_plan(&turns).is_none());
    }
}
