//! Editor de entrada: mini TUI con crossterm para multilinea real (Shift+Enter).
//! rustyline se mantiene para confirm_line (single-line) e historial.

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

use crossterm::cursor::{MoveToColumn, RestorePosition, SavePosition};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::style::Print;
use crossterm::terminal::{self, Clear, ClearType};

use rustyline::error::ReadlineError;
use rustyline::{Config, Editor};

use crate::ui;

pub enum ReadResult {
    Line(String),
    Interrupted,
    Eof,
}

pub struct InputEditor {
    cwd: PathBuf,
    history: Vec<String>,
    confirmer: Editor<(), rustyline::history::DefaultHistory>,
}

impl InputEditor {
    pub fn new(cwd: PathBuf) -> Self {
        let config = Config::builder()
            .max_history_size(1000)
            .unwrap()
            .build();
        let confirmer = Editor::with_config(config).expect("no se pudo crear el editor rustyline");
        Self { cwd, history: Vec::new(), confirmer }
    }

    pub fn read_input(&mut self, tag: &str) -> io::Result<ReadResult> {
        if !io::stdin().is_terminal() {
            return fallback_read_line("> ");
        }
        println!();
        let prompt = format!("  {} {}", ui::dim(tag), ui::accent("▸"));
        let result = raw_multiline_input(&prompt, &self.cwd, &self.history);
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
    cwd: &Path,
    history: &[String],
) -> io::Result<ReadResult> {
    let mut stdout = io::stdout();
    terminal::enable_raw_mode()?;
    execute!(stdout, SavePosition, Clear(ClearType::CurrentLine), Print(prompt))?;
    stdout.flush()?;

    let mut text = String::new();
    let mut cursor_col = 0usize;
    let mut history_idx: Option<usize> = None;
    let mut pending_tab: Option<usize> = None;
    let mut tab_matches: Vec<String> = Vec::new();

    let prompt_width = visible_width(prompt);
    let cont_prefix = " ".repeat(prompt_width);
    let lines_before = text.lines().count().max(1);

    let result = loop {
        let ev = match event::read() {
            Ok(ev) => ev,
            Err(_) => break Ok(ReadResult::Line(text)),
        };

        match ev {
            Event::Key(key) if key.kind == KeyEventKind::Release => continue,
            Event::Key(key) => match (key.code, key.modifiers) {
                (KeyCode::Enter, KeyModifiers::SHIFT) => {
                    text.insert(cursor_column(&text, cursor_col), '\n');
                    cursor_col += 1;
                    pending_tab = None;
                }
                (KeyCode::Char('\r'), KeyModifiers::SHIFT) => {
                    // Windows: Shift+Enter puede venir como Char('\r')+SHIFT
                    text.insert(cursor_column(&text, cursor_col), '\n');
                    cursor_col += 1;
                    pending_tab = None;
                }
                (KeyCode::Enter, _) => {
                    break Ok(ReadResult::Line(text));
                }
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    break Ok(ReadResult::Interrupted);
                }
                (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                    if text.is_empty() {
                        break Ok(ReadResult::Eof);
                    }
                }
                (KeyCode::Char(c), _) => {
                    let pos = cursor_column(&text, cursor_col);
                    text.insert(pos, c);
                    cursor_col += 1;
                    pending_tab = None;
                }
                (KeyCode::Backspace, _) => {
                    if cursor_col == 0 {
                        continue;
                    }
                    let pos = cursor_column(&text, cursor_col);
                    if pos > 0 {
                        cursor_col -= 1;
                        text.remove(pos - 1);
                    }
                    pending_tab = None;
                }
                (KeyCode::Tab, _) => {
                    pending_tab = try_tab_complete(
                        &text,
                        cursor_col,
                        cwd,
                        &mut tab_matches,
                        pending_tab,
                    );
                    if let Some(idx) = pending_tab
                        && let Some(replacement) = tab_matches.get(idx)
                    {
                        let replaced = replace_at_reference(&text, cursor_col, replacement);
                        text = replaced;
                        cursor_col = text.len();
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
                        cursor_col = text.len();
                        history_idx = Some(idx);
                        pending_tab = None;
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
                        cursor_col = text.len();
                        pending_tab = None;
                    }
                }
                (KeyCode::Esc, _) => {
                    break Ok(ReadResult::Interrupted);
                }
                _ => {}
            },
            _ => {}
        }

        render_multiline(&mut stdout, prompt, &cont_prefix, &text, lines_before)?;
    };

    terminal::disable_raw_mode()?;
    println!();
    result
}

fn cursor_column(text: &str, col: usize) -> usize {
    for (chars, (_, ch)) in text.char_indices().enumerate() {
        if chars >= col {
            return ch as usize;
        }
    }
    text.len()
}

fn visible_width(s: &str) -> usize {
    s.chars().count()
}

fn render_multiline(
    stdout: &mut io::Stdout,
    prompt: &str,
    cont_prefix: &str,
    text: &str,
    _prev_lines: usize,
) -> io::Result<()> {
    execute!(
        stdout,
        RestorePosition,
        Clear(ClearType::FromCursorDown),
        Print(prompt),
    )?;

    let lines: Vec<&str> = if text.is_empty() {
        vec![""]
    } else {
        text.lines().collect()
    };

    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            execute!(stdout, Print("\r\n"), Print(cont_prefix))?;
        }
        execute!(stdout, Print(line))?;
    }

    let cursor_line_width = if let Some(last) = text.lines().last() {
        visible_width(last)
    } else {
        0
    };

    if cursor_line_width == 0 {
        execute!(stdout, MoveToColumn(prompt.len() as u16))?;
    }

    stdout.flush()
}

fn try_tab_complete(
    text: &str,
    cursor_col: usize,
    cwd: &Path,
    matches: &mut Vec<String>,
    pending: Option<usize>,
) -> Option<usize> {
    let pos = cursor_column(text, cursor_col);
    let head = &text[..pos.min(text.len())];

    if let Some(idx) = head.rfind(|c: char| c.is_whitespace()) {
        let word = &head[idx + 1..];
        if let Some(partial) = word.strip_prefix('@') {
            if pending.is_none() {
                *matches = find_path_matches(cwd, partial);
            }
            if matches.is_empty() {
                return None;
            }
            let next = match pending {
                None => 0,
                Some(i) if i + 1 < matches.len() => i + 1,
                Some(_) => 0,
            };
            return Some(next);
        }
    }
    None
}

fn find_path_matches(cwd: &Path, partial: &str) -> Vec<String> {
    let search_path = cwd.join(partial);
    let dir = search_path.parent().unwrap_or(cwd);
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return vec![],
    };

    let mut pairs: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || name == "target" {
            continue;
        }
        if !name.starts_with(partial) && !partial.is_empty() {
            continue;
        }
        let suffix = if entry.path().is_dir() { "/" } else { "" };
        pairs.push(format!("{name}{suffix}"));
    }
    pairs.sort();
    pairs
}

fn replace_at_reference(text: &str, cursor_col: usize, replacement: &str) -> String {
    let pos = cursor_column(text, cursor_col);
    let head = &text[..pos.min(text.len())];
    if let Some(idx) = head.rfind(|c: char| c.is_whitespace()) {
        let word_start = idx + 1;
        let mut result = text[..word_start].to_string();
        result.push_str(replacement);
        if pos < text.len() {
            result.push_str(&text[pos..]);
        }
        return result;
    }
    text.to_string()
}
