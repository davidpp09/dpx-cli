//! Editor de entrada con rustyline: input multilinea, historial, autocompletado
//! con Tab para comandos `/...` y referencias `@archivo`, y confirmaciones de una linea.

use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Config, Editor, Helper, Context};

use crate::ui;

pub const COMMANDS: &[&str] = &[
    "/ayuda", "/estado", "/costo", "/presupuesto", "/modelos", "/limpiar",
    "/compactar", "/contexto", "/enfoque", "/modo", "/progreso", "/temario", "/examen",
    "/cerebro", "/comite", "/auto", "/actualizar", "/salir",
];

pub enum ReadResult {
    Line(String),
    Interrupted,
    Eof,
}

pub struct InputEditor {
    editor: Editor<DpxHelper, rustyline::history::DefaultHistory>,
    history: Vec<String>,
}

struct DpxHelper {
    cwd: PathBuf,
}

impl Completer for DpxHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let head = &line[..pos.min(line.len())];

        if head.starts_with('/') && !head.contains(' ') && !head.contains('\n') {
            let pairs: Vec<Pair> = COMMANDS
                .iter()
                .filter(|c| c.starts_with(head))
                .map(|c| Pair { display: c.to_string(), replacement: c.to_string() })
                .collect();
            if !pairs.is_empty() {
                return Ok((0, pairs));
            }
        }

        if let Some(idx) = head.rfind(|c: char| c.is_whitespace()) {
            let start = idx + 1;
            let word = &head[start..];
            if let Some(partial) = word.strip_prefix('@') {
                let search_path = self.cwd.join(partial);
                let dir = search_path.parent().unwrap_or(&self.cwd);

                if let Ok(entries) = std::fs::read_dir(dir) {
                    let mut pairs = Vec::new();
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.starts_with('.') || name == "target" {
                            continue;
                        }
                        let is_dir = entry.path().is_dir();
                        let (display, replacement) = if is_dir {
                            (format!("{name}/"), format!("@{partial}/{name}/"))
                        } else {
                            (name.clone(), format!("@{partial}{name}"))
                        };
                        pairs.push(Pair { display, replacement });
                    }
                    pairs.sort_by(|a, b| a.replacement.cmp(&b.replacement));
                    if !pairs.is_empty() {
                        return Ok((start, pairs));
                    }
                }
            }
        }

        Ok((0, vec![]))
    }
}

impl Hinter for DpxHelper {
    type Hint = String;
    fn hint(&self, _line: &str, _pos: usize, _ctx: &Context<'_>) -> Option<String> {
        None
    }
}

impl Highlighter for DpxHelper {
    fn highlight<'l>(&self, line: &'l str, _pos: usize) -> std::borrow::Cow<'l, str> {
        std::borrow::Cow::Borrowed(line)
    }
    fn highlight_prompt<'b, 's: 'b, 'p: 'b>(
        &'s self,
        prompt: &'p str,
        _default: bool,
    ) -> std::borrow::Cow<'b, str> {
        std::borrow::Cow::Borrowed(prompt)
    }
    fn highlight_hint<'h>(&self, hint: &'h str) -> std::borrow::Cow<'h, str> {
        std::borrow::Cow::Borrowed(hint)
    }
    fn highlight_candidate<'c>(
        &self,
        candidate: &'c str,
        _completion: rustyline::CompletionType,
    ) -> std::borrow::Cow<'c, str> {
        std::borrow::Cow::Borrowed(candidate)
    }
    fn highlight_char(&self, _line: &str, _pos: usize, _kind: rustyline::highlight::CmdKind) -> bool {
        false
    }
}

impl Validator for DpxHelper {}

impl Helper for DpxHelper {}

impl InputEditor {
    pub fn new(cwd: PathBuf) -> Self {
        let config = Config::builder()
            .max_history_size(1000)
            .unwrap()
            .build();
        let mut editor = Editor::with_config(config).expect("no se pudo crear el editor rustyline");
        editor.set_helper(Some(DpxHelper { cwd }));
        Self { editor, history: Vec::new() }
    }

    pub fn read_input(&mut self, tag: &str) -> io::Result<ReadResult> {
        if !io::stdin().is_terminal() {
            return fallback_read_line("> ");
        }
        println!();
        let prompt = format!("  {} {}", ui::dim(tag), ui::accent("▸"));
        let cont = "     ";
        let mut lines = String::new();
        let mut first = true;

        loop {
            let p = if first {
                prompt.as_str()
            } else {
                cont
            };
            match self.editor.readline(p) {
                Ok(line) => {
                    if first {
                        first = false;
                        lines = line;
                    } else {
                        lines.push('\n');
                        lines.push_str(&line);
                    }
                    if lines.ends_with('\\') {
                        lines.pop();
                        continue;
                    }
                    let trimmed = lines.trim();
                    if !trimmed.is_empty() {
                        if self.history.last().map(String::as_str) != Some(trimmed) {
                            self.history.push(trimmed.to_string());
                        }
                        self.editor.add_history_entry(trimmed).ok();
                    }
                    return Ok(ReadResult::Line(lines));
                }
                Err(ReadlineError::Interrupted) => {
                    return Ok(ReadResult::Interrupted);
                }
                Err(ReadlineError::Eof) => {
                    if lines.is_empty() {
                        return Ok(ReadResult::Eof);
                    }
                    let trimmed = lines.trim();
                    if !trimmed.is_empty() {
                        if self.history.last().map(String::as_str) != Some(trimmed) {
                            self.history.push(trimmed.to_string());
                        }
                        self.editor.add_history_entry(trimmed).ok();
                    }
                    return Ok(ReadResult::Line(lines));
                }
                Err(e) => {
                    return Err(io::Error::new(io::ErrorKind::Other, e));
                }
            }
        }
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
        match self.editor.readline("") {
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
