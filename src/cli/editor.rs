//! Editor de entrada: mini TUI con crossterm. Da multilínea real (Shift+Enter),
//! autocompletado inline en gris (ghost: `@archivo` y `/comando`), resaltado de
//! comandos y referencias mientras escribes, y una barra de atajos al pie con
//! el medidor de contexto. rustyline solo se mantiene para `confirm_line`.

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crossterm::cursor::{MoveToColumn, MoveUp, RestorePosition, SavePosition};
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind, KeyModifiers,
};
use crossterm::execute;
use crossterm::style::Print;
use crossterm::terminal::{self, Clear, ClearType};

use rustyline::error::ReadlineError;
use rustyline::{Config, Editor};

use crate::ui;

/// Comandos para el ghost autocompletado (orden alfabético → match estable).
/// El resaltado, en cambio, tiñe CUALQUIER `/palabra` inicial, no solo estos.
const COMMANDS: &[&str] = &[
    "actualizar", "auto", "ayuda", "cambios", "cerebro", "comité", "compactar",
    "contexto", "costo", "deshacer", "enfoque", "estado", "evaluar", "examen",
    "habilidades", "limpiar", "modelos", "modo", "presupuesto", "progreso",
    "revisar", "salir", "temario",
];

pub enum ReadResult {
    Line(String),
    Interrupted,
    Eof,
}

pub struct InputEditor {
    cwd: PathBuf,
    history: Vec<String>,
    confirmer: Editor<(), rustyline::history::DefaultHistory>,
    context: String,
    meter: String,
}

impl InputEditor {
    pub fn new(cwd: PathBuf) -> Self {
        let config = Config::builder()
            .max_history_size(1000)
            .unwrap()
            .build();
        let confirmer = Editor::with_config(config).expect("no se pudo crear el editor rustyline");
        Self {
            cwd,
            history: Vec::new(),
            confirmer,
            context: String::new(),
            meter: String::new(),
        }
    }

    /// Contexto inline del prompt: `focus  ·  modo` (en gris, antes de la flecha).
    pub fn set_context(&mut self, focus: &str, mode: &str) {
        self.context = format!("{focus}  ·  {mode}");
    }

    /// Medidor de contexto (barra ▓░ %) que se pinta en la barra de atajos al pie.
    pub fn set_meter(&mut self, meter: &str) {
        self.meter = meter.to_string();
    }

    pub fn read_input(&mut self) -> io::Result<ReadResult> {
        if !io::stdin().is_terminal() {
            return fallback_read_line("> ");
        }
        println!();
        // Prompt minimalista: solo `>` con el color del modo. El contexto
        // (focus · modo) se muestra ahora en la barra de atajos al pie.
        let prompt = format!("  {} ", ui::accent(">"));
        let prompt_width = ui::visible_width(&prompt);
        let result = raw_multiline_input(
            &prompt,
            prompt_width,
            &self.cwd,
            &self.history,
            &self.meter,
            &self.context,
        );
        if let Ok(ReadResult::Line(ref text)) = result {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                if self.history.last().map(String::as_str) != Some(trimmed) {
                    self.history.push(trimmed.to_string());
                }
                self.confirmer.add_history_entry(trimmed).ok();
            }
        }
        result
    }

    pub fn confirm_line(&mut self, prompt: &str) -> Option<String> {
        if !io::stdin().is_terminal() {
            print!("{prompt}");
            let _ = io::stdout().flush();
            let mut line = String::new();
            return match io::stdin().read_line(&mut line) {
                Ok(0) | Err(_) => None,
                Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_string()),
            };
        }
        crate::ui::print_confirmation_box(prompt);
        match self.confirmer.readline("") {
            Ok(line) => {
                let t = line.trim();
                if t.is_empty() { None } else { Some(t.to_string()) }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => None,
            Err(_) => None,
        }
    }
}

fn fallback_read_line(prompt: &str) -> io::Result<ReadResult> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    match io::stdin().read_line(&mut line)? {
        0 => Ok(ReadResult::Eof),
        _ => Ok(ReadResult::Line(line.trim_end_matches(['\r', '\n']).to_string())),
    }
}

fn raw_multiline_input(
    prompt: &str,
    prompt_width: usize,
    cwd: &Path,
    history: &[String],
    meter: &str,
    context: &str,
) -> io::Result<ReadResult> {
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    // Bracketed paste: el terminal envía un pegado como UN evento `Event::Paste`
    // en vez de teclas sueltas. Sin esto, los `\n` de un pegado multilínea
    // llegan como Enter y ENVÍAN a media pega (el bug clásico del input).
    execute!(stdout, EnableBracketedPaste)?;
    let cont_prefix = " ".repeat(prompt_width);
    execute!(stdout, SavePosition)?;

    let mut text = String::new();
    // Columna del cursor en CARACTERES. No hay movimiento lateral (←/→ libres),
    // así que el cursor vive siempre al final del texto; `byte_pos` lo traduce.
    let mut cursor_col = 0usize;
    let mut history_idx: Option<usize> = None;
    // Pegados GRANDES: en vez de volcar N líneas al input, se muestran como un
    // placeholder `[⎘ pegado #k · …]` y aquí guardamos (placeholder, contenido
    // real) para expandirlo al enviar. Pares; nunca se reordenan.
    let mut pastes: Vec<(String, String)> = Vec::new();

    render(&mut stdout, prompt, prompt_width, &cont_prefix, &text, cursor_col, context, cwd, meter)?;

    let result = loop {
        let ev = match event::read() {
            Ok(ev) => ev,
            Err(_) => break Ok(ReadResult::Line(text)),
        };

        // ── Pegado detectado por RÁFAGA (funciona sin bracketed paste; el caso
        // de Windows). CLAVE: en Windows cada tecla emite Press Y Release, así que
        // tras un Press SIEMPRE hay un evento en cola (su propio Release) — por
        // eso NO basta con "hay algo en cola". Solo es pegado si recolectamos
        // contenido EXTRA (otra tecla de carácter/Enter, no el Release). Una tecla
        // suelta deja `extra = false` y cae al manejo normal (Enter envía, etc.).
        if burst_char(&ev).is_some() && event::poll(Duration::from_millis(2)).unwrap_or(false) {
            let mut blob = String::new();
            if let Some(c) = burst_char(&ev) {
                blob.push(c);
            }
            let mut extra = false;
            while event::poll(Duration::from_millis(2)).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Key(k)) if k.kind == KeyEventKind::Release => continue,
                    Ok(Event::Paste(d)) => {
                        blob.push_str(&d);
                        extra = true;
                    }
                    Ok(e) => match burst_char(&e) {
                        Some(c) => {
                            blob.push(c);
                            extra = true;
                        }
                        None => break, // tecla no-contenido: fin de la ráfaga
                    },
                    Err(_) => break,
                }
            }
            if extra {
                apply_blob(&mut text, &mut cursor_col, &mut pastes, &blob);
                render(&mut stdout, prompt, prompt_width, &cont_prefix, &text, cursor_col, context, cwd, meter)?;
                continue;
            }
            // Sin contenido extra: era una tecla suelta (solo su Release estaba en
            // cola) → cae al manejo normal de abajo con el evento original.
        }

        match ev {
            Event::Key(key) if key.kind == KeyEventKind::Release => continue,
            Event::Key(key) => match (key.code, key.modifiers) {
                // Enter con CUALQUIER modificador (Shift/Ctrl/Alt) = salto de
                // línea; Enter solo = enviar. Cubre las convenciones de varias
                // terminales (algunas no distinguen Shift+Enter, otras sí
                // Ctrl/Alt+Enter). En Windows el salto puede llegar como CR/LF.
                (KeyCode::Enter, m) if !m.is_empty() => {
                    let pos = byte_pos(&text, cursor_col);
                    text.insert(pos, '\n');
                    cursor_col += 1;
                }
                (KeyCode::Char('\r' | '\n'), m) if !m.is_empty() => {
                    let pos = byte_pos(&text, cursor_col);
                    text.insert(pos, '\n');
                    cursor_col += 1;
                }
                (KeyCode::Enter, _) => break Ok(ReadResult::Line(text)),
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    break Ok(ReadResult::Interrupted);
                }
                (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                    if text.is_empty() {
                        break Ok(ReadResult::Eof);
                    }
                }
                // Tab acepta la sugerencia (ghost) si el cursor está al final.
                (KeyCode::Tab, _) => {
                    if cursor_col >= text.chars().count()
                        && let Some(g) = ghost_suffix(&text, cwd)
                    {
                        text.push_str(&g);
                        cursor_col = text.chars().count();
                    }
                }
                // → al final acepta el ghost; si no, mueve el cursor a la derecha.
                (KeyCode::Right, _) => {
                    let end = text.chars().count();
                    if cursor_col >= end {
                        if let Some(g) = ghost_suffix(&text, cwd) {
                            text.push_str(&g);
                            cursor_col = text.chars().count();
                        }
                    } else {
                        cursor_col += 1;
                    }
                }
                (KeyCode::Left, _) => cursor_col = cursor_col.saturating_sub(1),
                (KeyCode::Home, _) => {
                    // Inicio de la línea actual (tras el último salto antes del cursor).
                    let chars: Vec<char> = text.chars().collect();
                    let mut i = cursor_col.min(chars.len());
                    while i > 0 && chars[i - 1] != '\n' {
                        i -= 1;
                    }
                    cursor_col = i;
                }
                (KeyCode::End, _) => {
                    // Fin de la línea actual (hasta el próximo salto o el final).
                    let chars: Vec<char> = text.chars().collect();
                    let mut i = cursor_col.min(chars.len());
                    while i < chars.len() && chars[i] != '\n' {
                        i += 1;
                    }
                    cursor_col = i;
                }
                // Borrado por PALABRA hacia la izquierda: Ctrl+W (clásico de
                // readline; muchas terminales mapean Ctrl+Backspace/Ctrl+Delete a
                // esto) y Ctrl/Alt+Backspace. Sin estos handlers, Ctrl+W caía en el
                // arm genérico de char e insertaba una 'w' literal (el bug de "wwww").
                (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                    delete_word_back(&mut text, &mut cursor_col);
                }
                (KeyCode::Backspace, m) if m.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                    delete_word_back(&mut text, &mut cursor_col);
                }
                // Borrado por PALABRA hacia la derecha: Ctrl/Alt+Delete.
                (KeyCode::Delete, m) if m.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
                    delete_word_forward(&mut text, &mut cursor_col);
                }
                // Delete simple: borra el carácter a la derecha del cursor.
                (KeyCode::Delete, _) => {
                    if cursor_col < text.chars().count() {
                        let pos = byte_pos(&text, cursor_col);
                        text.remove(pos);
                    }
                }
                // Carácter normal. El guard exige que NO haya exactamente un
                // modificador Ctrl o Alt (eso son atajos: Ctrl+W, Alt+f…). Permite
                // sin modificador, con Shift, y con AltGr (= Ctrl+Alt a la vez, que
                // en teclados español/internacional produce `@ # ~ { } [ ]`).
                (KeyCode::Char(c), m)
                    if !c.is_control()
                        && m.contains(KeyModifiers::CONTROL) == m.contains(KeyModifiers::ALT) =>
                {
                    let pos = byte_pos(&text, cursor_col);
                    text.insert(pos, c);
                    cursor_col += 1;
                }
                (KeyCode::Backspace, _) => {
                    // Con el cursor AL FINAL, si el buffer termina en un placeholder
                    // de pegado se borra ENTERO (átomo). En medio del texto, normal.
                    let at_end = cursor_col >= text.chars().count();
                    let ph_len = at_end
                        .then(|| {
                            pastes
                                .iter()
                                .rev()
                                .find(|(ph, _)| text.ends_with(ph.as_str()))
                                .map(|(ph, _)| ph.len())
                        })
                        .flatten();
                    if let Some(len) = ph_len {
                        text.truncate(text.len() - len);
                        cursor_col = text.chars().count();
                    } else if cursor_col > 0 {
                        let pos = byte_pos(&text, cursor_col - 1);
                        text.remove(pos);
                        cursor_col -= 1;
                    }
                }
                (KeyCode::Up, _) => {
                    if !history.is_empty() {
                        let idx = match history_idx {
                            None => history.len() - 1,
                            Some(0) => 0,
                            Some(i) => i - 1,
                        };
                        text = history[idx].clone();
                        cursor_col = text.chars().count();
                        history_idx = Some(idx);
                    }
                }
                (KeyCode::Down, _) => {
                    if let Some(i) = history_idx {
                        if i + 1 < history.len() {
                            text = history[i + 1].clone();
                            history_idx = Some(i + 1);
                        } else {
                            text.clear();
                            history_idx = None;
                        }
                        cursor_col = text.chars().count();
                    }
                }
                (KeyCode::Esc, _) => break Ok(ReadResult::Interrupted),
                _ => {}
            },
            // Pegado por bracketed paste (terminales que lo soportan): un solo
            // evento con TODO el texto. Mismo tratamiento que la ráfaga.
            Event::Paste(data) => apply_blob(&mut text, &mut cursor_col, &mut pastes, &data),
            _ => {}
        }

        render(&mut stdout, prompt, prompt_width, &cont_prefix, &text, cursor_col, context, cwd, meter)?;
    };

    // Limpieza: en Line dejamos el texto confirmado a la vista (sin ghost ni
    // barra); en Interrupted/Eof borramos todo el área del editor.
    execute!(stdout, DisableBracketedPaste)?;
    match &result {
        Ok(ReadResult::Line(t)) => finalize(&mut stdout, prompt, &cont_prefix, t)?,
        _ => {
            execute!(stdout, RestorePosition, Clear(ClearType::FromCursorDown))?;
        }
    }
    terminal::disable_raw_mode()?;
    println!();
    // Lo que se ENVÍA expande los placeholders a su contenido real; en pantalla
    // quedó el `[⎘ pegado …]` compacto, pero el modelo recibe el texto completo.
    match result {
        Ok(ReadResult::Line(t)) => Ok(ReadResult::Line(expand_pastes(&t, &pastes))),
        other => other,
    }
}

/// ¿Es una tecla de "contenido" (parte de un pegado o tecleo)? Devuelve el
/// carácter: imprimible sin Ctrl/Alt, o `'\n'` para Enter/CR/LF SIN modificador.
/// Las teclas de control (flechas, backspace, Ctrl+C…) devuelven `None`.
fn burst_char(ev: &Event) -> Option<char> {
    let Event::Key(k) = ev else { return None };
    if k.kind == KeyEventKind::Release {
        return None;
    }
    match k.code {
        KeyCode::Char(c)
            if !c.is_control()
                && !k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            Some(c)
        }
        KeyCode::Enter | KeyCode::Char('\r') | KeyCode::Char('\n') if k.modifiers.is_empty() => {
            Some('\n')
        }
        _ => None,
    }
}

/// Inserta un blob (pegado o ráfaga). Grande (multilínea o >200 chars) → chip
/// `[⎘ pegado #k · N líneas · M chars]` (el contenido real se guarda para
/// expandirlo al enviar); chico → texto inline normal.
fn apply_blob(
    text: &mut String,
    cursor_col: &mut usize,
    pastes: &mut Vec<(String, String)>,
    blob: &str,
) {
    if blob.is_empty() {
        return;
    }
    let lines = blob.lines().count().max(1);
    let chars = blob.chars().count();
    // Chip solo si es de verdad GRANDE (varias líneas o muy largo). Un texto de
    // una sola línea —aunque traiga un salto al final— va inline.
    let insert = if lines > 1 || chars > 200 {
        let k = pastes.len() + 1;
        let placeholder = format!("[⎘ pegado #{k} · {lines} líneas · {chars} chars]");
        pastes.push((placeholder.clone(), blob.to_string()));
        placeholder
    } else {
        blob.to_string()
    };
    // Inserta en la posición del cursor (no siempre al final).
    let pos = byte_pos(text, *cursor_col);
    text.insert_str(pos, &insert);
    *cursor_col += insert.chars().count();
}

/// Reemplaza cada placeholder `[⎘ pegado …]` por su contenido real al enviar.
/// Los placeholders que el usuario borró (ya no están en el texto) se ignoran.
fn expand_pastes(text: &str, pastes: &[(String, String)]) -> String {
    let mut out = text.to_string();
    for (placeholder, content) in pastes {
        if out.contains(placeholder.as_str()) {
            out = out.replace(placeholder.as_str(), content);
        }
    }
    out
}

/// Borra la palabra a la IZQUIERDA del cursor: primero los espacios pegados al
/// cursor, luego el bloque de no-espacios. Mueve `cursor_col` al inicio borrado.
fn delete_word_back(text: &mut String, cursor_col: &mut usize) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = (*cursor_col).min(chars.len());
    while i > 0 && chars[i - 1].is_whitespace() {
        i -= 1;
    }
    while i > 0 && !chars[i - 1].is_whitespace() {
        i -= 1;
    }
    if i < *cursor_col {
        let start = byte_pos(text, i);
        let end = byte_pos(text, *cursor_col);
        text.replace_range(start..end, "");
        *cursor_col = i;
    }
}

/// Borra la palabra a la DERECHA del cursor: espacios pegados y luego el bloque
/// de no-espacios. El cursor no se mueve (el texto de la derecha se corre).
fn delete_word_forward(text: &mut String, cursor_col: &mut usize) {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    let mut i = (*cursor_col).min(n);
    while i < n && chars[i].is_whitespace() {
        i += 1;
    }
    while i < n && !chars[i].is_whitespace() {
        i += 1;
    }
    if i > *cursor_col {
        let start = byte_pos(text, *cursor_col);
        let end = byte_pos(text, i);
        text.replace_range(start..end, "");
    }
}

/// Índice en BYTES del carácter en la columna `char_col` (o el final del texto).
fn byte_pos(text: &str, char_col: usize) -> usize {
    text.char_indices()
        .nth(char_col)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

/// Dibuja el prompt + texto resaltado + ghost, y debajo la barra de atajos.
/// Posiciona el cursor en `cursor_col` (su línea y columna reales).
#[allow(clippy::too_many_arguments)] // args cohesivos del render; agruparlos no aclara
fn render(
    stdout: &mut io::Stdout,
    prompt: &str,
    prompt_width: usize,
    cont_prefix: &str,
    text: &str,
    cursor_col: usize,
    context: &str,
    cwd: &Path,
    meter: &str,
) -> io::Result<()> {
    execute!(stdout, RestorePosition, Clear(ClearType::FromCursorDown), Print(prompt))?;

    // `split('\n')` conserva la línea vacía final (si el texto acaba en salto),
    // a diferencia de `lines()` — así el cursor cae en la línea nueva real.
    let lines: Vec<&str> = text.split('\n').collect();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            execute!(stdout, Print("\r\n"), Print(cont_prefix))?;
        }
        execute!(stdout, Print(highlight_line(line, i == 0)))?;
    }

    // Ghost SOLO con el cursor al final (completa el token que se está escribiendo;
    // si el cursor está en medio del texto no tiene sentido sugerir).
    if cursor_col >= text.chars().count()
        && let Some(g) = ghost_suffix(text, cwd)
    {
        execute!(stdout, Print(ui::dim(&g)))?;
    }

    // Barra de atajos: una línea por debajo del input (no en posición absoluta,
    // así el scroll de la respuesta no la empuja como a una barra fija).
    let footer = footer_line(context, meter, lines.len());
    execute!(stdout, Print("\r\n"), Print(&footer))?;

    // Cursor a su (línea, columna) REAL dentro del texto. El footer quedó 1 fila
    // por debajo de la última línea, así que subimos hasta la línea del cursor.
    let (cur_line, cur_col) = cursor_line_col(text, cursor_col);
    let rows_up = (lines.len() - cur_line) as u16;
    execute!(stdout, MoveUp(rows_up), MoveToColumn((prompt_width + cur_col) as u16))?;
    stdout.flush()
}

/// (línea, columna) en CARACTERES donde cae `cursor_col` dentro de `text`.
fn cursor_line_col(text: &str, cursor_col: usize) -> (usize, usize) {
    let (mut line, mut col) = (0usize, 0usize);
    for (i, ch) in text.chars().enumerate() {
        if i >= cursor_col {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Redibujo final (al confirmar con Enter): texto resaltado, sin ghost ni barra.
fn finalize(
    stdout: &mut io::Stdout,
    prompt: &str,
    cont_prefix: &str,
    text: &str,
) -> io::Result<()> {
    execute!(stdout, RestorePosition, Clear(ClearType::FromCursorDown), Print(prompt))?;
    for (i, line) in text.split('\n').enumerate() {
        if i > 0 {
            execute!(stdout, Print("\r\n"), Print(cont_prefix))?;
        }
        execute!(stdout, Print(highlight_line(line, i == 0)))?;
    }
    stdout.flush()
}

/// Sugerencia inline para el token al final del texto: `@archivo` o `/comando`.
/// Devuelve solo el SUFIJO que falta por escribir (lo que se pinta en gris).
fn ghost_suffix(text: &str, cwd: &Path) -> Option<String> {
    let tail = text.rsplit(char::is_whitespace).next().unwrap_or("");
    if let Some(partial) = tail.strip_prefix('@') {
        let best = complete_path(cwd, partial).into_iter().next()?;
        return best
            .strip_prefix(partial)
            .map(str::to_string)
            .filter(|s| !s.is_empty());
    }
    // Comando: el `/token` debe ser lo ÚNICO escrito (primer token, sin espacios).
    if let Some(partial) = text.strip_prefix('/')
        && !partial.contains(char::is_whitespace)
    {
        let cmd = COMMANDS
            .iter()
            .find(|c| c.starts_with(partial) && **c != partial)?;
        return Some(cmd[partial.len()..].to_string());
    }
    None
}

/// Candidatos de ruta (rutas relativas completas) para un parcial tras `@`.
/// Maneja subdirectorios: `src/ma` lista `src/` filtrando por `ma`.
fn complete_path(cwd: &Path, partial: &str) -> Vec<String> {
    let (dir_part, frag) = match partial.rfind('/') {
        Some(i) => (&partial[..=i], &partial[i + 1..]),
        None => ("", partial),
    };
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(cwd.join(dir_part)) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name == "target" {
                continue;
            }
            if !frag.is_empty() && !name.starts_with(frag) {
                continue;
            }
            let slash = if entry.path().is_dir() { "/" } else { "" };
            out.push(format!("{dir_part}{name}{slash}"));
        }
    }
    out.sort();
    out
}

/// Resalta (en color de acento del modo) los `/comandos` iniciales y las
/// `@referencias` de la línea, conservando el espaciado exacto.
fn highlight_line(line: &str, is_first: bool) -> String {
    if line.is_empty() {
        return String::new();
    }
    // Chip de pegado `[⎘ … ]`: se pinta como pastilla (acento del modo) y el
    // resto de la línea se resalta normal alrededor (recursivo, soporta varios).
    if let Some(start) = line.find("[⎘")
        && let Some(rel) = line[start..].find(']')
    {
        let end = start + rel + 1;
        return format!(
            "{}{}{}",
            highlight_line(&line[..start], is_first),
            ui::accent(&line[start..end]),
            highlight_line(&line[end..], false),
        );
    }
    let mut out = String::new();
    let mut rest = line;
    let mut word_idx = 0usize;
    while !rest.is_empty() {
        // Espacios iniciales: se preservan tal cual.
        let ws = rest.find(|c: char| !c.is_whitespace()).unwrap_or(rest.len());
        if ws > 0 {
            out.push_str(&rest[..ws]);
            rest = &rest[ws..];
            if rest.is_empty() {
                break;
            }
        }
        let wlen = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let word = &rest[..wlen];
        let is_cmd = is_first && word_idx == 0 && word.starts_with('/') && word.len() > 1;
        let is_ref = word.starts_with('@') && word.len() > 1;
        if is_cmd || is_ref {
            out.push_str(&ui::accent(word));
        } else {
            out.push_str(word);
        }
        rest = &rest[wlen..];
        word_idx += 1;
    }
    out
}

/// Barra al pie: `focus · modo` (color del modo) + atajos a la izquierda,
/// medidor + nº de líneas a la derecha. Garantiza UNA sola línea: si no cabe,
/// recorta primero los atajos, luego el medidor — el contexto SIEMPRE se ve
/// (es lo que el usuario pidió mostrar ahí). Una sola línea evita que el cálculo
/// del cursor se rompa con un salto inesperado.
fn footer_line(context: &str, meter: &str, line_count: usize) -> String {
    let w = ui::term_width().clamp(24, 120);
    let ctx = context.trim();

    let mut right = meter.to_string();
    if line_count > 1 {
        if !right.is_empty() {
            right.push_str(&ui::dim("  ·  "));
        }
        right.push_str(&ui::dim(&format!("{line_count} líneas")));
    }

    const HINTS_FULL: &str = "Shift+Enter nueva línea · Tab completa · /ayuda";
    const HINTS_MIN: &str = "Tab · /ayuda";

    // Ancho visible de "ctx   hints" (3 espacios de separación si ambos existen).
    let left_w = |hints: &str| match (ctx.is_empty(), hints.is_empty()) {
        (true, true) => 0,
        (true, false) => ui::visible_width(hints),
        (false, true) => ui::visible_width(ctx),
        (false, false) => ui::visible_width(ctx) + 3 + ui::visible_width(hints),
    };
    let fits = |hints: &str, rw: usize| 2 + left_w(hints) + 2 + rw <= w;

    // Recorta: atajos completos → mínimos → ninguno; y si aún no cabe, sin medidor.
    let rw = ui::visible_width(&right);
    let (hints, right) = if fits(HINTS_FULL, rw) {
        (HINTS_FULL, right)
    } else if fits(HINTS_MIN, rw) {
        (HINTS_MIN, right)
    } else if fits("", rw) {
        ("", right)
    } else {
        ("", String::new())
    };

    // Contexto en el acento del modo; atajos en gris.
    let left_display = match (ctx.is_empty(), hints.is_empty()) {
        (true, true) => String::new(),
        (true, false) => ui::dim(hints),
        (false, true) => ui::accent(ctx),
        (false, false) => format!("{}   {}", ui::accent(ctx), ui::dim(hints)),
    };
    let used = 2 + left_w(hints) + ui::visible_width(&right) + 2;
    let gap = w.saturating_sub(used).max(2);
    format!("  {}{}{}", left_display, " ".repeat(gap), right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_pastes_reemplaza_el_placeholder_por_el_contenido_real() {
        let pastes = vec![(
            "[⎘ pegado #1 · 3 líneas · 12 chars]".to_string(),
            "uno\ndos\ntres".to_string(),
        )];
        let enviado = expand_pastes("revisa: [⎘ pegado #1 · 3 líneas · 12 chars]", &pastes);
        assert_eq!(enviado, "revisa: uno\ndos\ntres");
    }

    #[test]
    fn expand_pastes_ignora_un_chip_que_el_usuario_borro() {
        // Si el placeholder ya no está en el texto (se borró con Backspace), no
        // se inyecta nada: el envío queda tal cual.
        let pastes = vec![("[⎘ pegado #1 · 2 líneas · 3 chars]".to_string(), "a\nb".to_string())];
        assert_eq!(expand_pastes("solo texto", &pastes), "solo texto");
    }

    #[test]
    fn highlight_line_conserva_el_contenido_del_chip() {
        let out = highlight_line("[⎘ pegado #1 · 2 líneas · 5 chars]", false);
        assert!(out.contains("pegado #1"), "el chip debe seguir legible, vi: {out}");
    }

    #[test]
    fn cursor_line_col_ubica_dentro_de_texto_multilinea() {
        // "ab\ncd": a0 b1 \n2 c3 d4.
        assert_eq!(cursor_line_col("ab\ncd", 0), (0, 0));
        assert_eq!(cursor_line_col("ab\ncd", 2), (0, 2)); // fin de la línea 0
        assert_eq!(cursor_line_col("ab\ncd", 3), (1, 0)); // inicio de la línea 1
        assert_eq!(cursor_line_col("ab\ncd", 5), (1, 2)); // fin del texto
        assert_eq!(cursor_line_col("ab\ncd", 99), (1, 2)); // más allá → fin
    }

    #[test]
    fn delete_word_back_borra_palabra_y_espacios() {
        // Cursor al final: borra "mundo".
        let mut t = "hola mundo".to_string();
        let mut c = t.chars().count();
        delete_word_back(&mut t, &mut c);
        assert_eq!(t, "hola ");
        assert_eq!(c, 5);
        // Otra vez: borra el espacio sobrante + "hola".
        delete_word_back(&mut t, &mut c);
        assert_eq!(t, "");
        assert_eq!(c, 0);
        // En buffer vacío no truena.
        delete_word_back(&mut t, &mut c);
        assert_eq!(t, "");
    }

    #[test]
    fn delete_word_forward_borra_a_la_derecha_sin_mover_cursor() {
        let mut t = "hola mundo".to_string();
        let mut c = 0usize;
        delete_word_forward(&mut t, &mut c);
        assert_eq!(t, " mundo"); // borró "hola", el cursor sigue en 0
        assert_eq!(c, 0);
        delete_word_forward(&mut t, &mut c);
        assert_eq!(t, ""); // borró " mundo"
    }

    #[test]
    fn apply_blob_grande_hace_chip_y_chico_va_inline() {
        // Multilínea → chip, y al expandir vuelve el contenido real.
        let mut text = String::new();
        let mut col = 0usize;
        let mut pastes = Vec::new();
        apply_blob(&mut text, &mut col, &mut pastes, "a\nb\nc");
        assert!(text.starts_with("[⎘ pegado #1"), "esperaba un chip, vi: {text}");
        assert_eq!(pastes.len(), 1);
        assert_eq!(expand_pastes(&text, &pastes), "a\nb\nc");

        // Texto chico de una línea → inline, sin chip.
        let mut text2 = String::new();
        let mut col2 = 0usize;
        let mut p2 = Vec::new();
        apply_blob(&mut text2, &mut col2, &mut p2, "hola");
        assert_eq!(text2, "hola");
        assert!(p2.is_empty());
    }
}
