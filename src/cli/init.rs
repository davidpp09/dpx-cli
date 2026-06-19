//! Wizard `dpx init`: asiste al usuario para configurar el proyecto.
//!
//! El flujo es:
//! 1. Saludo y detección de stack (por archivos raíz como en `fs::detect_stack`).
//! 2. Confirmación o cambio manual del focus pack.
//! 3. Selección de cerebro por defecto (deepseek), con indicación de
//!    si cada uno tiene API key configurada.
//! 4. Modo (pro/hack).
//! 5. Modo autónomo: off / reads / writes / all.
//! 6. Guarda `.dpx/config.toml` y muestra resumen final.
//!
//! Es interactivo: pregunta opción por opción, con defaults sensatos basados
//! en la detección automática. No requiere el editor raw (usa stdin directo).

use std::io::{self, Write};
use std::path::Path;

use anyhow::{Context, Result};

use crate::config::ProjectConfig;
use crate::fs;
use crate::focus;
use crate::agent::BRAIN_LABEL;
use crate::focus::Mode;
use crate::cli::AutoMode;
use crate::ui;

/// Configuración de PRIMER ARRANQUE: se dispara cuando se abre `dpx <modo>` en
/// un proyecto que aún no tiene `.dpx/`. Es como `dpx init` pero el MODO ya lo
/// fijó el subcomando (no se pregunta), y al terminar la sesión continúa.
/// Devuelve la config guardada para que el REPL la adopte (focus + auto).
pub fn onboarding(cwd: &Path, mode: Mode) -> Result<ProjectConfig> {
    println!();
    println!("  {} primer arranque · vamos a configurar este proyecto", ui::accent("✻"));
    println!(
        "  {}",
        ui::dim("aún no hay .dpx/ aquí. Dos pasos rápidos (Enter acepta el default).")
    );
    let mode_human = match mode {
        Mode::Code => "code (agente autónomo)",
        Mode::Hack => "hack (construir rápido con criterio)",
        Mode::Learn => "learn (tutor socrático)",
    };
    println!("  {}   {}", ui::dim("modo"), ui::accent(mode_human));

    let stack = fs::detect_stack(cwd);
    let focus_id = step_focus(stack)?;
    let auto = step_auto()?;

    let config = ProjectConfig {
        focus: focus_id,
        brain: "deepseek".to_string(),
        mode: mode.name().to_string(),
        auto,
    };
    config.save(cwd).context("No se pudo guardar la configuración")?;

    // Template de hooks: un ejemplo comentado para que el usuario sepa cómo arrancar.
    let hooks_toml = cwd.join(".dpx").join("hooks.toml");
    if !hooks_toml.exists() {
        let _ = std::fs::write(
            &hooks_toml,
            "# hooks automáticos de dpx (opcional)\n\
             # Cada [[hooks]] define un comando que se ejecuta automáticamente ante\n\
             # un evento del ciclo de vida: PreToolUse, PostToolUse, OnSessionStart,\n\
             # OnSessionEnd, PreCommit.\n\
             #\n\
             # Ejemplos:\n\
             #\n\
             # [[hooks]]\n\
             # event = \"PreCommit\"\n\
             # command = \"cargo test\"\n\
             #\n\
             # [[hooks]]\n\
             # event = \"PostToolUse\"\n\
             # tools = [\"write_file\", \"edit_file\"]\n\
             # command = \"cargo fmt\"\n\
             ",
        );
    }

    println!("\n  {}", ui::dim("⎿ configuración guardada en .dpx/config.toml · ya puedes empezar"));
    Ok(config)
}

/// Arranca el asistente interactivo de inicialización.
pub fn run(cwd: &Path) -> Result<()> {
    ui::logo();
    println!();
    println!("  {} dpx init — prepara este proyecto para el mentor", ui::accent("✻"));
    println!();
    println!("  El asistente va a configurar el proyecto paso a paso.");
    println!("  En cada paso puedes aceptar el default [entre corchetes] pulsando Enter,");
    println!("  o escribir tu propio valor.");
    println!();

    let stack = fs::detect_stack(cwd);
    let focus_id = step_focus(stack)?;
    let mode = step_mode()?;
    let auto = step_auto()?;

    let config = ProjectConfig {
        focus: focus_id.clone(),
        brain: "deepseek".to_string(),
        mode: mode.name().to_string(),
        auto,
    };

    config.save(cwd).context("No se pudo guardar la configuración")?;

    // Resumen final
    println!();
    println!("{}", ui::accent("╭─────────────────────────────────────────────╮"));
    println!("{}", ui::accent("│  dpx init — configuración guardada          │"));
    println!("{}", ui::accent("╰─────────────────────────────────────────────╯"));
    let focus_display = focus_id
        .as_deref()
        .map(|id| focus::display_name(Some(id)))
        .unwrap_or("(general, sin stack específico)");
    println!("  {}   {}", ui::dim("enfoque"), focus_display);
    println!("  {}    {}", ui::dim("cerebro"), BRAIN_LABEL);
    let mode_human = match mode {
        Mode::Code => "code (agente autónomo)",
        Mode::Hack => "hack (construir rápido con criterio)",
        Mode::Learn => "learn (tutor socrático)",
    };
    println!("  {}       {}", ui::dim("modo"), mode_human);
    let auto_label = AutoMode::parse(&config.auto).map(|a| a.label()).unwrap_or("off");
    println!("  {}   {}   {}", ui::dim("auto"), ui::accent(auto_label), ui::dim("· /auto para cambiar"));
    println!();
    println!("  {}", ui::dim("Puedes cambiarlo en cualquier momento desde el REPL"));
    println!("  {}  {}", ui::dim("con /focus, /mode, /auto, o volviendo a ejecutar"), ui::accent("dpx init"));
    println!();

    Ok(())
}

/// Lee una línea de stdin, recortada, o devuelve el default si está vacía.
/// Sin TTY (stdin pipeado, dpx headless): no consumir stdin — sería el mensaje
/// del usuario. Devolvemos vacío para que `read_with_default` use el default.
fn read_line(prompt: &str) -> io::Result<String> {
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        return Ok(String::new());
    }
    print!("{}", prompt);
    io::stdout().flush()?;
    let mut buf = String::new();
    io::stdin().read_line(&mut buf)?;
    let trimmed = buf.trim().to_string();
    Ok(trimmed)
}

/// Lee una línea y si está vacía devuelve el default.
fn read_with_default(prompt: &str, default: &str) -> io::Result<String> {
    let line = read_line(prompt)?;
    if line.is_empty() { Ok(default.to_string()) } else { Ok(line) }
}

/// Paso 1: focus pack.
fn step_focus(detected: Option<&str>) -> Result<Option<String>> {
    println!();
    println!("{}", ui::accent("⏺ 1. Stack del proyecto"));

    let catalog = focus::catalog();
    let default_id: &str = detected.unwrap_or("");

    match detected {
        Some(id) => {
            let name = focus::display_name(Some(id));
            println!("  Detectado: {} ({})", ui::accent(name), ui::dim(id));
        }
        None => {
            println!("  {}", ui::dim("No se detectó un stack conocido."));
        }
    }
    println!();

    // Mostramos los disponibles
    println!("  {}", ui::dim("Enfoques disponibles:"));
    for f in &catalog {
        let marker = if detected == Some(f.id) { ui::accent("● ") } else { ui::dim("○ ") };
        println!("    {}{:<14} {}", marker, f.id, ui::dim(f.tagline));
    }
    println!("    {}  {:<14} {}", ui::dim("○ "), "(vacío)", ui::dim("mentor genérico (sin stack)"));
    println!();

    let default = if default_id.is_empty() { "(vacío)".to_string() } else { default_id.to_string() };
    let answer = read_with_default(&format!("  Enfoque [{}]: ", default), &default)?;

    let chosen = if answer.is_empty() || answer == "(vacío)" {
        None
    } else if catalog.iter().any(|f| f.id == answer) {
        Some(answer)
    } else {
        eprintln!(
            "  {} Enfoque '{}' no reconocido. Se usará mentor genérico.",
            ui::dim("⚠"),
            answer
        );
        None
    };

    Ok(chosen)
}

/// Paso 2: modo.
fn step_mode() -> Result<Mode> {
    println!();
    println!("{}", ui::accent("⏺ 3. Modo de trabajo"));

    println!("  {} {}", ui::accent("code"),  ui::dim("agente autónomo: escribe, ejecuta, itera y deja funcionando"));
    println!("  {} {}", ui::accent("hack"), ui::dim("construir rápido CON criterio: demo sólida, mínimo boilerplate"));
    println!("  {}{}", ui::accent("learn "), ui::dim("tutor socrático: te enseña conceptos y arquitectura, tú escribes"));
    println!();

    let answer = read_with_default("  Modo [code]: ", "code")?;
    let chosen = Mode::parse(&answer).unwrap_or(Mode::Code);

    Ok(chosen)
}

/// Paso 4: modo autónomo granular.
fn step_auto() -> Result<String> {
    println!();
    println!("{}", ui::accent("⏺ 4. Modo autónomo"));
    println!("  {}  {}", ui::accent("off"),    ui::dim("cada cambio y comando pide confirmación"));
    println!("  {} {}", ui::accent("reads"),   ui::dim("auto-extiende rondas (investigación larga sin preguntar)"));
    println!("  {} {}", ui::accent("writes"),  ui::dim("lo anterior + aplica escrituras/ediciones sin confirmación"));
    println!("  {}   {}", ui::accent("all"),    ui::dim("todo: escrituras y comandos seguros sin preguntar"));
    println!();
    println!("  {}", ui::dim("Los comandos peligrosos y los guards anti-truncado preguntan SIEMPRE."));
    println!();

    let answer = read_with_default("  Nivel [off]: ", "off")?;
    Ok(AutoMode::parse(&answer).map(|a| a.label().to_string()).unwrap_or_else(|| "off".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_son_consistentes() {
        let cfg = ProjectConfig::default();
        assert_eq!(cfg.brain, "deepseek");
        assert_eq!(cfg.mode, "code");
        assert!(cfg.focus.is_none());
        assert_eq!(cfg.auto, "off");
    }
}
