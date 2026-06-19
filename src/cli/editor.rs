//! Editor de entrada: mini TUI con crossterm. Da multilínea real (Shift+Enter),
//! autocompletado inline en gris (ghost: `@archivo` y `/comando`), resaltado de
//! comandos y referencias mientras escribes, y una barra de atajos al pie con
//! el medidor de contexto. rustyline solo se mantiene para `confirm_line`.

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use crossterm::cursor::{MoveToColumn, MoveUp, RestorePosition, SavePosition};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
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
        let arrow = ui::mode_arrow();
        let prompt = if self.context.is_empty() {
            format!("  {arrow} ")
        } else {
            format!("  {}  {arrow} ", ui::dim(&self.context))
        };
        let prompt_width = ui::visible_width(&prompt);
        let result =
            raw_multiline_input(&prompt, prompt_width, &self.cwd, &self.history, &self.meter);
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
) -> io::Result<ReadResult> {
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    let cont_prefix = " ".repeat(prompt_width);
    execute!(stdout, SavePosition)?;

    let mut text = String::new();
    // Columna del cursor en CARACTERES. No hay movimiento lateral (←/→ libres),
    // así que el cursor vive siempre al final del texto; `byte_pos` lo traduce.
    let mut cursor_col = 0usize;
    let mut history_idx: Option<usize> = None;

    render(&mut stdout, prompt, prompt_width, &cont_prefix, &text, cwd, meter)?;

    let result = loop {
        let ev = match event::read() {
            Ok(ev) => ev,
            Err(_) => break Ok(ReadResult::Line(text)),
        };

        match ev {
            Event::Key(key) if key.kind == KeyEventKind::Release => continue,
            Event::Key(key) => match (key.code, key.modifiers) {
                // Shift+Enter inserta salto de línea (en Windows puede llegar
                // como Char('\r')+SHIFT).
                (KeyCode::Enter, KeyModifiers::SHIFT)
                | (KeyCode::Char('\r'), KeyModifiers::SHIFT) => {
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
                // Tab y → aceptan la sugerencia inline (ghost), si la hay.
                (KeyCode::Tab, _) | (KeyCode::Right, _) => {
                    if let Some(g) = ghost_suffix(&text, cwd) {
                        text.push_str(&g);
                        cursor_col = text.chars().count();
                    }
                }
                (KeyCode::Char(c), _) => {
                    let pos = byte_pos(&text, cursor_col);
                    text.insert(pos, c);
                    cursor_col += 1;
                }
                (KeyCode::Backspace, _) => {
                    if cursor_col > 0 {
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
            _ => {}
        }

        render(&mut stdout, prompt, prompt_width, &cont_prefix, &text, cwd, meter)?;
    };

    // Limpieza: en Line dejamos el texto confirmado a la vista (sin ghost ni
    // barra); en Interrupted/Eof borramos todo el área del editor.
    match &result {
        Ok(ReadResult::Line(t)) => finalize(&mut stdout, prompt, &cont_prefix, t)?,
        _ => {
            execute!(stdout, RestorePosition, Clear(ClearType::FromCursorDown))?;
        }
    }
    terminal::disable_raw_mode()?;
    println!();
    result
}

/// Índice en BYTES del carácter en la columna `char_col` (o el final del texto).
fn byte_pos(text: &str, char_col: usize) -> usize {
    text.char_indices()
        .nth(char_col)
        .map(|(b, _)| b)
        .unwrap_or(text.len())
}

/// Dibuja el prompt + texto resaltado + ghost, y debajo la barra de atajos.
/// Deja el cursor justo al final del texto escrito (antes del ghost).
fn render(
    stdout: &mut io::Stdout,
    prompt: &str,
    prompt_width: usize,
    cont_prefix: &str,
    text: &str,
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

    // Sugerencia inline en gris a la derecha del cursor.
    if let Some(g) = ghost_suffix(text, cwd) {
        execute!(stdout, Print(ui::dim(&g)))?;
    }

    // Barra de atajos: una línea por debajo del input (no en posición absoluta,
    // así el scroll de la respuesta no la empuja como a una barra fija).
    let footer = footer_line(meter, lines.len());
    execute!(stdout, Print("\r\n"), Print(&footer))?;

    // Reposiciona el cursor: sube a la última línea del input y a su columna.
    let last = lines.last().copied().unwrap_or("");
    let typed_col = (prompt_width + ui::visible_width(last)) as u16;
    execute!(stdout, MoveUp(1), MoveToColumn(typed_col))?;
    stdout.flush()
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

/// Barra de atajos al pie: atajos a la izquierda, medidor + nº de líneas a la
/// derecha. Garantiza UNA sola línea (si no cabe, recorta de derecha a izquierda)
/// para no romper el cálculo del cursor con un salto de línea inesperado.
fn footer_line(meter: &str, line_count: usize) -> String {
    let w = ui::term_width().clamp(24, 120);

    let mut right = meter.to_string();
    if line_count > 1 {
        if !right.is_empty() {
            right.push_str(&ui::dim("  ·  "));
        }
        right.push_str(&ui::dim(&format!("{line_count} líneas")));
    }

    let left_full = "Shift+Enter nueva línea · Tab completa · /ayuda";
    let left_min = "Tab completa · /ayuda";
    let fits = |l: &str, r: &str| 2 + ui::visible_width(l) + 2 + ui::visible_width(r) <= w;

    // Recorta de derecha a izquierda hasta que quepa en una línea.
    if !fits(left_full, &right) {
        right = if line_count > 1 {
            ui::dim(&format!("{line_count} líneas"))
        } else {
            String::new()
        };
        if !fits(left_full, &right) {
            right = String::new();
        }
    }
    let left = if fits(left_full, &right) { left_full } else { left_min };

    let used = 2 + ui::visible_width(left) + ui::visible_width(&right) + 2;
    let gap = w.saturating_sub(used).max(2);
    format!("  {}{}{}", ui::dim(left), " ".repeat(gap), right)
}
