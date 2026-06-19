//! Helpers de comandos y modo, extraídos de `chat`: mapeo español→canónico,
//! disponibilidad de comandos por modo, etiquetas legibles del modo y acorte
//! estético del path del proyecto. Lógica leaf, sin estado.

use crate::focus::Mode;
use crate::ui;

/// Traduce el nombre de comando en español a su clave canónica (en inglés) que
/// usa el dispatcher. Lo no listado se devuelve tal cual.
pub(crate) fn canonical_cmd(name: &str) -> &str {
    match name {
        "ayuda" => "help",
        "estado" => "status",
        "costo" | "coste" => "cost",
        "presupuesto" => "budget",
        "modelos" => "models",
        "cambios" => "diff",
        "deshacer" => "undo",
        "limpiar" => "clear",
        "compactar" => "compact",
        "contexto" => "context",
        "enfoque" => "focus",
        "modo" => "mode",
        "examen" => "quiz",
        "habilidades" => "skills",
        "cerebro" => "brain",
        "comité" => "comite",
        "actualizar" => "update",
        other => other,
    }
}

/// ¿Está disponible el comando `name` (clave canónica) en el modo activo? Los
/// comandos no listados aquí son GLOBALES (los tres modos los tienen). El
/// reparto: `auto` solo en code/hack (learn no escribe), `comite`/`brainstorm`
/// solo en hack (lluvia de ideas), `quiz`/`progreso`/`temario` solo en learn.
pub(crate) fn command_in_mode(name: &str, mode: Mode) -> bool {
    match name {
        "auto" => matches!(mode, Mode::Code | Mode::Hack),
        "comite" | "brainstorm" => mode == Mode::Hack,
        "quiz" | "progreso" | "temario" | "evaluar" | "revisar" => mode == Mode::Learn,
        _ => true,
    }
}

/// Avisa (con una pista) que un comando no pertenece al modo activo.
pub(crate) fn reject_command(name: &str, mode: Mode) {
    let needed = match name {
        "auto" => "code o hack",
        "comite" | "brainstorm" => "hack",
        "quiz" | "progreso" | "temario" | "evaluar" | "revisar" => "learn",
        _ => "otro",
    };
    println!(
        "{}",
        ui::dim(&format!(
            "/{name} no está en modo {} · es de modo {needed} — cambia con /modo {}",
            mode.name(),
            needed.split(' ').next().unwrap_or(needed)
        ))
    );
}

pub(crate) fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Code => "code (agente autónomo)",
        Mode::Hack => "hack (construir rápido con criterio)",
        Mode::Learn => "learn (tutor socrático)",
    }
}

/// Acorta el path del proyecto usando `~` para el home (solo estético).
pub(crate) fn short_path(cwd: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = cwd.strip_prefix(&home)
    {
        // Normalizamos a '/' para que se vea consistente en Windows.
        return format!("~/{}", rest.display().to_string().replace('\\', "/"));
    }
    cwd.display().to_string().replace('\\', "/")
}
