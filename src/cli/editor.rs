//! Editor de entrada propio sobre crossterm (modo raw), estilo Claude Code:
//! input multilínea que crece hacia abajo con la barra de estado SIEMPRE viva
//! debajo (regla / input / regla / barra), Shift+Enter para saltos de línea,
//! autocompletado con Tab, resaltado en vivo e historial con ↑/↓.
//!
//! Reemplaza a rustyline: al leer los eventos de teclado nosotros mismos,
//! crossterm en Windows recibe los `INPUT_RECORD`s nativos con modificadores
//! (Shift+Enter llega distinguible SIN el hack del win32-input-mode) y un
//! pegado se detecta como ráfaga de eventos en nuestro propio loop (muere el
//! FFI manual a `PeekConsoleInputW`).
//!
//! Limitación heredada: el ancho se mide en `chars` (un carácter = una
//! columna); los caracteres anchos (CJK, algunos emoji) descuadran el cursor,
//! igual que con el cálculo anterior.

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use termimad::crossterm::{
    cursor::{MoveDown, MoveToColumn, MoveUp},
    event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    queue,
    style::Print,
    terminal::{self, Clear, ClearType},
};

use crate::focus;
use crate::ui;

/// Comandos disponibles, para el autocompletado con Tab.
pub const COMMANDS: &[&str] = &[
    "/help", "/status", "/cost", "/budget", "/models", "/clear", "/compact", "/context", "/focus",
    "/mode", "/brain", "/mentor", "/code", "/auto", "/update", "/salir",
];

const PROMPT: &str = " › ";
const CONT: &str = "   ";
/// Un pegado con más líneas que esto se colapsa a un placeholder editable.
const PASTE_INLINE_MAX_LINES: usize = 3;
const PASTE_INLINE_MAX_CHARS: usize = 200;
/// Filas visibles del menú de autocompletado.
const MENU_ROWS: usize = 6;

/// Resultado de leer una entrada (equivalente a lo que daba rustyline).
pub enum ReadResult {
    Line(String),
    /// Ctrl-C en el prompt: se descarta la entrada, la sesión sigue.
    Interrupted,
    /// Ctrl-D con el buffer vacío: salida limpia.
    Eof,
}

/// Editor de entrada del REPL. Vive toda la sesión (conserva el historial ↑/↓).
pub struct InputEditor {
    cwd: PathBuf,
    history: Vec<String>,
}

impl InputEditor {
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd, history: Vec::new() }
    }

    /// Lee una entrada completa con el área estilo Claude Code. `bar` es la
    /// línea de estado que se mantiene pintada debajo del input.
    pub fn read_input(&mut self, bar: &str) -> io::Result<ReadResult> {
        // Sin TTY (stdin pipeado, tests) no hay modo raw: línea a línea plana.
        if !io::stdin().is_terminal() {
            return fallback_read_line(PROMPT);
        }
        let (result, submitted) = {
            let mut prompt = Prompt::new(&self.cwd, bar, &self.history);
            let result = prompt.run();
            (result, std::mem::take(&mut prompt.submitted_raw))
        };
        // Al historial va la versión SIN expandir (con placeholders), como antes.
        let entry = submitted.trim();
        if matches!(&result, Ok(ReadResult::Line(_)))
            && !entry.is_empty()
            && self.history.last().map(String::as_str) != Some(entry)
        {
            self.history.push(entry.to_string());
        }
        result
    }

    /// Prompt simple de UNA línea para confirmaciones (`[s/N]`, `[s/N/a]`).
    /// Devuelve `None` si el usuario cancela (Ctrl-C / Esc / Ctrl-D).
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
        confirm_line_raw(prompt).unwrap_or(None)
    }
}

/// Lectura plana por stdin cuando no hay terminal (pipes, tests).
fn fallback_read_line(prompt: &str) -> io::Result<ReadResult> {
    print!("{prompt}");
    io::stdout().flush()?;
    let mut line = String::new();
    match io::stdin().read_line(&mut line)? {
        0 => Ok(ReadResult::Eof),
        _ => Ok(ReadResult::Line(line.trim_end_matches(['\r', '\n']).to_string())),
    }
}

/// Lector raw de una línea, con eco, para confirmaciones cortas.
fn confirm_line_raw(prompt: &str) -> io::Result<Option<String>> {
    let _raw = RawGuard::enable()?;
    let mut out = io::stdout();
    queue!(out, Print(prompt))?;
    out.flush()?;
    let mut buf = String::new();
    loop {
        if let Event::Key(k) = event::read()? {
            if k.kind == KeyEventKind::Release {
                continue;
            }
            match (k.code, k.modifiers) {
                (KeyCode::Char('c' | 'd'), m) if m == KeyModifiers::CONTROL => {
                    queue!(out, Print("\r\n"))?;
                    out.flush()?;
                    return Ok(None);
                }
                (KeyCode::Esc, _) => {
                    queue!(out, Print("\r\n"))?;
                    out.flush()?;
                    return Ok(None);
                }
                (KeyCode::Enter, _) => {
                    queue!(out, Print("\r\n"))?;
                    out.flush()?;
                    return Ok(Some(buf));
                }
                (KeyCode::Backspace, _) => {
                    if buf.pop().is_some() {
                        queue!(out, Print("\x08 \x08"))?;
                        out.flush()?;
                    }
                }
                (KeyCode::Char(c), m) if m.difference(KeyModifiers::SHIFT).is_empty() => {
                    buf.push(c);
                    queue!(out, Print(c))?;
                    out.flush()?;
                }
                _ => {}
            }
        }
    }
}

/// Activa el modo raw y lo restaura al soltar (también ante un panic).
struct RawGuard;

impl RawGuard {
    fn enable() -> io::Result<Self> {
        terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

impl Drop for RawGuard {
    fn drop(&mut self) {
        let _ = terminal::disable_raw_mode();
    }
}

/// Un pegado capturado, colapsado a `[pegado #N: …]` en el input; al enviar se
/// expande a su contenido real.
struct PendingPaste {
    placeholder: String,
    content: String,
}

/// Candidato de autocompletado: lo que se muestra y lo que se inserta.
struct Cand {
    display: String,
    replacement: String,
}

/// Menú de autocompletado abierto: candidatos y la palabra que reemplazan.
struct Menu {
    cands: Vec<Cand>,
    /// `None` hasta que el usuario cicla con Tab (la apertura solo inserta el
    /// prefijo común); después, índice del candidato aplicado.
    idx: Option<usize>,
    word_start: usize,
    word_end: usize,
}

impl Menu {
    fn render(&self, width: usize) -> Vec<String> {
        let sel = self.idx.unwrap_or(usize::MAX);
        let page = if sel == usize::MAX { 0 } else { sel / MENU_ROWS };
        let start = page * MENU_ROWS;
        let visible = &self.cands[start..(start + MENU_ROWS).min(self.cands.len())];
        let mut lines: Vec<String> = visible
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let display: String = c.display.chars().take(width.saturating_sub(4)).collect();
                if start + i == sel {
                    ui::accent(&format!("  ▸ {display}"))
                } else {
                    format!("    {display}")
                }
            })
            .collect();
        if self.cands.len() > start + visible.len() {
            lines.push(ui::dim(&format!(
                "    … y {} más (Tab para ciclar)",
                self.cands.len() - start - visible.len()
            )));
        }
        lines
    }
}

/// Estado de una lectura en curso: buffer, cursor, pegados, menú e historial.
struct Prompt<'a> {
    cwd: &'a Path,
    bar: &'a str,
    history: &'a [String],
    buffer: String,
    /// Posición del cursor como offset en BYTES dentro de `buffer`.
    cursor: usize,
    pastes: Vec<PendingPaste>,
    menu: Option<Menu>,
    /// Navegación de historial: índice actual y el borrador guardado.
    hist_idx: Option<usize>,
    draft: String,
    /// Fila de la región (0 = regla superior) donde quedó el cursor real.
    cur_region_row: usize,
    /// Total de filas pintadas la última vez.
    region_rows: usize,
    /// El buffer tal cual se envió (con placeholders), para el historial.
    submitted_raw: String,
}

impl<'a> Prompt<'a> {
    fn new(cwd: &'a Path, bar: &'a str, history: &'a [String]) -> Self {
        Self {
            cwd,
            bar,
            history,
            buffer: String::new(),
            cursor: 0,
            pastes: Vec::new(),
            menu: None,
            hist_idx: None,
            draft: String::new(),
            cur_region_row: 0,
            region_rows: 0,
            submitted_raw: String::new(),
        }
    }

    fn run(&mut self) -> io::Result<ReadResult> {
        let mut out = io::stdout();
        // Línea en blanco de separación con la salida anterior, como antes.
        print!("\r\n");
        out.flush()?;
        let _raw = RawGuard::enable()?;
        self.paint(&mut out)?;
        loop {
            let ev = event::read()?;
            match ev {
                Event::Key(k) => {
                    if k.kind == KeyEventKind::Release {
                        continue;
                    }
                    if let Some(result) = self.handle_key(k, &mut out)? {
                        return Ok(result);
                    }
                }
                // Bracketed paste (Unix): llega ya integrado.
                Event::Paste(text) => self.insert_paste(&text),
                Event::Resize(..) => {}
                _ => continue,
            }
            self.paint(&mut out)?;
        }
    }

    /// Atiende una tecla. `Some(_)` = la lectura terminó.
    fn handle_key(&mut self, k: KeyEvent, out: &mut io::Stdout) -> io::Result<Option<ReadResult>> {
        // ¿Ráfaga de pegado? Si justo detrás de esta tecla hay más eventos en
        // cola (imposible tecleando a mano), se drenan y se tratan como pegado.
        // El Enter espera unos ms extra: si es el primer salto de línea de un
        // pegado cuyo siguiente tramo aún no llegó, tratarlo como submit
        // enviaría media línea al modelo (los chars sueltos solo se insertan).
        let burst_pending = match k.code {
            KeyCode::Enter if k.modifiers.is_empty() => event::poll(Duration::from_millis(10))?,
            KeyCode::Char(_) | KeyCode::Tab => event::poll(Duration::ZERO)?,
            _ => false,
        };
        if burst_pending {
            let first = match k.code {
                KeyCode::Char(c) => c,
                KeyCode::Enter => '\n',
                _ => '\t',
            };
            let burst = collect_burst(first)?;
            if burst.chars().count() > 1 {
                self.menu = None;
                self.insert_paste(&burst);
                return Ok(None);
            }
            // Falsa alarma (p.ej. un key-release encolado): sigue como tecla normal.
        }

        match (k.code, k.modifiers) {
            // --- terminación ---
            (KeyCode::Enter, m) if m.is_empty() => {
                if self.menu.take().is_some() {
                    return Ok(None); // Enter con menú abierto: solo acepta/cierra
                }
                // Compatibilidad: línea terminada en '\' = seguir en la siguiente.
                if self.buffer.ends_with('\\') {
                    self.insert("\n");
                    return Ok(None);
                }
                self.finish(out)?;
                self.submitted_raw = self.buffer.clone();
                let line = expand_pastes(self.buffer.clone(), &self.pastes);
                return Ok(Some(ReadResult::Line(line)));
            }
            (KeyCode::Enter, _) => self.insert("\n"), // Shift/Ctrl/Alt+Enter
            (KeyCode::Char('j'), m) if m == KeyModifiers::CONTROL => self.insert("\n"),
            (KeyCode::Char('c'), m) if m == KeyModifiers::CONTROL => {
                self.finish(out)?;
                return Ok(Some(ReadResult::Interrupted));
            }
            (KeyCode::Char('d'), m) if m == KeyModifiers::CONTROL => {
                if self.buffer.is_empty() {
                    self.finish(out)?;
                    return Ok(Some(ReadResult::Eof));
                }
                self.delete_forward();
            }
            // --- autocompletado ---
            (KeyCode::Tab, m) if m.is_empty() => self.tab(),
            (KeyCode::BackTab, _) => self.cycle_menu(-1),
            (KeyCode::Esc, _) => {
                self.menu = None;
            }
            // --- edición ---
            (KeyCode::Backspace, m) if m.contains(KeyModifiers::CONTROL) => self.delete_word_back(),
            (KeyCode::Char('w'), m) if m == KeyModifiers::CONTROL => self.delete_word_back(),
            (KeyCode::Char('u'), m) if m == KeyModifiers::CONTROL => {
                self.buffer.replace_range(..self.cursor, "");
                self.cursor = 0;
                self.menu = None;
            }
            (KeyCode::Backspace, _) => self.delete_back(),
            (KeyCode::Delete, _) => self.delete_forward(),
            // --- movimiento ---
            (KeyCode::Left, m) if m.contains(KeyModifiers::CONTROL) => self.word_left(),
            (KeyCode::Right, m) if m.contains(KeyModifiers::CONTROL) => self.word_right(),
            (KeyCode::Left, _) => self.move_left(),
            (KeyCode::Right, _) => self.move_right(),
            (KeyCode::Home, _) => self.row_home(),
            (KeyCode::Char('a'), m) if m == KeyModifiers::CONTROL => self.row_home(),
            (KeyCode::End, _) => self.row_end(),
            (KeyCode::Char('e'), m) if m == KeyModifiers::CONTROL => self.row_end(),
            (KeyCode::Up, _) => self.up(),
            (KeyCode::Down, _) => self.down(),
            // --- texto ---
            (KeyCode::Char(c), m) if m.difference(KeyModifiers::SHIFT).is_empty() => {
                self.menu = None;
                self.insert(&c.to_string());
            }
            _ => {}
        }
        Ok(None)
    }

    // ----- render -----

    /// Repinta la región completa (regla / input / regla / barra / menú) y
    /// deja el cursor real en su posición dentro del input.
    fn paint(&mut self, out: &mut io::Stdout) -> io::Result<()> {
        let width = ui::real_term_width();
        let content_w = content_width(width);
        let rows = wrap_rows(&self.buffer, content_w);
        let (crow, ccol) = pos_to_rowcol(&self.buffer, &rows, self.cursor);

        // La región NUNCA puede superar el alto de la pantalla: si la parte de
        // arriba se va al scrollback, el MoveUp del repintado ya no la alcanza
        // y toda la aritmética de filas se rompe. Con un input más alto que la
        // ventana se muestra solo un tramo alrededor del cursor.
        let max_input_rows = term_height().saturating_sub(7).max(3);
        let (start, end) = if rows.len() <= max_input_rows {
            (0, rows.len())
        } else {
            let start = crow.saturating_sub(max_input_rows - 1).min(rows.len() - max_input_rows);
            (start, start + max_input_rows)
        };

        let is_cmd = self.buffer.starts_with('/') && !self.buffer.contains('\n');
        let mut lines: Vec<String> = Vec::with_capacity(end - start + 5);
        lines.push(ui::rule());
        if start > 0 {
            lines.push(ui::dim(&format!("   ↑ {start} filas más")));
        }
        for (i, r) in rows[start..end].iter().enumerate() {
            let prefix = if start + i == 0 { PROMPT } else { CONT };
            lines.push(format!("{prefix}{}", colorize(&r.text, is_cmd)));
        }
        if end < rows.len() {
            lines.push(ui::dim(&format!("   ↓ {} filas más", rows.len() - end)));
        }
        lines.push(ui::rule());
        lines.push(self.bar.to_string());
        if let Some(menu) = &self.menu {
            lines.extend(menu.render(width));
        }

        // +1 por la regla superior, +1 si hay indicador de filas ocultas arriba.
        let target_row = 1 + usize::from(start > 0) + (crow - start);
        queue!(out, MoveToColumn(0))?;
        if self.cur_region_row > 0 {
            queue!(out, MoveUp(self.cur_region_row as u16))?;
        }
        queue!(out, Clear(ClearType::FromCursorDown))?;
        for (i, l) in lines.iter().enumerate() {
            queue!(out, Print(l))?;
            if i + 1 < lines.len() {
                queue!(out, Print("\r\n"))?;
            }
        }
        let up = lines.len() - 1 - target_row;
        if up > 0 {
            queue!(out, MoveUp(up as u16))?;
        }
        queue!(out, MoveToColumn((PROMPT.chars().count() + ccol) as u16))?;
        out.flush()?;
        self.cur_region_row = target_row;
        self.region_rows = lines.len();
        Ok(())
    }

    /// Pintado final (sin menú) y cursor debajo de la región, listo para que
    /// el REPL siga imprimiendo normal.
    fn finish(&mut self, out: &mut io::Stdout) -> io::Result<()> {
        self.menu = None;
        self.paint(out)?;
        let below = self.region_rows - 1 - self.cur_region_row;
        if below > 0 {
            queue!(out, MoveDown(below as u16))?;
        }
        queue!(out, MoveToColumn(0), Print("\r\n"))?;
        out.flush()
    }

    // ----- edición básica -----

    fn insert(&mut self, s: &str) {
        self.buffer.insert_str(self.cursor, s);
        self.cursor += s.len();
        self.hist_idx = None;
    }

    fn delete_back(&mut self) {
        self.menu = None;
        if let Some((i, _)) = self.buffer[..self.cursor].char_indices().next_back() {
            self.buffer.replace_range(i..self.cursor, "");
            self.cursor = i;
        }
    }

    fn delete_forward(&mut self) {
        self.menu = None;
        if let Some(c) = self.buffer[self.cursor..].chars().next() {
            let end = self.cursor + c.len_utf8();
            self.buffer.replace_range(self.cursor..end, "");
        }
    }

    fn delete_word_back(&mut self) {
        self.menu = None;
        let start = word_start_before(&self.buffer, self.cursor);
        self.buffer.replace_range(start..self.cursor, "");
        self.cursor = start;
    }

    fn move_left(&mut self) {
        self.menu = None;
        if let Some((i, _)) = self.buffer[..self.cursor].char_indices().next_back() {
            self.cursor = i;
        }
    }

    fn move_right(&mut self) {
        self.menu = None;
        if let Some(c) = self.buffer[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    fn word_left(&mut self) {
        self.menu = None;
        self.cursor = word_start_before(&self.buffer, self.cursor);
    }

    fn word_right(&mut self) {
        self.menu = None;
        self.cursor = word_end_after(&self.buffer, self.cursor);
    }

    fn row_home(&mut self) {
        self.menu = None;
        let rows = wrap_rows(&self.buffer, content_width(ui::real_term_width()));
        let (row, _) = pos_to_rowcol(&self.buffer, &rows, self.cursor);
        self.cursor = rowcol_to_pos(&rows, row, 0);
    }

    fn row_end(&mut self) {
        self.menu = None;
        let rows = wrap_rows(&self.buffer, content_width(ui::real_term_width()));
        let (row, _) = pos_to_rowcol(&self.buffer, &rows, self.cursor);
        self.cursor = rowcol_to_pos(&rows, row, usize::MAX);
    }

    // ----- ↑/↓: cursor entre filas o navegación de historial -----

    fn up(&mut self) {
        if self.menu.is_some() {
            self.cycle_menu(-1);
            return;
        }
        let rows = wrap_rows(&self.buffer, content_width(ui::real_term_width()));
        let (row, col) = pos_to_rowcol(&self.buffer, &rows, self.cursor);
        if row > 0 {
            self.cursor = rowcol_to_pos(&rows, row - 1, col);
        } else {
            self.history_prev();
        }
    }

    fn down(&mut self) {
        if self.menu.is_some() {
            self.cycle_menu(1);
            return;
        }
        let rows = wrap_rows(&self.buffer, content_width(ui::real_term_width()));
        let (row, col) = pos_to_rowcol(&self.buffer, &rows, self.cursor);
        if row + 1 < rows.len() {
            self.cursor = rowcol_to_pos(&rows, row + 1, col);
        } else {
            self.history_next();
        }
    }

    fn history_prev(&mut self) {
        let idx = match self.hist_idx {
            Some(0) => return,
            Some(i) => i - 1,
            None if self.history.is_empty() => return,
            None => {
                self.draft = self.buffer.clone();
                self.history.len() - 1
            }
        };
        self.hist_idx = Some(idx);
        self.buffer = self.history[idx].clone();
        self.cursor = self.buffer.len();
    }

    fn history_next(&mut self) {
        match self.hist_idx {
            None => {}
            Some(i) if i + 1 < self.history.len() => {
                self.hist_idx = Some(i + 1);
                self.buffer = self.history[i + 1].clone();
                self.cursor = self.buffer.len();
            }
            Some(_) => {
                self.hist_idx = None;
                self.buffer = std::mem::take(&mut self.draft);
                self.cursor = self.buffer.len();
            }
        }
    }

    // ----- pegado -----

    /// Inserta un pegado: corto va directo al buffer; largo se colapsa a un
    /// placeholder editable que se expande al enviar.
    fn insert_paste(&mut self, text: &str) {
        let content = text.replace("\r\n", "\n").replace('\r', "\n");
        let lines = content.lines().count().max(1);
        let chars = content.chars().count();
        if lines <= PASTE_INLINE_MAX_LINES && chars <= PASTE_INLINE_MAX_CHARS {
            self.insert(&content);
            return;
        }
        let placeholder =
            format!("[pegado #{}: {} líneas, {} caracteres]", self.pastes.len() + 1, lines, chars);
        self.insert(&format!("{placeholder} "));
        self.pastes.push(PendingPaste { placeholder, content });
    }

    // ----- autocompletado -----

    /// Tab: abre el menú (insertando el prefijo común) o cicla el siguiente.
    fn tab(&mut self) {
        if self.menu.is_some() {
            self.cycle_menu(1);
            return;
        }
        let Some((word_start, cands)) = completion_candidates(self.cwd, &self.buffer, self.cursor)
        else {
            return;
        };
        if cands.is_empty() {
            return;
        }
        if cands.len() == 1 {
            self.replace_word(word_start, self.cursor, &cands[0].replacement.clone());
            return;
        }
        let prefix = common_prefix(&cands);
        let word_end = if prefix.len() > self.cursor - word_start {
            self.replace_word(word_start, self.cursor, &prefix);
            word_start + prefix.len()
        } else {
            self.cursor
        };
        self.menu = Some(Menu { cands, idx: None, word_start, word_end });
    }

    /// Avanza (`+1`) o retrocede (`-1`) en el menú, aplicando el candidato.
    fn cycle_menu(&mut self, dir: isize) {
        let Some(mut menu) = self.menu.take() else { return };
        let n = menu.cands.len() as isize;
        let next = match menu.idx {
            None if dir > 0 => 0,
            None => n - 1,
            Some(i) => (i as isize + dir).rem_euclid(n),
        } as usize;
        let repl = menu.cands[next].replacement.clone();
        self.replace_word(menu.word_start, menu.word_end, &repl);
        menu.idx = Some(next);
        menu.word_end = menu.word_start + repl.len();
        self.menu = Some(menu);
    }

    fn replace_word(&mut self, start: usize, end: usize, repl: &str) {
        self.buffer.replace_range(start..end, repl);
        self.cursor = start + repl.len();
    }
}

/// Drena una ráfaga de eventos pendientes (un pegado) y la devuelve como
/// texto. ConPTY entrega los pegados grandes TROCEADOS, con huecos entre
/// tramos: el `poll` con timeout sigue esperando hasta que haya 60 ms de
/// silencio total (cortarse a mitad dejaría el resto entrando como teclas
/// sueltas, donde cada Enter enviaría un mensaje).
fn collect_burst(first: char) -> io::Result<String> {
    let mut s = String::new();
    s.push(first);
    while event::poll(Duration::from_millis(60))? {
        match event::read()? {
            Event::Key(k) if k.kind != KeyEventKind::Release => match k.code {
                KeyCode::Char(c) => s.push(c),
                KeyCode::Enter => s.push('\n'),
                KeyCode::Tab => s.push('\t'),
                _ => {}
            },
            Event::Paste(p) => s.push_str(&p),
            _ => {}
        }
    }
    Ok(s)
}

// ----- layout: wrapping y mapeo cursor ↔ (fila, columna) -----

/// Una fila de pantalla del input: su texto y el offset en bytes del buffer
/// donde empieza.
struct Row {
    text: String,
    start: usize,
}

/// Ancho útil para el texto del input (descontando el prefijo del prompt).
fn content_width(term_width: usize) -> usize {
    term_width.saturating_sub(PROMPT.chars().count() + 1).max(10)
}

/// Alto real de la terminal, para acotar la región del input.
fn term_height() -> usize {
    terminal::size().map(|(_, h)| h as usize).unwrap_or(24).max(8)
}

/// Parte el buffer en filas de pantalla: una por línea lógica, más las de
/// wrapping. Cada línea produce al menos una fila (y una extra vacía si llena
/// exactamente el ancho, para que el cursor al final tenga dónde dibujarse).
fn wrap_rows(buffer: &str, content_w: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut offset = 0usize;
    for line in buffer.split('\n') {
        let chars: Vec<char> = line.chars().collect();
        let n_rows = chars.len() / content_w + 1;
        let mut start = offset;
        for i in 0..n_rows {
            let seg: String =
                chars[i * content_w..((i + 1) * content_w).min(chars.len())].iter().collect();
            let seg_bytes = seg.len();
            rows.push(Row { text: seg, start });
            start += seg_bytes;
        }
        offset += line.len() + 1; // +1 por el '\n'
    }
    rows
}

/// Fila y columna (en chars) donde se dibuja el cursor que está en el byte
/// `cursor` del buffer.
fn pos_to_rowcol(buffer: &str, rows: &[Row], cursor: usize) -> (usize, usize) {
    let mut row = 0;
    for (i, r) in rows.iter().enumerate() {
        if r.start <= cursor {
            row = i;
        } else {
            break;
        }
    }
    let col = buffer[rows[row].start..cursor.min(buffer.len())].chars().count();
    (row, col.min(rows[row].text.chars().count()))
}

/// Offset en bytes del buffer correspondiente a una (fila, columna) de
/// pantalla; la columna se acota al largo de la fila.
fn rowcol_to_pos(rows: &[Row], row: usize, col: usize) -> usize {
    let r = &rows[row.min(rows.len().saturating_sub(1))];
    let take = col.min(r.text.chars().count());
    r.start + r.text.chars().take(take).map(char::len_utf8).sum::<usize>()
}

/// Inicio de la palabra anterior al cursor (para Ctrl+←, Ctrl+W).
fn word_start_before(buffer: &str, cursor: usize) -> usize {
    let head = &buffer[..cursor];
    let trimmed = head.trim_end_matches(char::is_whitespace);
    match trimmed.rfind(char::is_whitespace) {
        Some(i) => i + buffer[i..].chars().next().map_or(1, char::len_utf8),
        None => 0,
    }
}

/// Final de la palabra siguiente al cursor (para Ctrl+→).
fn word_end_after(buffer: &str, cursor: usize) -> usize {
    let tail = &buffer[cursor..];
    let skipped = tail.len() - tail.trim_start_matches(char::is_whitespace).len();
    let word = &tail[skipped..];
    let end = word.find(char::is_whitespace).unwrap_or(word.len());
    cursor + skipped + end
}

// ----- resaltado -----

/// Pinta una fila del input: un comando `/...` va entero en acento; si no,
/// palabras reservadas y referencias `@archivo` en acento. (El resaltado es
/// por fila: una palabra partida por wrapping pierde el color — menor.)
fn colorize(text: &str, is_command_line: bool) -> String {
    if is_command_line {
        return ui::accent(text);
    }
    if text.split(' ').any(|w| is_reserved_word(w) || is_file_ref(w)) {
        highlight_reserved(text)
    } else {
        text.to_string()
    }
}

/// ¿Es `word` una palabra reservada del CLI? (token exacto, sin distinguir mayúsculas)
fn is_reserved_word(word: &str) -> bool {
    let w = word.to_ascii_lowercase();
    matches!(
        w.as_str(),
        "dpx" | "pro" | "hack" | "deepseek" | "kimi" | "qwen"
    ) || focus::catalog().iter().any(|f| f.id == w)
}

/// ¿Es `word` una referencia a archivo `@ruta`?
fn is_file_ref(word: &str) -> bool {
    word.len() > 1 && word.starts_with('@')
}

/// Reconstruye la línea coloreando palabras reservadas y referencias `@archivo`,
/// preservando los espacios exactos.
fn highlight_reserved(line: &str) -> String {
    let mut out = String::new();
    for (i, part) in line.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if is_reserved_word(part) || is_file_ref(part) {
            out.push_str(&ui::accent(part));
        } else {
            out.push_str(part);
        }
    }
    out
}

// ----- candidatos de autocompletado -----

/// Candidatos para la palabra bajo el cursor: comandos `/...` (si la línea
/// entera es un comando a medias) o rutas `@...` del proyecto. Devuelve
/// `(inicio de la palabra, candidatos)`.
fn completion_candidates(cwd: &Path, buffer: &str, cursor: usize) -> Option<(usize, Vec<Cand>)> {
    let head = &buffer[..cursor.min(buffer.len())];

    // Comando: la línea entera es /... (sin espacio aún).
    if head.starts_with('/') && !head.contains(' ') && !head.contains('\n') {
        let cands = COMMANDS
            .iter()
            .filter(|c| c.starts_with(head))
            .map(|c| Cand { display: (*c).to_string(), replacement: (*c).to_string() })
            .collect();
        return Some((0, cands));
    }

    // Referencia @archivo: completar rutas del proyecto desde la palabra actual.
    let word_start = head.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
    let partial = head[word_start..].strip_prefix('@')?;
    Some((word_start, path_candidates(cwd, partial)))
}

/// Candidatos de ruta para autocompletar `@...`, navegando por subcarpetas.
fn path_candidates(cwd: &Path, partial: &str) -> Vec<Cand> {
    let (dir, prefix) = match partial.rfind('/') {
        Some(i) => (&partial[..=i], &partial[i + 1..]), // dir incluye el '/' final
        None => ("", partial),
    };
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(cwd.join(dir)) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            // Saltamos ocultos y carpetas de build/control de versiones.
            if name.starts_with('.') || name == "target" || !name.starts_with(prefix) {
                continue;
            }
            let is_dir = entry.path().is_dir();
            let slash = if is_dir { "/" } else { "" };
            out.push(Cand {
                display: format!("{name}{slash}"),
                replacement: format!("@{dir}{name}{slash}"),
            });
        }
    }
    out.sort_by(|a, b| a.replacement.cmp(&b.replacement));
    out
}

/// Prefijo común más largo entre los `replacement` de los candidatos.
fn common_prefix(cands: &[Cand]) -> String {
    let mut prefix = cands[0].replacement.clone();
    for c in &cands[1..] {
        while !c.replacement.starts_with(&prefix) {
            prefix.pop();
            if prefix.is_empty() {
                return prefix;
            }
        }
    }
    prefix
}

// ----- expansión de pegados -----

/// Sustituye los placeholders `[pegado #N: …]` por su contenido real. Iterativo
/// y acotado: un pegado puede contener el placeholder de otro anterior.
fn expand_pastes(mut line: String, pastes: &[PendingPaste]) -> String {
    for _ in 0..=pastes.len() {
        let mut changed = false;
        for p in pastes {
            if line.contains(&p.placeholder) {
                line = line.replace(&p.placeholder, &p.content);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rowtexts(buffer: &str, w: usize) -> Vec<String> {
        wrap_rows(buffer, w).into_iter().map(|r| r.text).collect()
    }

    #[test]
    fn wrap_rows_linea_simple_y_vacia() {
        assert_eq!(rowtexts("hola", 10), vec!["hola"]);
        assert_eq!(rowtexts("", 10), vec![""]);
    }

    #[test]
    fn wrap_rows_corta_al_ancho_y_fila_extra_si_llena_exacto() {
        assert_eq!(rowtexts("abcdefghij", 5), vec!["abcde", "fghij", ""]);
        assert_eq!(rowtexts("abcdefg", 5), vec!["abcde", "fg"]);
    }

    #[test]
    fn wrap_rows_multilinea() {
        assert_eq!(rowtexts("uno\ndos", 10), vec!["uno", "dos"]);
    }

    #[test]
    fn cursor_rowcol_ida_y_vuelta() {
        let buf = "abcdefg\nhola";
        let rows = wrap_rows(buf, 5);
        // cursor al final de "abcde" (byte 5) cae al inicio de la fila wrap.
        assert_eq!(pos_to_rowcol(buf, &rows, 5), (1, 0));
        // cursor al final de la primera línea (byte 7, antes del \n).
        assert_eq!(pos_to_rowcol(buf, &rows, 7), (1, 2));
        // cursor en "hola" (línea 2): byte 8 + 2.
        assert_eq!(pos_to_rowcol(buf, &rows, 10), (2, 2));
        assert_eq!(rowcol_to_pos(&rows, 2, 2), 10);
        // columna fuera de rango se acota al final de la fila.
        assert_eq!(rowcol_to_pos(&rows, 0, 99), 5);
    }

    #[test]
    fn cursor_rowcol_con_multibyte() {
        let buf = "ñandú"; // ñ y ú son 2 bytes
        let rows = wrap_rows(buf, 10);
        assert_eq!(pos_to_rowcol(buf, &rows, buf.len()), (0, 5));
        assert_eq!(rowcol_to_pos(&rows, 0, 5), buf.len());
    }

    #[test]
    fn word_start_y_end() {
        let buf = "hola mundo cruel";
        assert_eq!(word_start_before(buf, 10), 5); // desde el final de "mundo"
        assert_eq!(word_start_before(buf, 11), 5); // desde el espacio siguiente
        assert_eq!(word_end_after(buf, 4), 10); // del espacio al final de "mundo"
        assert_eq!(word_start_before(buf, 0), 0);
        assert_eq!(word_end_after(buf, buf.len()), buf.len());
    }

    #[test]
    fn expand_pastes_sustituye_placeholder() {
        let pastes = vec![PendingPaste {
            placeholder: "[pegado #1: 2 líneas, 8 caracteres]".to_string(),
            content: "uno\ndos".to_string(),
        }];
        let line = "mira esto: [pegado #1: 2 líneas, 8 caracteres] ¿qué opinas?".to_string();
        assert_eq!(expand_pastes(line, &pastes), "mira esto: uno\ndos ¿qué opinas?");
    }

    #[test]
    fn expand_pastes_anidado_y_placeholder_borrado() {
        let pastes = vec![
            PendingPaste { placeholder: "[pegado #1: x]".to_string(), content: "AAA".to_string() },
            PendingPaste {
                placeholder: "[pegado #2: y]".to_string(),
                content: "antes [pegado #1: x] después".to_string(),
            },
        ];
        // El pegado 2 contiene el placeholder del 1: expansión iterativa.
        assert_eq!(expand_pastes("[pegado #2: y]".to_string(), &pastes), "antes AAA después");
        // Placeholder borrado por el usuario: el pegado se descarta sin error.
        assert_eq!(expand_pastes("sin nada".to_string(), &pastes), "sin nada");
    }

    #[test]
    fn completion_de_comandos() {
        let cwd = std::env::temp_dir();
        let (start, cands) = completion_candidates(&cwd, "/he", 3).unwrap();
        assert_eq!(start, 0);
        assert_eq!(cands.len(), 1);
        assert_eq!(cands[0].replacement, "/help");
        // Con espacio ya no es un comando a medias.
        assert!(completion_candidates(&cwd, "/help x", 7).is_none());
    }

    #[test]
    fn common_prefix_de_candidatos() {
        let cands = vec![
            Cand { display: String::new(), replacement: "/mode".to_string() },
            Cand { display: String::new(), replacement: "/models".to_string() },
        ];
        assert_eq!(common_prefix(&cands), "/mode");
    }
}
