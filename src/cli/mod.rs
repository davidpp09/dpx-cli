//! Definición de la CLI con `clap` y despacho de comandos.

mod chat;
mod commands;
mod editor;
pub mod hooks;
mod init;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::agent::Brain;
use crate::focus::{self, Mode};

// ── AutoMode granular ──────────────────────────────────────────────
/// Nivel de autonomía del modo auto. Acumulativo: `All` ⊃ `Writes` ⊃ `Reads` ⊃ `Off`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoMode {
    /// Nada automático: cada acción pide confirmación.
    Off,
    /// Lecturas y búsquedas (ya libres) + auto-extiende rondas sin preguntar.
    Reads,
    /// Lo anterior + escrituras/ediciones sin preguntar (los guards siguen).
    Writes,
    /// Todo: lecturas, escrituras, comandos seguros + auto-extiende rondas.
    All,
}

impl AutoMode {
    pub fn label(&self) -> &'static str {
        match self {
            AutoMode::Off => "off",
            AutoMode::Reads => "reads",
            AutoMode::Writes => "writes",
            AutoMode::All => "all",
        }
    }

    /// ¿Auto-extiende el presupuesto de rondas sin preguntar?
    pub fn extends(&self) -> bool {
        !matches!(self, AutoMode::Off)
    }

    /// ¿Aplica escrituras/ediciones sin confirmación?
    /// Los guards (anti-truncado, big-rewrite) preguntan SIEMPRE.
    pub fn writes(&self) -> bool {
        matches!(self, AutoMode::Writes | AutoMode::All)
    }

    /// ¿Ejecuta comandos seguros sin confirmación?
    /// Los peligrosos/prohibidos ignoran esto: sus puertas son incondicionales.
    pub fn commands(&self) -> bool {
        matches!(self, AutoMode::All)
    }

    /// Parsea desde string: CLI arg, config toml, o comando `/auto`.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "off" | "false" | "no" => Some(AutoMode::Off),
            "reads" | "read" | "r" => Some(AutoMode::Reads),
            "writes" | "write" | "w" => Some(AutoMode::Writes),
            "all" | "on" | "true" | "yes" | "full" => Some(AutoMode::All),
            _ => None,
        }
    }
}

#[derive(Parser)]
#[command(
    name = "dpx",
    version,
    about = "Tu mentor senior de desarrollo, en la terminal.",
    long_about = "dpx es un mentor de ingeniería que te enseña mientras programas: \
                  explica el porqué de cada decisión y te deja a ti escribir el código. \
                  Se enfoca según tu stack (primer enfoque: Spring Boot)."
)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

/// Opciones comunes a los tres modos (`code`/`hack`/`learn`). El MODO lo fija
/// el subcomando; aquí solo van enfoque, cerebro y autonomía.
#[derive(clap::Args)]
struct ModeArgs {
    /// Enfoque (focus pack) a usar. Sin él, dpx detecta el stack del proyecto.
    #[arg(short, long)]
    focus: Option<String>,

    /// Cerebro (modelo) a usar. DeepSeek por defecto (el más capaz).
    #[arg(short, long, value_enum)]
    brain: Option<Brain>,

    /// Modo autónomo: `off`, `reads` (auto-extiende rondas), `writes` (o
    /// +auto-apply), `all` (o +comandos seguros). `/auto` en el REPL.
    /// Acepta `--auto` (= `all`), `--auto writes`, etc.; o ausente (usa config).
    #[arg(long, num_args = 0..=1, default_missing_value = "all")]
    auto: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Modo CODE 🤖: agente autónomo que hace el trabajo e itera (escribe,
    /// ejecuta, corrige) hasta dejarlo robusto.
    Code(ModeArgs),

    /// Modo HACK ⚡: construye rápido pero con criterio — demo sólida, mínimo
    /// boilerplate, código correcto que corre ya.
    Hack(ModeArgs),

    /// Modo LEARN 🎓: tutor socrático que te hace pensar y te enseña conceptos,
    /// patrones y arquitectura. Tú escribes el código, él te guía.
    Learn(ModeArgs),

    /// Lista los enfoques (focus packs) disponibles.
    Focus,

    /// Asistente de inicialización: configura el proyecto paso a paso.
    /// Crea `.dpx/config.toml` con el stack, cerebro y modo por defecto.
    Init,
}

impl Cli {
    pub async fn run(self) -> Result<()> {
        // Config del proyecto: se mergea con los CLI flags (éstos ganan).
        let cwd = std::env::current_dir()?;
        let proj_cfg = crate::config::ProjectConfig::load(&cwd).unwrap_or_default();

        // Modo por defecto (cuando se abre `dpx` a secas): de la config, o `code`.
        let default_mode = Mode::parse(&proj_cfg.mode).unwrap_or(Mode::Code);
        let resolve_brain = |cli: Option<Brain>| {
            cli.unwrap_or_else(|| Brain::parse(&proj_cfg.brain).unwrap_or(Brain::Deepseek))
        };
        let resolve_auto = |cli: Option<String>| {
            cli.and_then(|s| AutoMode::parse(&s))
                .unwrap_or_else(|| AutoMode::parse(&proj_cfg.auto).unwrap_or(AutoMode::Off))
        };
        // focus: CLI flag gana; si es None, la config; si también None, detecta.
        let resolve_focus = |cli: Option<String>| cli.or_else(|| proj_cfg.focus.clone());

        // Lanza el REPL con el modo del subcomando y las opciones comunes.
        let launch = |mode: Mode, a: ModeArgs| {
            chat::run(
                resolve_focus(a.focus),
                mode,
                resolve_brain(a.brain),
                resolve_auto(a.auto),
            )
        };

        match self.command {
            // `dpx` a secas abre el modo por defecto de la config.
            None => {
                chat::run(
                    resolve_focus(None),
                    default_mode,
                    resolve_brain(None),
                    resolve_auto(None),
                )
                .await
            }
            Some(Commands::Code(a)) => launch(Mode::Code, a).await,
            Some(Commands::Hack(a)) => launch(Mode::Hack, a).await,
            Some(Commands::Learn(a)) => launch(Mode::Learn, a).await,
            Some(Commands::Focus) => {
                focus::print_catalog();
                Ok(())
            }
            Some(Commands::Init) => init::run(&cwd),
        }
    }
}
