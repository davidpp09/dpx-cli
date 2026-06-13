//! Definición de la CLI con `clap` y despacho de comandos.

mod chat;
mod commands;
mod editor;
pub mod hooks;
mod init;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::agent::Brain;
use crate::focus::{self, Mode, Persona};

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

#[derive(Subcommand)]
enum Commands {
    /// Inicia una sesión conversacional con el mentor en la carpeta actual.
    Chat {
        /// Enfoque (focus pack) a usar. Sin él, dpx detecta el stack del proyecto.
        #[arg(short, long)]
        focus: Option<String>,

        /// Actitud del mentor: `pro` (metódico) o `hack` (rápido, para hackathones).
        #[arg(short, long, value_enum)]
        mode: Option<Mode>,

        /// Cerebro (modelo) a usar. DeepSeek por defecto (el más capaz).
        #[arg(short, long, value_enum)]
        brain: Option<Brain>,

        /// Modo autónomo: `off`, `reads` (auto-extiende rondas), `writes` (o
        /// +auto-apply), `all` (o +comandos seguros). `/auto` en el REPL.
        /// Acepta `--auto` (= `all`), `--auto writes`, etc.; o ausente (usa config).
        #[arg(long, num_args = 0..=1, default_missing_value = "all")]
        auto: Option<String>,
    },

    /// Agente autónomo: hace el trabajo e itera (escribe, ejecuta, corrige).
    Code {
        /// Enfoque (focus pack) a usar. Sin él, dpx detecta el stack del proyecto.
        #[arg(short, long)]
        focus: Option<String>,

        /// Actitud: `pro` (metódico) o `hack` (rápido).
        #[arg(short, long, value_enum)]
        mode: Option<Mode>,

        /// Cerebro (modelo) a usar.
        #[arg(short, long, value_enum)]
        brain: Option<Brain>,

        /// Modo autónomo: `off`, `reads`, `writes`, `all`. Misma semántica que en Chat.
        /// Acepta `--auto` (= `all`), `--auto writes`, etc.
        #[arg(long, num_args = 0..=1, default_missing_value = "all")]
        auto: Option<String>,
    },

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

        // Helper: CLI flag → si es None, usa la config.
        let resolve_mode = |cli: Option<Mode>| {
            cli.unwrap_or_else(|| match proj_cfg.mode.as_str() {
                "hack" => Mode::Hack,
                _ => Mode::Pro,
            })
        };
        let resolve_brain = |cli: Option<Brain>| {
            cli.unwrap_or_else(|| Brain::parse(&proj_cfg.brain).unwrap_or(Brain::Deepseek))
        };
        let resolve_auto = |cli: Option<String>| {
            cli.and_then(|s| AutoMode::parse(&s))
                .unwrap_or_else(|| AutoMode::parse(&proj_cfg.auto).unwrap_or(AutoMode::Off))
        };
        // focus: CLI flag gana; si es None, la config; si también None, detecta.
        let resolve_focus = |cli: Option<String>| cli.or_else(|| proj_cfg.focus.clone());

        match self.command {
            // `dpx` a secas abre el mentor: config o detección de stack.
            None => {
                let focus = resolve_focus(None);
                chat::run(focus, resolve_mode(None), resolve_brain(None), Persona::Mentor, resolve_auto(None)).await
            }
            Some(Commands::Chat { focus, mode, brain, auto }) => {
                chat::run(resolve_focus(focus), resolve_mode(mode), resolve_brain(brain), Persona::Mentor, resolve_auto(auto)).await
            }
            Some(Commands::Code { focus, mode, brain, auto }) => {
                chat::run(resolve_focus(focus), resolve_mode(mode), resolve_brain(brain), Persona::Code, resolve_auto(auto)).await
            }
            Some(Commands::Focus) => {
                focus::print_catalog();
                Ok(())
            }
            Some(Commands::Init) => {
                init::run(&cwd)
            }
        }
    }
}
