//! Definición de la CLI con `clap` y despacho de comandos.

mod chat;
mod editor;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::agent::Brain;
use crate::focus::{self, Mode, Persona};

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
        #[arg(short, long, value_enum, default_value_t = Mode::Pro)]
        mode: Mode,

        /// Cerebro (modelo) a usar. DeepSeek por defecto (el más capaz).
        #[arg(short, long, value_enum, default_value_t = Brain::Deepseek)]
        brain: Brain,
    },

    /// Agente autónomo: hace el trabajo e itera (escribe, ejecuta, corrige).
    Code {
        /// Enfoque (focus pack) a usar. Sin él, dpx detecta el stack del proyecto.
        #[arg(short, long)]
        focus: Option<String>,

        /// Actitud: `pro` (metódico) o `hack` (rápido).
        #[arg(short, long, value_enum, default_value_t = Mode::Pro)]
        mode: Mode,

        /// Cerebro (modelo) a usar.
        #[arg(short, long, value_enum, default_value_t = Brain::Deepseek)]
        brain: Brain,
    },

    /// Lista los enfoques (focus packs) disponibles.
    Focus,
}

impl Cli {
    pub async fn run(self) -> Result<()> {
        match self.command {
            // `dpx` a secas abre el mentor: el arranque detecta el stack del proyecto.
            None => chat::run(None, Mode::Pro, Brain::Deepseek, Persona::Mentor).await,
            Some(Commands::Chat { focus, mode, brain }) => {
                chat::run(focus, mode, brain, Persona::Mentor).await
            }
            Some(Commands::Code { focus, mode, brain }) => {
                chat::run(focus, mode, brain, Persona::Code).await
            }
            Some(Commands::Focus) => {
                focus::print_catalog();
                Ok(())
            }
        }
    }
}
