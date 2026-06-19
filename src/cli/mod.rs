//! Definicion de la CLI con `clap` y despacho de comandos.

mod chat;
mod editor;
mod init;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::focus::{self, Mode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoMode {
    Off,
    Reads,
    Writes,
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

    pub fn extends(&self) -> bool {
        !matches!(self, AutoMode::Off)
    }

    pub fn writes(&self) -> bool {
        matches!(self, AutoMode::Writes | AutoMode::All)
    }

    pub fn commands(&self) -> bool {
        matches!(self, AutoMode::All)
    }

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
    long_about = "dpx es un mentor de ingenieria que te ensena mientras programas: \
                  explica el porque de cada decision y te deja a ti escribir el codigo. \
                  Se enfoca segun tu stack (primer enfoque: Spring Boot)."
)]
pub struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(clap::Args)]
struct ModeArgs {
    #[arg(short, long)]
    focus: Option<String>,

    #[arg(long, num_args = 0..=1, default_missing_value = "all")]
    auto: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Modo CODE: agente autonomo que hace el trabajo e itera.
    Code(ModeArgs),
    /// Modo HACK: construye rapido pero con criterio.
    Hack(ModeArgs),
    /// Modo LEARN: tutor socratico, tu escribes el codigo.
    Learn(ModeArgs),
    /// Lista los enfoques (focus packs) disponibles.
    Focus,
    /// Asistente de inicializacion: crea .dpx/config.toml.
    Init,
}

impl Cli {
    pub async fn run(self) -> Result<()> {
        let cwd = std::env::current_dir()?;
        let proj_cfg = crate::config::ProjectConfig::load(&cwd).unwrap_or_default();

        let default_mode = Mode::parse(&proj_cfg.mode).unwrap_or(Mode::Code);

        let resolve_auto = |cli: Option<String>| {
            cli.and_then(|s| AutoMode::parse(&s))
                .unwrap_or_else(|| AutoMode::parse(&proj_cfg.auto).unwrap_or(AutoMode::Off))
        };
        let resolve_focus = |cli: Option<String>| cli.or_else(|| proj_cfg.focus.clone());

        let launch = |mode: Mode, a: ModeArgs| {
            chat::run(resolve_focus(a.focus), mode, resolve_auto(a.auto))
        };

        match self.command {
            None => {
                chat::run(resolve_focus(None), default_mode, resolve_auto(None)).await
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
