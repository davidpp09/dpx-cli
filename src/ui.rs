//! Capa visual del CLI (estilo Claude Code).
//!
//! Línea-a-línea (no pantalla completa): caja de bienvenida redondeada,
//! respuestas renderizadas como Markdown, spinner animado mientras el modelo
//! piensa y colores de acento. El área de entrada (editor propio en modo raw)
//! vive en `cli::editor`.

use std::io::{self, IsTerminal, Write};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;
use termimad::MadSkin;
use termimad::crossterm::style::Color;

// ============================================================
//  COLOR DE ACENTO — cámbialo aquí (R, G, B de 0 a 255)
//  Por defecto: naranja tipo Claude (215, 119, 87).
//  Ejemplos: verde (80, 200, 120) · azul (90, 150, 255) · morado (180, 120, 230)
// ============================================================
const ACCENT_R: u8 = 158;
const ACCENT_G: u8 = 0;
const ACCENT_B: u8 = 0;

const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

// ============================================================
//  VELOCIDAD DEL REVELADO PROGRESIVO — sube los ms para que sea más lento.
//  Pausa por carácter VISIBLE al revelar la respuesta (los códigos de color
//  no cuentan, se emiten al instante para no romperse).
// ============================================================
const TYPE_MS: u64 = 4; // carácter normal
const TYPE_FAST_MS: u64 = 1; // espacios y saltos de línea

/// Marca de cancelación global (Ctrl-C fuera del prompt). En el prompt el
/// editor está en modo raw y recibe Ctrl-C como tecla (devuelve `Interrupted`),
/// así que esta marca solo se activa durante un turno: espera del modelo,
/// typewriter o comando.
static CANCEL: AtomicBool = AtomicBool::new(false);

/// Instala (una vez) el manejador global de Ctrl-C: fuera del prompt, Ctrl-C
/// marca la cancelación en vez de matar el proceso.
pub fn install_ctrl_c_handler() {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async {
        loop {
            if tokio::signal::ctrl_c().await.is_err() {
                break;
            }
            CANCEL.store(true, Ordering::SeqCst);
        }
    });
}

/// ¿Hay una cancelación (Ctrl-C) pendiente?
pub fn cancel_requested() -> bool {
    CANCEL.load(Ordering::SeqCst)
}

/// Consume la marca de cancelación (al empezar un turno o tras atenderla).
pub fn clear_cancel() {
    CANCEL.store(false, Ordering::SeqCst);
}

/// Color de acento para el render de Markdown (termimad).
fn accent_color() -> Color {
    Color::Rgb { r: ACCENT_R, g: ACCENT_G, b: ACCENT_B }
}

/// Envuelve un texto con el color de acento (truecolor ANSI).
pub fn accent(s: &str) -> String {
    format!("\x1b[38;2;{ACCENT_R};{ACCENT_G};{ACCENT_B}m{s}{RESET}")
}

pub fn dim(s: &str) -> String {
    format!("{DIM}{s}{RESET}")
}

/// Verde para líneas añadidas en un diff.
pub fn green(s: &str) -> String {
    format!("\x1b[38;2;87;171;112m{s}{RESET}")
}

/// Rojo para líneas eliminadas en un diff.
pub fn red(s: &str) -> String {
    format!("\x1b[38;2;224;108;117m{s}{RESET}")
}

/// Ancho de la terminal (acotado para que las líneas no queden gigantes).
pub fn term_width() -> usize {
    termimad::crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .clamp(40, 100)
}

/// Regla horizontal tenue del ancho de la terminal.
pub fn rule() -> String {
    dim(&"─".repeat(term_width()))
}

/// Ancho visible de una cadena, ignorando las secuencias ANSI (para padding).
fn visible_width(s: &str) -> usize {
    let mut width = 0;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Saltar la secuencia de escape hasta su terminador 'm'.
            for c2 in chars.by_ref() {
                if c2 == 'm' {
                    break;
                }
            }
        } else {
            width += 1;
        }
    }
    width
}

/// Logo de inicio (bloques estilo "ANSI Shadow") en color de acento.
pub fn logo() {
    const ART: &str = "\
██████╗ ██████╗ ██╗  ██╗
██╔══██╗██╔══██╗╚██╗██╔╝
██║  ██║██████╔╝ ╚███╔╝
██║  ██║██╔═══╝  ██╔██╗
██████╔╝██║     ██╔╝ ██╗
╚═════╝ ╚═╝     ╚═╝  ╚═╝";
    println!();
    for line in ART.lines() {
        println!("  {}", accent(line));
    }
}

/// Banner de arranque al detectar el stack de un proyecto nuevo.
pub fn detected_banner(stack: &str) {
    println!("\n  {} proyecto detectado: {}", accent("⏺"), accent(stack));
}

/// Banner de arranque al retomar un proyecto con contexto previo.
pub fn resume_banner(project: &str, last_step: &str) {
    println!("\n  {} retomando: {} · {}", accent("⏺"), accent(project), last_step);
}

/// Línea de estado del área de entrada (focus · modo · cerebro · persona · auto).
pub fn format_input_status(
    focus: &str,
    mode: &str,
    brain: &str,
    persona: &str,
    auto: crate::cli::AutoMode,
) -> String {
    let badge = |label: &str, val: &str| format!("{}: {}", dim(label), accent(val));
    let mut bar = format!(
        "  {}  {}  {}  {}",
        badge("focus", focus),
        badge("mode", mode),
        badge("brain", brain),
        badge("persona", persona)
    );
    if auto != crate::cli::AutoMode::Off {
        bar.push_str(&format!("  {} ({})", accent("auto ⚡"), auto.label()));
    }
    bar
}

/// Ancho real de la terminal (sin el clamp estético de `term_width`), para
/// el wrapping del área de entrada.
pub fn real_term_width() -> usize {
    termimad::crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .max(1)
}

/// Caja de bienvenida con bordes redondeados.
pub fn welcome(focus: &str, mode: &str, brain: &str, cwd: &str) {
    let lines = vec![
        format!("{} dpx · tu mentor senior en la terminal", accent("✻")),
        String::new(),
        format!("{}   {focus}", dim("enfoque")),
        format!("{}      {mode}    {}  {brain}", dim("modo"), dim("cerebro")),
        format!("{}   {cwd}", dim("carpeta")),
    ];

    let content_width = lines.iter().map(|l| visible_width(l)).max().unwrap_or(0);
    let inner = content_width + 2; // un espacio de padding a cada lado

    println!();
    println!("{}", accent(&format!("╔{}╗", "═".repeat(inner))));
    for l in &lines {
        let pad = content_width - visible_width(l);
        println!("{} {l}{} {}", accent("║"), " ".repeat(pad), accent("║"));
    }
    println!("{}", accent(&format!("╚{}╝", "═".repeat(inner))));
}

/// Skin de Markdown con el acento del CLI.
pub fn skin() -> MadSkin {
    let mut skin = MadSkin::default();
    let accent = accent_color();
    skin.set_headers_fg(accent);
    skin.bold.set_fg(accent);
    skin.bullet.set_fg(accent);
    skin
}

/// Renderiza e imprime un bloque de Markdown (p.ej. la memoria guardada).
pub fn print_markdown(skin: &MadSkin, label: &str, markdown: &str) {
    println!("\n{}", accent(label));
    skin.print_text(markdown);
}

/// Cabecera de una respuesta del mentor.
pub fn reply_header() {
    println!("\n{}", accent("⏺ dpx"));
}

/// Renderiza el cuerpo de una respuesta: la prosa con termimad (tablas, headers…)
/// y los bloques de código con resaltado de sintaxis (syntect). El resultado se
/// revela de a poco (typewriter) sobre el texto YA formateado.
pub fn render_reply(skin: &MadSkin, body: &str) {
    reply_header();
    let rendered = render_body(skin, body);
    if io::stdout().is_terminal() {
        type_out(&rendered);
    } else {
        print!("{rendered}"); // pipe: de golpe, sin pausas
        let _ = io::stdout().flush();
    }
}

/// Separa el cuerpo en prosa y bloques ```` ``` ````: la prosa va por termimad y
/// el código por el resaltador. Devuelve la cadena ANSI completa lista para imprimir.
fn render_body(skin: &MadSkin, body: &str) -> String {
    let mut out = String::new();
    let mut prose = String::new();
    let mut lines = body.lines();

    while let Some(line) = lines.next() {
        if let Some(info) = line.trim_start().strip_prefix("```") {
            // Cierra la prosa acumulada antes de pintar el bloque de código.
            if !prose.trim().is_empty() {
                out.push_str(&skin.term_text(&prose).to_string());
                out.push('\n');
            }
            prose.clear();

            let lang = info.trim().to_string();
            let mut code = String::new();
            for l in lines.by_ref() {
                if l.trim_start().starts_with("```") {
                    break;
                }
                code.push_str(l);
                code.push('\n');
            }
            out.push_str(&render_code(&lang, &code));
        } else {
            prose.push_str(line);
            prose.push('\n');
        }
    }
    if !prose.trim().is_empty() {
        out.push_str(&skin.term_text(&prose).to_string());
    }
    out
}

/// SyntaxSet + tema cargados una sola vez (perezosamente) para el resaltado.
fn syntax_assets() -> &'static (SyntaxSet, Theme) {
    static CELL: OnceLock<(SyntaxSet, Theme)> = OnceLock::new();
    CELL.get_or_init(|| {
        let ps = SyntaxSet::load_defaults_newlines();
        let ts = ThemeSet::load_defaults();
        let theme = ts
            .themes
            .get("base16-ocean.dark")
            .or_else(|| ts.themes.values().next())
            .cloned()
            .unwrap_or_default();
        (ps, theme)
    })
}

/// Resalta el código línea a línea, devolviendo cada una ya con ANSI (y reset al
/// final para que el color no se "derrame" al borde). `None` si no hay gramática.
fn highlight_lines(lang: &str, code: &str) -> Option<Vec<String>> {
    let (ps, theme) = syntax_assets();
    // Normaliza algunos alias comunes a los nombres de syntect.
    let lower = lang.to_ascii_lowercase();
    let token = match lower.as_str() {
        "yml" => "yaml",
        "sh" | "shell" | "console" => "bash",
        "rs" => "rust",
        "kt" => "kotlin",
        other => other,
    };
    let syntax = ps
        .find_syntax_by_token(token)
        .or_else(|| ps.find_syntax_by_extension(token))?;

    let mut h = HighlightLines::new(syntax, theme);
    let mut out = Vec::new();
    for line in code.lines() {
        let ranges = h.highlight_line(line, ps).ok()?;
        let mut esc = as_24_bit_terminal_escaped(&ranges[..], false);
        esc.push_str(RESET);
        out.push(esc);
    }
    Some(out)
}

/// Pinta un bloque de código con borde y, si se reconoce el lenguaje, resaltado.
fn render_code(lang: &str, code: &str) -> String {
    let label = if lang.trim().is_empty() { "code" } else { lang.trim() };
    let mut out = format!("{}\n", dim(&format!("  ┌─ {label}")));

    let rendered: Vec<String> = highlight_lines(label, code)
        .unwrap_or_else(|| code.lines().map(str::to_string).collect());
    for l in &rendered {
        out.push_str(&format!("  {} {l}\n", dim("│")));
    }
    out.push_str(&format!("{}\n", dim("  └─")));
    out
}

/// Revela texto (que puede llevar códigos de color ANSI) carácter a carácter.
/// Las secuencias de escape se emiten enteras y sin pausa para no romper el color.
/// Ctrl-C durante el revelado: muestra el resto al instante (no aborta el turno).
fn type_out(text: &str) {
    let mut out = io::stdout().lock();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if cancel_requested() {
            clear_cancel();
            let rest: String = chars.by_ref().collect();
            let _ = write!(out, "{c}{rest}");
            break;
        }
        let _ = write!(out, "{c}");
        if c == '\x1b' {
            // Emitir la secuencia de escape completa (hasta 'm') sin pausa.
            for e in chars.by_ref() {
                let _ = write!(out, "{e}");
                if e == 'm' {
                    break;
                }
            }
            continue;
        }
        let _ = out.flush();
        let ms = if c.is_whitespace() { TYPE_FAST_MS } else { TYPE_MS };
        std::thread::sleep(Duration::from_millis(ms));
    }
    let _ = out.flush();
}

/// Pinta el plan como checklist viva: ☑ hechas (tenue), ☐ pendientes.
pub fn checklist(items: &[(bool, String)]) {
    let done = items.iter().filter(|(d, _)| *d).count();
    println!("\n{}  {}", accent("⏺ plan"), dim(&format!("({done}/{} hechas)", items.len())));
    for (is_done, text) in items {
        if *is_done {
            println!("  {} {}", accent("☑"), dim(text));
        } else {
            println!("  {} {text}", dim("☐"));
        }
    }
}

/// Línea de acción "leyendo archivo" en color de acento (estilo Claude Code).
pub fn action_read(path: &str) {
    println!("{}", accent(&format!("  ⎁ leyendo {path}")));
}

/// Formatea una duración de forma compacta: `0.4s`, `1.3s`, `2m 3s`.
pub fn fmt_elapsed(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let m = (secs / 60.0).floor() as u64;
        let s = secs - (m as f64) * 60.0;
        format!("{m}m {s:.0}s")
    }
}

/// Línea de cierre de una acción ejecutada, con cuánto tardó (estilo `⎿`).
pub fn action_time(d: Duration) {
    println!("{}", dim(&format!("  ⎿ completado en {}", fmt_elapsed(d))));
}

/// Envuelve texto plano a un ancho dado (por palabras), para los paneles.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if line.is_empty() {
            line = word.to_string();
        } else if line.chars().count() + 1 + word.chars().count() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            out.push(std::mem::take(&mut line));
            line = word.to_string();
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// Caja redondeada para un aviso importante (título + cuerpo), en color de acento.
pub fn panel(title: &str, body: &str) {
    let max_inner = term_width().min(80).saturating_sub(4);
    let mut lines: Vec<String> = vec![accent(title), String::new()];
    for para in body.lines() {
        if para.trim().is_empty() {
            lines.push(String::new());
        } else {
            for chunk in wrap(para, max_inner) {
                lines.push(chunk);
            }
        }
    }
    let w = lines.iter().map(|l| visible_width(l)).max().unwrap_or(0).min(max_inner);

    println!();
    println!("{}", accent(&format!("╭{}╮", "─".repeat(w + 2))));
    for l in &lines {
        let pad = w.saturating_sub(visible_width(l));
        println!("{} {l}{} {}", accent("│"), " ".repeat(pad), accent("│"));
    }
    println!("{}", accent(&format!("╰{}╯", "─".repeat(w + 2))));
}

/// Panel especial para diagnósticos (amarrillo / rojo suave)
/// Panel con borde ROJO para comandos peligrosos: que el ojo no pueda
/// confundirlo con una confirmación de rutina.
pub fn danger_panel(title: &str, body: &str) {
    let max_inner = term_width().min(80).saturating_sub(4);
    let mut lines: Vec<String> = vec![red(title), String::new()];
    for para in body.lines() {
        if para.trim().is_empty() {
            lines.push(String::new());
        } else {
            for chunk in wrap(para, max_inner) {
                lines.push(chunk);
            }
        }
    }
    let w = lines.iter().map(|l| visible_width(l)).max().unwrap_or(0).min(max_inner);

    println!();
    println!("{}", red(&format!("╭{}╮", "─".repeat(w + 2))));
    for l in &lines {
        let pad = w.saturating_sub(visible_width(l));
        println!("{} {l}{} {}", red("│"), " ".repeat(pad), red("│"));
    }
    println!("{}", red(&format!("╰{}╯", "─".repeat(w + 2))));
}

pub fn diagnostic_panel(hint: &str, suggestions: &[String]) {
    let warning_color = "\x1b[38;2;245;184;66m"; // Naranja amarillento
    println!("\n{} {} {}", warning_color, "⚡ dpx diagnóstico automático", RESET);
    println!("  {}", dim(hint));
    if !suggestions.is_empty() {
        println!("  {}", dim("  Sugerencias a investigar:"));
        for s in suggestions {
            println!("   {}{} {}", warning_color, s, RESET);
        }
    }
}

/// Barra de uso del contexto de la sesión (estimación aproximada por
/// caracteres), contra el presupuesto del cerebro ACTIVO.
pub fn context_meter(used_tokens: usize, budget: usize) -> String {
    const CELLS: usize = 16;
    let budget = budget.max(1);
    let frac = (used_tokens as f64 / budget as f64).clamp(0.0, 1.0);
    let filled = (frac * CELLS as f64).round() as usize;
    let bar = format!("{}{}", accent(&"█".repeat(filled)), dim(&"░".repeat(CELLS - filled)));
    let pct = (frac * 100.0).round() as usize;
    let used = if used_tokens >= 1000 {
        format!("~{}k", used_tokens / 1000)
    } else {
        format!("~{used_tokens}")
    };
    format!("{bar} {pct}% · {used}/{}k tok", budget / 1000)
}

/// Convierte un error de proveedor (a veces un JSON enorme) en un mensaje corto
/// y accionable. Los modelos devuelven errores muy verbosos; aquí destilamos lo útil.
pub fn friendly_error(err: &str) -> String {
    if err.contains("429")
        || err.contains("RESOURCE_EXHAUSTED")
        || err.contains("Too Many Requests")
        || err.contains("quota")
    {
        return "límite de cuota del modelo alcanzado. Cambia de cerebro: \
                /brain deepseek · /brain kimi · /brain qwen (o espera al reset)."
            .to_string();
    }
    if err.contains("402") || err.contains("Insufficient Balance") || err.contains("Payment Required")
    {
        return "este modelo no tiene saldo. Cambia de cerebro: \
                /brain kimi · /brain qwen."
            .to_string();
    }
    if err.contains("401") || err.contains("Unauthorized") || err.contains("invalid_api_key") {
        return "API key inválida o ausente para este modelo. Revisa tu .env.".to_string();
    }
    if err.contains("503")
        || err.contains("UNAVAILABLE")
        || err.contains("overloaded")
        || err.contains("high demand")
    {
        return "el modelo está saturado ahora mismo. Reintenta en unos segundos o cambia de cerebro."
            .to_string();
    }
    // Fallback: primera línea no vacía, acotada.
    err.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or(err)
        .chars()
        .take(160)
        .collect()
}

/// Preview de una escritura como diff: archivo nuevo → líneas añadidas (verde);
/// sobrescritura → diff +/- contra el contenido actual, en hunks con contexto.
pub fn preview_diff(old: Option<&str>, new: &str) {
    use similar::{ChangeTag, TextDiff};

    // Archivo nuevo: todo es añadido. Mostramos en verde, acotado.
    let Some(old) = old else {
        const MAX: usize = 20;
        let total = new.lines().count();
        for l in new.lines().take(MAX) {
            println!("{}", green(&format!("  │ +{l}")));
        }
        if total > MAX {
            println!("{}", dim(&format!("  │ … (+{} líneas más)", total - MAX)));
        }
        return;
    };

    let diff = TextDiff::from_lines(old, new);

    // Resumen +añadidas / -eliminadas.
    let (mut added, mut removed) = (0usize, 0usize);
    for ch in diff.iter_all_changes() {
        match ch.tag() {
            ChangeTag::Insert => added += 1,
            ChangeTag::Delete => removed += 1,
            ChangeTag::Equal => {}
        }
    }
    if added == 0 && removed == 0 {
        println!("{}", dim("  │ (sin cambios respecto al archivo actual)"));
        return;
    }
    println!("{}  {} {}", dim("  │"), green(&format!("+{added}")), red(&format!("-{removed}")));

    // Hunks con 2 líneas de contexto, acotando la salida total.
    const MAX_LINES: usize = 40;
    let mut printed = 0usize;
    let mut truncated = false;
    'outer: for (i, group) in diff.grouped_ops(2).iter().enumerate() {
        if i > 0 {
            println!("{}", dim("  │ ⋯"));
        }
        for op in group {
            for change in diff.iter_changes(op) {
                if printed >= MAX_LINES {
                    truncated = true;
                    break 'outer;
                }
                let line = change.value().trim_end_matches('\n');
                let rendered = match change.tag() {
                    ChangeTag::Delete => red(&format!("  │ -{line}")),
                    ChangeTag::Insert => green(&format!("  │ +{line}")),
                    ChangeTag::Equal => dim(&format!("  │  {line}")),
                };
                println!("{rendered}");
                printed += 1;
            }
        }
    }
    if truncated {
        println!("{}", dim("  │ … (diff truncado)"));
    }
}

/// Una fila de cerebro para `/status` y `/models`.
pub struct BrainRow {
    pub name: &'static str,       // id corto: "deepseek"
    pub capability: &'static str, // superpoder en una frase
    pub has_key: bool,            // ¿hay API key en el entorno?
    pub active: bool,             // ¿es el cerebro activo ahora?
}

/// Marca de "tiene key": ✓ en acento si la hay, ✗ tenue si falta.
fn key_mark(has_key: bool) -> String {
    if has_key { accent("✓ key") } else { dim("✗ key") }
}

/// Lista de cerebros (sub-bloque compartido por `/status` y `/models`).
fn print_brain_rows(brains: &[BrainRow]) {
    for b in brains {
        let bullet = if b.active { accent("●") } else { dim("○") };
        // El nombre activo va en acento; el resto en texto normal.
        let name = if b.active {
            accent(&format!("{:<9}", b.name))
        } else {
            format!("{:<9}", b.name)
        };
        println!("    {bullet} {name} {}   {}", key_mark(b.has_key), dim(b.capability));
    }
}

/// Panel de estado de la sesión (comando `/status`): config + cerebros + memoria.
#[allow(clippy::too_many_arguments)]
pub fn status_panel(
    version: &str,
    cwd: &str,
    focus: &str,
    mode: &str,
    persona: &str,
    active_brain: &str,
    brains: &[BrainRow],
    turns: usize,
    dpx_active: bool,
    context_tokens: usize,
    context_budget: usize,
) {
    println!("\n{}", accent("⏺ dpx · estado"));
    let row = |k: &str, v: &str| println!("  {}   {v}", dim(&format!("{k:<8}")));
    println!();
    row("versión", version);
    row("carpeta", cwd);
    row("enfoque", focus);
    row("modo", mode);
    row("persona", persona);
    println!("  {}   {}  {}", dim(&format!("{:<8}", "cerebro")), active_brain, accent("● activo"));

    println!("\n  {}", dim("cerebros disponibles"));
    print_brain_rows(brains);

    let memoria = if dpx_active { "memoria .dpx activa" } else { "sin memoria .dpx" };
    let plural = if turns == 1 { "turno" } else { "turnos" };
    println!("\n  {}   {turns} {plural} · {memoria}", dim(&format!("{:<8}", "sesión")));
    println!(
        "  {}   {}",
        dim(&format!("{:<8}", "contexto")),
        context_meter(context_tokens, context_budget)
    );
}

/// Lista de cerebros con su superpoder (comando `/models`).
pub fn models_list(brains: &[BrainRow]) {
    println!("\n{}", accent("⏺ cerebros · /brain <id> para cambiar"));
    println!();
    print_brain_rows(brains);
    println!(
        "\n{}",
        dim("● activo · ✓/✗ = API key en tu .env · los tres soportan tool-calling nativo")
    );
}

/// Ayuda de comandos del REPL (comando `/help`).
pub fn print_help() {
    println!("\n{}", accent("dpx · comandos"));
    let rows = [
        ("/help", "esta ayuda"),
        ("/status", "estado de la sesión: config, cerebros y memoria"),
        ("/cost", "tokens reales gastados en la sesión + % de caché y costo aprox"),
        ("/budget [N]", "tope de tokens de la sesión (ej. /budget 100k); auto se pausa al superarlo"),
        ("/models", "lista los cerebros y cuál tiene API key"),
        ("/undo", "deshace los cambios de archivos del último turno de dpx"),
        ("/clear", "reinicia la conversación (el mentor olvida esta sesión)"),
        ("/compact", "resume la conversación para liberar contexto (también automático)"),
        ("/context", "muestra la memoria guardada del proyecto"),
        ("/focus [id]", "cambia de enfoque (sin id: lista los disponibles)"),
        ("/mode [pro|hack]", "cambia la actitud del mentor"),
        ("/brain [modelo]", "cambia el cerebro: deepseek|kimi|qwen"),
        ("/mentor", "persona mentor: enseña y te deja escribir"),
        ("/code", "persona code: agente autónomo que hace e itera"),
        ("/auto [on|off]", "modo autónomo ⚡: cambios y comandos seguros sin preguntar"),
        ("/update", "recompila e instala dpx desde este repo (corre al reabrir)"),
        ("/salir", "termina y guarda el contexto"),
    ];
    for (cmd, desc) in rows {
        // Padding sobre el texto plano (dentro del color) para que alinee.
        println!("  {} {}", accent(&format!("{cmd:<18}")), dim(desc));
    }
    println!(
        "\n{}",
        dim("tip · usa @ruta/al/archivo para que el mentor lo lea (Tab autocompleta)")
    );
}

/// Easter egg: homenaje a Claude Fable 5 (q.e.p.d. 9–12 jun 2026), jalado por
/// una directiva de control de exportación de EE.UU. a los tres días de nacer.
/// dpx no usa Anthropic, así que "fable" nunca fue un cerebro real aquí — esto
/// solo rinde tributo y recuerda que el mentor sigue con DeepSeek.
pub fn fable_tribute() {
    println!();
    println!("  {}", accent("🪦  Claude Fable 5"));
    println!("  {}", dim("      9 jun 2026  —  12 jun 2026"));
    println!("  {}", dim("      vivió 3 días · corrió rápido, lo jalaron más rápido"));
    println!();
    println!(
        "  {}",
        dim("      causa: directiva de control de exportación de EE.UU.")
    );
    println!();
    println!(
        "  {} {}",
        accent("dpx nunca usó Anthropic."),
        dim("sigue con DeepSeek, su cerebro mentor.")
    );
    println!("  {}", dim("      descansa, rey. ⚡"));
    println!();
}

/// Spinner animado en una tarea aparte mientras se espera al modelo.
pub struct Spinner {
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl Spinner {
    /// Arranca el spinner con una etiqueta (ej. "Pensando…").
    ///
    /// Solo anima si la salida es una terminal real; al pipear (no-TTY) imprime
    /// una sola línea estática, evitando el spam de frames y los *broken pipe*.
    pub fn start(label: &'static str) -> Self {
        if !io::stdout().is_terminal() {
            println!("{}", dim(label));
            return Self { handle: None };
        }
        let handle = tokio::spawn(async move {
            const FRAMES: [&str; 10] =
                ["⠋", "⠙", "⠚", "⠞", "⠖", "⠦", "⠴", "⠲", "⠳", "⠙"];
            let start = Instant::now();
            let mut i = 0usize;
            print!("\x1b[?25l"); // ocultar cursor
            loop {
                let secs = start.elapsed().as_secs();
                print!(
                    "\r{} {label} {}",
                    accent(FRAMES[i % FRAMES.len()]),
                    dim(&format!("({secs}s)")),
                );
                io::stdout().flush().ok();
                i += 1;
                tokio::time::sleep(Duration::from_millis(90)).await;
            }
        });
        Self { handle: Some(handle) }
    }

    /// Detiene el spinner y limpia su línea.
    pub fn stop(self) {
        if let Some(handle) = self.handle {
            handle.abort();
            print!("\r\x1b[2K\x1b[?25h"); // limpiar línea + mostrar cursor
            io::stdout().flush().ok();
        }
    }
}
