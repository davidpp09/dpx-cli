//! dpx — tu mentor senior de desarrollo, en la terminal.
//!
//! No es un generador de scaffolding ni un autocompletador. Es un agente que
//! enseña, explica el porqué de cada decisión y te deja a ti escribir el código.
//! Se hiper-enfoca según el stack en el que trabajas (el primer enfoque es
//! Spring Boot). Debajo, un Model Router reparte el trabajo entre los modelos
//! disponibles, con DeepSeek como cerebro.

mod agent;
mod cli;
mod config;
mod focus;
mod fs;
mod session;
mod skill;
mod token;
mod ui;

use clap::Parser;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    if let Some(home) = dirs::home_dir() {
        dotenvy::from_path(home.join(".dpx").join(".env")).ok();
    }

    cli::Cli::parse().run().await
}
