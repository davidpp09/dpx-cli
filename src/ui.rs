//! Capa visual del CLI (estilo Claude Code).
//!
//! Línea-a-línea (no pantalla completa): caja de bienvenida redondeada,
//! respuestas renderizadas como Markdown, spinner animado mientras el modelo
//! piensa y colores de acento. El área de entrada (editor propio en modo raw)
//! vive en `cli::editor`.

mod prompts;
pub use prompts::print_confirmation_box;

use std::io::{self, IsTerminal, Write};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use syntect::easy::HighlightLines;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::as_24_bit_terminal_escaped;
use termimad::MadSkin;
use termimad::crossterm::style::Color;

// ============================================================
//  TEMA POR MODO — el color de acento y el degradado cambian según el modo
//  activo (code azul · hack ámbar · learn verde). Se fija al arrancar
//  y en cada `/mode` con `set_mode_theme`, y TODA la UI bonita (logo, reglas,
//  bordes, spinner, acentos) lo lee en caliente desde estos átomos.
// ============================================================
/// Empaqueta un color RGB en un `u32` (0x00RRGGBB) para guardarlo en un átomo.
const fn pack_rgb(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

/// Desempaqueta un `u32` 0x00RRGGBB en su terna `(r, g, b)`.
fn unpack_rgb(v: u32) -> (u8, u8, u8) {
    (((v >> 16) & 0xff) as u8, ((v >> 8) & 0xff) as u8, (v & 0xff) as u8)
}

// Tema por defecto (rojo dpx), por si se pinta algo antes de elegir modo.
static ACCENT: AtomicU32 = AtomicU32::new(pack_rgb(158, 0, 0));
static GRAD_HI: AtomicU32 = AtomicU32::new(pack_rgb(188, 46, 40));
static GRAD_LO: AtomicU32 = AtomicU32::new(pack_rgb(66, 10, 14));

/// Color de acento activo (lo fija `set_mode_theme`).
fn accent_rgb() -> (u8, u8, u8) {
    unpack_rgb(ACCENT.load(Ordering::Relaxed))
}
/// Extremo claro del degradado activo.
fn grad_from() -> (u8, u8, u8) {
    unpack_rgb(GRAD_HI.load(Ordering::Relaxed))
}
/// Extremo oscuro del degradado activo.
fn grad_to() -> (u8, u8, u8) {
    unpack_rgb(GRAD_LO.load(Ordering::Relaxed))
}

/// Fija el tema visual (acento + degradado) según el modo activo. Cada modo
/// tiene su propia identidad de color: code azul, hack ámbar, learn verde.
pub fn set_mode_theme(mode: crate::focus::Mode) {
    use crate::focus::Mode;
    let (accent, hi, lo) = match mode {
        // code — azul: agente que construye, sereno y preciso.
        Mode::Code => ((74, 144, 226), (90, 160, 240), (28, 58, 130)),
        // hack — ámbar: energía, velocidad con criterio.
        Mode::Hack => ((224, 138, 38), (240, 165, 55), (130, 70, 10)),
        // learn — verde: crecimiento, aprender.
        Mode::Learn => ((76, 175, 110), (95, 200, 130), (24, 92, 58)),
    };
    ACCENT.store(pack_rgb(accent.0, accent.1, accent.2), Ordering::Relaxed);
    GRAD_HI.store(pack_rgb(hi.0, hi.1, hi.2), Ordering::Relaxed);
    GRAD_LO.store(pack_rgb(lo.0, lo.1, lo.2), Ordering::Relaxed);
}

const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";

// ============================================================
//  VELOCIDAD DEL REVELADO PROGRESIVO — sube los ms para que sea más lento.
//  Pausa por carácter VISIBLE al revelar la respuesta (los códigos de color
//  no cuentan, se emiten al instante para no romperse).
// ============================================================
const TYPE_MS: u64 = 4; // carácter normal
const TYPE_FAST_MS: u64 = 1; // espacios y saltos de línea

// ============================================================
//  VERBOS ROTATIVOS DEL SPINNER (estilo Claude Code: "Reasoning…", "Exploring…")
//  Cambian cada ~2s para que la espera se sienta viva y se note QUÉ fase corre.
// ============================================================
const THINKING_VERBS: &[&str] = &[
    "Pensando…",
    "Razonando…",
    "Tramando…",
    "Maquinando…",
    "Hilando fino…",
    "Conectando ideas…",
    "Rumiando…",
    "Cocinando…",
];
const WORKING_VERBS: &[&str] = &[
    "Continuando…",
    "Iterando…",
    "Avanzando…",
    "Puliendo…",
    "Ajustando…",
    "Rematando…",
];

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
    let (r, g, b) = accent_rgb();
    Color::Rgb { r, g, b }
}

/// Envuelve un texto con el color de acento (truecolor ANSI) del modo activo.
pub fn accent(s: &str) -> String {
    let (r, g, b) = accent_rgb();
    format!("\x1b[38;2;{r};{g};{b}m{s}{RESET}")
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

/// Interpola dos colores RGB con `t ∈ [0,1]`.
fn lerp_rgb(a: (u8, u8, u8), b: (u8, u8, u8), t: f64) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let mix = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    (mix(a.0, b.0), mix(a.1, b.1), mix(a.2, b.2))
}

/// Envuelve un solo carácter/segmento en un color RGB (truecolor).
fn rgb(s: &str, (r, g, b): (u8, u8, u8)) -> String {
    format!("\x1b[38;2;{r};{g};{b}m{s}{RESET}")
}

/// Pinta `s` con un degradado horizontal de `from` a `to`, carácter a carácter.
/// Bonito para títulos, reglas y bordes. Ignora longitudes 0/1 con gracia.
pub fn gradient(s: &str, from: (u8, u8, u8), to: (u8, u8, u8)) -> String {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    if n == 0 {
        return String::new();
    }
    let mut out = String::with_capacity(s.len() * 4);
    for (i, c) in chars.iter().enumerate() {
        let t = if n == 1 { 0.0 } else { i as f64 / (n - 1) as f64 };
        let (r, g, b) = lerp_rgb(from, to, t);
        out.push_str(&format!("\x1b[38;2;{r};{g};{b}m{c}"));
    }
    out.push_str(RESET);
    out
}

/// Atajo: degradado con la paleta del modo activo.
pub fn grad(s: &str) -> String {
    gradient(s, grad_from(), grad_to())
}

/// Pone el título de la pestaña/ventana de la terminal (secuencia OSC). No-op
/// si la salida no es una terminal real (p.ej. al pipear), para no ensuciarla.
/// Así la pestaña muestra qué hace dpx, como el resto de CLIs serias.
pub fn set_title(text: &str) {
    if io::stdout().is_terminal() {
        // OSC 0: fija título de ventana e icono. Terminador BEL (`\x07`).
        print!("\x1b]0;{text}\x07");
        let _ = io::stdout().flush();
    }
}

/// Título "en reposo": dpx esperando tu mensaje, con el proyecto a la vista.
pub fn title_idle(label: &str) {
    set_title(&format!("✳ dpx — {label}"));
}

/// Título "ocupado": la acción que dpx está ejecutando ahora mismo.
pub fn title_busy(activity: &str) {
    set_title(&format!("dpx · {activity}"));
}

/// Ancho de la terminal (acotado para que las líneas no queden gigantes).
pub fn term_width() -> usize {
    termimad::crossterm::terminal::size()
        .map(|(w, _)| w as usize)
        .unwrap_or(80)
        .clamp(40, 100)
}

/// Regla horizontal del ancho de la terminal, con degradado dinámico.
pub fn rule() -> String {
    grad(&"─".repeat(term_width()))
}

/// Ancho visible de una cadena, ignorando las secuencias ANSI (para padding).
pub(crate) fn visible_width(s: &str) -> usize {
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
    let lines: Vec<&str> = ART.lines().collect();
    let last = lines.len().saturating_sub(1).max(1);
    println!();
    for (i, line) in lines.iter().enumerate() {
        // Degradado VERTICAL: el color baja del extremo claro al oscuro por fila.
        let color = lerp_rgb(grad_from(), grad_to(), i as f64 / last as f64);
        println!("  {}", rgb(line, color));
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

/// Cartel de arranque PROPIO de cada modo: deja claro en cuál estás y qué
/// esperar. Se pinta al iniciar la sesión y se reúsa el color del tema activo.
pub fn mode_banner(mode: crate::focus::Mode) {
    use crate::focus::Mode;
    let (title, l1, l2) = match mode {
        Mode::Code => (
            "modo code activado",
            "hago el trabajo: escribo, ejecuto y corrijo hasta dejarlo funcionando.",
            "pídeme una tarea y voy a ello · /modo para cambiar de modo.",
        ),
        Mode::Hack => (
            "modo hack activado",
            "construyo rápido y con criterio: lo imprescindible que corre ya, sin chapuza.",
            "cuéntame qué quieres montar · /comité <idea> para evaluarla con un comité.",
        ),
        Mode::Learn => (
            "modo learn activado",
            "te hago pensar y te enseño el porqué — tú escribes, yo te guío.",
            "mira tu avance cuando quieras con /progreso · /temario para el plan.",
        ),
    };
    println!("\n  {}", grad(title));
    println!("  {}", dim(l1));
    println!("  {}", dim(l2));
}

/// Dibuja una caja redondeada con bordes ╭─╮ ╰─╯ usando el color indicado.
/// Cada línea de `lines` ya viene con sus colores; la función pone los bordes
/// y el padding a la derecha para que todas alineen. `max_width` opcional
/// acota el ancho interior (útil para paneles con texto largo).
fn draw_box(lines: &[String], color: fn(&str) -> String, max_width: Option<usize>) {
    let raw = lines.iter().map(|l| visible_width(l)).max().unwrap_or(0);
    let w = if let Some(max) = max_width { raw.min(max) } else { raw };
    let inner = w + 2;

    println!();
    println!("{}", color(&format!("╭{}╮", "─".repeat(inner))));
    for l in lines {
        let pad = w.saturating_sub(visible_width(l));
        println!("{} {l}{} {}", color("│"), " ".repeat(pad), color("│"));
    }
    println!("{}", color(&format!("╰{}╯", "─".repeat(inner))));
}

/// Panel de aprendizaje (comando `/progreso`): cada concepto con dots de nivel,
/// un badge de dominio en texto y mensaje motivador si aún no hay skills.
/// Reusa la caja redondeada con degradado del modo activo.
pub fn learn_panel(skills: &[crate::skill::Skill], today: &str) {
    use crate::skill::{SkillLevel, needs_review};

    // --- sin skills aún: mensaje motivador ---
    if skills.is_empty() {
        draw_box(
            &[
                grad("tu aprendizaje"),
                String::new(),
                dim("aún no hay nada registrado."),
                dim("entra en modo learn (/modo learn) y empieza a descubrir."),
                String::new(),
                dim("cada concepto que trabajemos irá apareciendo aquí"),
                dim("con su nivel: ●●● dominado · ●●○ practicando · ●○○ visto"),
            ],
            grad,
            None,
        );
        return;
    }

    // --- estadísticas ---
    let dominados = skills.iter().filter(|s| s.level == SkillLevel::Dominado).count();
    let practicando = skills.iter().filter(|s| s.level == SkillLevel::Practicando).count();
    let vistos = skills.iter().filter(|s| s.level == SkillLevel::Visto).count();
    let total = skills.len();

    // Badge de dominio en texto (sin emoji): según cuántos conceptos domina.
    let badge = if dominados >= 10 {
        "maestría"
    } else if dominados >= 5 {
        "nivel avanzado"
    } else if dominados >= 1 {
        "en marcha"
    } else {
        "recién empiezas"
    };

    let mut lines: Vec<String> = Vec::new();

    // Título y resumen
    lines.push(grad("tu aprendizaje"));
    lines.push(String::new());
    lines.push(format!(
        "{} {total} conceptos · {dominados} dominados · {badge}",
        dim("⏺"),
    ));
    if practicando > 0 || vistos > 0 {
        lines.push(format!(
            "  {} {practicando} practicando · {vistos} vistos",
            dim(""),
        ));
    }
    lines.push(String::new());

    // --- cada concepto con dots ---
    // Agrupados por stack (la lista ya viene ordenada por stack+tema desde merge).
    let mut current_stack = String::new();
    for s in skills {
        if s.stack != current_stack {
            current_stack = s.stack.clone();
            lines.push(format!("  {}", accent(&current_stack)));
        }
        let dots = match s.level {
            SkillLevel::Dominado => green("●●●").to_string(),
            SkillLevel::Practicando => dim("●●○").to_string(),
            SkillLevel::Visto => dim("●○○").to_string(),
        };
        lines.push(format!("    {}  {}", dots, s.topic));
    }

    // --- repaso espaciado ---
    let to_review: Vec<&crate::skill::Skill> = skills
        .iter()
        .filter(|s| needs_review(s, today))
        .collect();
    if !to_review.is_empty() {
        lines.push(String::new());
        lines.push(accent("↻ para repasar hoy").to_string());
        for s in &to_review {
            lines.push(format!("    {} {}", dim("·"), s.topic));
        }
        lines.push(dim("pídele a dpx \"repasemos\" en modo learn para reforzarlos."));
    }

    // --- pintar la caja ---
    draw_box(&lines, grad, None);
}

/// Línea de estado del área de entrada (focus · modo · cerebro · auto).
/// Trunca al ancho REAL de la terminal para evitar que desborde y corte la barra.
pub fn format_input_status(
    focus: &str,
    mode: &str,
    brain: &str,
    auto: crate::cli::AutoMode,
) -> String {
    let badge = |label: &str, val: &str| format!("{} {}", dim(label), grad(val));
    let sep = dim(" · ");
    let mut bar = format!(
        "  {}{sep}{}{sep}{}",
        badge("focus", focus),
        badge("mode", mode),
        badge("brain", brain),
    );
    if auto != crate::cli::AutoMode::Off {
        bar.push_str(&format!("{sep}{} {}", grad("auto"), dim(auto.label())));
    }
    // Truncar al ancho real de la terminal (con margen mínimo); si la barra
    // es más larga, se corta el excedente para que no desborde la línea.
    let max_w = real_term_width().saturating_sub(2);
    if visible_width(&bar) > max_w {
        bar = truncate_visible(&bar, max_w);
    }
    bar
}

/// Trunca una cadena CON códigos ANSI a un ancho VISIBLE máximo.
fn truncate_visible(s: &str, max_visible: usize) -> String {
    let mut out = String::new();
    let mut visible = 0;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            out.push(c);
            for e in chars.by_ref() {
                out.push(e);
                if e == 'm' {
                    break;
                }
            }
        } else {
            if visible >= max_visible {
                // Ya llegamos al límite: cerrar cualquier secuencia ANSI abierta.
                out.push_str("\x1b[0m");
                break;
            }
            visible += 1;
            out.push(c);
        }
    }
    out
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
        format!("{} {}", grad("✻"), grad("dpx · tu mentor senior en la terminal")),
        String::new(),
        format!("{}   {focus}", dim("enfoque")),
        format!("{}      {mode}    {}  {brain}", dim("modo"), dim("cerebro")),
        format!("{}   {cwd}", dim("carpeta")),
    ];

    draw_box(&lines, grad, None);
}

/// Dashboard de proyecto (comando `/panel`): 3 tarjetas de modo con el color
/// de su propia identidad visual (code azul, hack ámbar, learn verde), más un
/// contador de recuerdos y skills del agente. Todo centrado en una caja.
pub fn project_panel(
    has_context: bool,
    plan_progress: Option<(usize, usize)>, // (hechas, total)
    skills_dominados: usize,
    skills_total: usize,
    recuerdos: usize,
    agent_skills: usize,
) {
    // ── colores de identidad FIJA de cada modo (no el tema activo) ──
    let mode_accent = |r: u8, g: u8, b: u8, s: &str| -> String {
        format!("\x1b[38;2;{r};{g};{b}m{s}{RESET}")
    };
    let (cr, cg, cb) = (74u8, 144u8, 226u8); // code — azul
    let (hr, hg, hb) = (224u8, 138u8, 38u8); // hack — ámbar
    let (lr, lg, lb) = (76u8, 175u8, 110u8); // learn — verde

    // ── tarjeta code ──
    let code_header = mode_accent(cr, cg, cb, "code");
    let code_status = if has_context {
        "contexto guardado · listo para construir"
    } else {
        "sin contexto aún · inicia una tarea"
    };

    // ── tarjeta hack ──
    let hack_header = mode_accent(hr, hg, hb, "hack");
    let hack_status = match plan_progress {
        Some((done, total)) => {
            let pct = done.checked_mul(100).and_then(|n| n.checked_div(total)).unwrap_or(0);
            format!("plan {done}/{total} ({pct}%) · en marcha")
        }
        None => "sin plan · lanza una idea en modo hack".to_string(),
    };

    // ── tarjeta learn ──
    let learn_header = mode_accent(lr, lg, lb, "learn");
    let learn_status = if skills_total > 0 {
        let resto = skills_total.saturating_sub(skills_dominados);
        format!("{skills_total} conceptos · {skills_dominados} dominados · {resto} por repasar")
    } else {
        "sin skills aún · explora en modo learn".to_string()
    };

    let lines: Vec<String> = vec![
        grad("panel del proyecto"),
        String::new(),
        format!("  {code_header}   {}", dim(code_status)),
        format!("  {hack_header}   {}", dim(&hack_status)),
        format!("  {learn_header}  {}", dim(&learn_status)),
        String::new(),
        dim(&format!(
            "{} recuerdos · {} skills del agente",
            recuerdos, agent_skills
        )),
    ];

    draw_box(&lines, grad, None);
}

/// Vista del modo hack: resumen del proyecto, enfoque activo, ideas del
/// comité y checklist de tareas. Caja redondeada con degradado, sin emojis.
pub fn hack_panel(
    proyecto: Option<&str>,
    enfoque: &str,
    committee: Option<&str>,
    plan_items: Option<&[(bool, String)]>,
) {
    let short = |s: &str| -> String {
        let s = s.trim();
        if s.chars().count() > 64 {
            format!("{}…", s.chars().take(63).collect::<String>())
        } else {
            s.to_string()
        }
    };

    let mut lines: Vec<String> = Vec::new();

    // Título
    lines.push(grad("modo hack"));
    lines.push(String::new());

    // Proyecto
    let proj = match proyecto {
        Some(p) => short(p),
        None => "proyecto nuevo".to_string(),
    };
    lines.push(format!("{}  {}", grad("proyecto"), dim(&proj)));

    // Enfoque
    lines.push(format!("{}   {}", grad("enfoque"), dim(enfoque)));

    // Ideas del comité (si hay)
    if let Some(c) = committee {
        let summary = committee_summary(c);
        if !summary.is_empty() {
            lines.push(String::new());
            lines.push(grad("── ideas del comité ──"));
            lines.push(dim(&summary));
        }
    }

    // Plan: checklist
    if let Some(items) = plan_items
        && !items.is_empty()
    {
        lines.push(String::new());
        let done = items.iter().filter(|(d, _)| *d).count();
        lines.push(format!(
            "{}  {}",
            grad("plan"),
            dim(&format!("({done}/{})", items.len()))
        ));
        for (is_done, text) in items {
            let t = short(text);
            if *is_done {
                lines.push(format!("  {} {}", acc_checked(), dim(&t)));
            } else {
                lines.push(format!("  {} {}", dim("☐"), dim(&t)));
            }
        }
    }

    // Caja redondeada con degradado
    draw_box(&lines, grad, None);
}

/// Extrae el resumen del comité: primera línea sustancial tras "idea refinada"
/// o "veredicto", acotada a 72 chars.
fn committee_summary(committee_md: &str) -> String {
    let mut found_key = false;
    for line in committee_md.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with("```") {
            continue;
        }
        let lower = t.to_lowercase();
        if lower.contains("idea refinada") || lower.contains("veredicto") {
            found_key = true;
            continue;
        }
        if found_key {
            let s: String = t.chars().take(72).collect();
            if t.chars().count() > 72 {
                return format!("{s}…");
            }
            return s;
        }
    }
    // Fallback: primera línea no vacía que no sea header ni fence
    committee_md
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("```"))
        .map(|l| {
            if l.chars().count() > 72 {
                format!("{}…", l.chars().take(71).collect::<String>())
            } else {
                l.to_string()
            }
        })
        .unwrap_or_default()
}

/// ☑ en color de acento (tarea completada).
fn acc_checked() -> String {
    acc("☑")
}

fn acc(s: &str) -> String {
    accent(s)
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
    println!("\n{} dpx diagnóstico automático {}", warning_color, RESET);
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
        return "límite de cuota de DeepSeek alcanzado. Espera al reset o revisa tu plan."
            .to_string();
    }
    if err.contains("402") || err.contains("Insufficient Balance") || err.contains("Payment Required")
    {
        return "DeepSeek no tiene saldo. Recarga en platform.deepseek.com.".to_string();
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

/// Lista los playbooks curados del proyecto (`skills/*.md`, comando `/skills`).
pub fn skills_list(skills: &[&crate::agent_skill::AgentSkill]) {
    if skills.is_empty() {
        println!("\n{}", accent("⏺ skills del proyecto"));
        println!(
            "  {}",
            dim("aún ninguno · crea un .md en skills/ con los pasos de una tarea que se repita")
        );
        return;
    }
    println!(
        "\n{} {}",
        accent("⏺ skills del proyecto"),
        dim(&format!("· {} playbooks curados · edítalos en skills/", skills.len()))
    );
    println!();
    for s in skills {
        let stack = if s.focus.is_empty() { String::new() } else { format!(" · {}", s.focus) };
        println!("  {} {}", accent(&format!("• {}", s.name)), dim(&stack));
        // "Cuándo se usa" (frase de disparo), si la trae.
        if !s.when.trim().is_empty() {
            println!("     {}", dim(&format!("cuándo: {}", s.when)));
        }
        // Cuerpo recortado a una línea para la vista de lista.
        let body = s.body.replace('\n', " ");
        let body: String = body.chars().take(96).collect();
        if !body.trim().is_empty() {
            println!("     {}", dim(&body));
        }
    }
    println!("\n  {}", dim("dpx las recupera por significado y las aplica cuando encajan con tu tarea"));
}

/// "Doctor" de skills (parte de `/skills`): muestra problemas de salud del
/// catálogo — gatillos vacíos, cuerpos triviales y, lo más útil con muchos
/// skills, pares cuyos gatillos colisionan (dpx dispararía el equivocado).
pub fn skills_doctor(warnings: &[String]) {
    if warnings.is_empty() {
        println!("  {}", dim("⏺ doctor: catálogo sano · sin gatillos en colisión"));
        return;
    }
    println!("\n  {}", accent("⏺ doctor de skills"));
    for w in warnings {
        println!("  {} {}", red("⚠"), dim(w));
    }
    println!("  {}", dim("afina el «cuando» de los que colisionan para que dpx dispare el correcto"));
}

/// Lista de cerebros con su superpoder (comando `/models`).
pub fn models_list(brains: &[BrainRow]) {
    println!("\n{}", accent("⏺ cerebros · /cerebro <id> para cambiar"));
    println!();
    print_brain_rows(brains);
    println!(
        "\n{}",
        dim("● activo · ✓/✗ = API key en tu .env · los tres soportan tool-calling nativo")
    );
}

/// Ayuda de comandos del REPL (comando `/help`). Filtra los comandos según el
/// modo activo: cada fila declara en qué modos aplica (`""` = todos).
pub fn print_help(mode: crate::focus::Mode) {
    use crate::focus::Mode;
    println!("\n{} {}", accent("dpx · comandos"), dim(&format!("· modo {}", mode_id(mode))));
    // (comando, descripción, modos donde aplica — vacío = global).
    let rows: &[(&str, &str, &[Mode])] = &[
        ("/ayuda", "esta ayuda", &[]),
        ("/panel", "dashboard del proyecto: contexto, plan, skills y recuerdos", &[]),
        ("/estado", "estado de la sesión: config, cerebros y memoria", &[]),
        ("/costo", "tokens reales gastados en la sesión + % de caché y costo aprox", &[]),
        ("/presupuesto [N]", "tope de tokens de la sesión (ej. /presupuesto 100k)", &[]),
        ("/modelos", "lista los cerebros y cuál tiene API key", &[]),
        ("/cambios", "muestra todo lo que dpx cambió en la sesión", &[Mode::Code, Mode::Hack]),
        ("/deshacer", "deshace los cambios de archivos del último turno", &[Mode::Code, Mode::Hack]),
        ("/limpiar", "reinicia la conversación (olvida esta sesión)", &[]),
        ("/compactar", "resume la conversación para liberar contexto", &[]),
        ("/comité <idea>", "convoca al comité (4 roles) a evaluar tu idea", &[Mode::Hack]),
        ("/contexto", "muestra la memoria guardada del proyecto", &[]),
        ("/enfoque [id]", "cambia de enfoque (sin id: lista los disponibles)", &[]),
        ("/modo [code|hack|learn]", "cambia de modo (hace · rápido · enseña)", &[]),
        ("/progreso", "tu progreso de aprendizaje: nivel por tema y qué repasar", &[Mode::Learn]),
        ("/temario", "el temario del stack y cuánto llevas", &[Mode::Learn]),
        ("/examen [tema]", "el tutor te interroga para fijar lo aprendido", &[Mode::Learn]),
        ("/recordar <texto>", "guarda algo en memoria de largo plazo", &[]),
        ("/habilidades", "playbooks curados del proyecto (skills/*.md)", &[Mode::Code, Mode::Hack]),
        ("/cerebro [modelo]", "cerebro (dpx usa solo deepseek)", &[]),
        ("/auto [off|all]", "modo autónomo: cambios sin preguntar", &[Mode::Code, Mode::Hack]),
        ("/actualizar", "recompila e instala dpx desde este repo", &[]),
        ("/salir", "termina y guarda el contexto", &[]),
    ];
    for (cmd, desc, modes) in rows {
        if !modes.is_empty() && !modes.contains(&mode) {
            continue; // no aplica a este modo: se oculta
        }
        // Padding sobre el texto plano (dentro del color) para que alinee.
        println!("  {} {}", accent(&format!("{cmd:<18}")), dim(desc));
    }
    println!(
        "\n{}",
        dim("tip · usa @ruta/al/archivo para que dpx lo lea (Tab autocompleta)")
    );
}

/// Identificador corto del modo, para cabeceras de la UI.
fn mode_id(mode: crate::focus::Mode) -> &'static str {
    use crate::focus::Mode;
    match mode {
        Mode::Code => "code",
        Mode::Hack => "hack",
        Mode::Learn => "learn",
    }
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
    println!("  {}", dim("      descansa, rey."));
    println!();
}

/// Spinner animado en una tarea aparte mientras se espera al modelo.
pub struct Spinner {
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl Spinner {
    /// Arranca el spinner con una etiqueta FIJA (ej. "Compactando contexto…").
    pub fn start(label: &str) -> Self {
        Self::rotating(vec![label.to_string()])
    }

    /// Spinner de espera del modelo con verbos rotativos ("Pensando…",
    /// "Razonando…", …) — el equivalente al "Reasoning…" de otras CLIs.
    pub fn thinking() -> Self {
        Self::rotating(THINKING_VERBS.iter().map(|s| s.to_string()).collect())
    }

    /// Igual, pero para las rondas de continuación de un turno agéntico.
    pub fn working() -> Self {
        Self::rotating(WORKING_VERBS.iter().map(|s| s.to_string()).collect())
    }

    /// Núcleo del spinner: rota `labels` (cambia cada ~2s) sobre el frame
    /// animado. Solo anima si la salida es una terminal real; al pipear (no-TTY)
    /// imprime una sola línea estática, evitando spam de frames y *broken pipe*.
    fn rotating(labels: Vec<String>) -> Self {
        if labels.is_empty() {
            return Self { handle: None };
        }
        if !io::stdout().is_terminal() {
            println!("{}", dim(&labels[0]));
            return Self { handle: None };
        }
        let handle = tokio::spawn(async move {
            const FRAMES: [&str; 10] =
                ["⠋", "⠙", "⠚", "⠞", "⠖", "⠦", "⠴", "⠲", "⠳", "⠙"];
            const FRAMES_PER_WORD: usize = 24; // ~2.2s por verbo (a 90ms/frame)
            let start = Instant::now();
            let mut i = 0usize;
            print!("\x1b[?25l"); // ocultar cursor
            loop {
                let secs = start.elapsed().as_secs();
                let label = &labels[(i / FRAMES_PER_WORD) % labels.len()];
                // El glifo PULSA entre los dos colores del degradado (oscilación
                // suave con seno) para que la espera se sienta viva.
                let pulse = ((i as f64) * 0.18).sin() * 0.5 + 0.5;
                let glyph = rgb(FRAMES[i % FRAMES.len()], lerp_rgb(grad_from(), grad_to(), pulse));
                // Adornos que "respiran": un trío de rombos cuyo brillo corre al
                // ritmo del spinner, en el degradado rojo oscuro.
                let spark = ["◆◇◇", "◇◆◇", "◇◇◆", "◇◆◇"][(i / 3) % 4];
                // `\x1b[2K` limpia la línea: los verbos varían de largo y si no,
                // un verbo más corto dejaría residuos del anterior.
                print!(
                    "\r\x1b[2K{glyph} {} {}  {}",
                    grad(label),
                    grad(spark),
                    dim(&format!("{secs}s")),
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

#[cfg(test)]
mod tests {
    use super::committee_summary;

    #[test]
    fn committee_summary_toma_la_linea_tras_la_clave() {
        // Devuelve la primera línea sustancial DESPUÉS de "idea refinada"/"veredicto".
        let md = "# Comité\n\n## Idea refinada\nUn CLI mentor por stack con focus packs.\n";
        assert_eq!(committee_summary(md), "Un CLI mentor por stack con focus packs.");
        let md2 = "Veredicto:\nArrancar por el MVP de Spring Boot.";
        assert_eq!(committee_summary(md2), "Arrancar por el MVP de Spring Boot.");
    }

    #[test]
    fn committee_summary_sin_clave_usa_la_primera_linea_real() {
        // Fallback: ignora headers y toma la primera línea con contenido real.
        let md = "# titulo\n\nPrimera idea concreta.";
        assert_eq!(committee_summary(md), "Primera idea concreta.");
        assert_eq!(committee_summary(""), "");
    }
}
