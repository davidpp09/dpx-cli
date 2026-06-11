//! Rutas y configuración global del CLI (`~/.dpx`).
//!
//! Por ahora solo resolvemos directorios. La config del usuario
//! (preferencias, modelo por defecto) vivirá aquí más adelante.

use std::path::PathBuf;

use anyhow::{Context, Result};

/// Directorio home del usuario.
pub fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().context("No se pudo determinar el directorio home del usuario")
}

/// Directorio global del CLI: `~/.dpx`.
#[allow(dead_code)] // se usará cuando guardemos config/preferencias globales
pub fn global_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".dpx"))
}
