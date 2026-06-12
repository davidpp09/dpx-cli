//! Rutas y configuración global del CLI (`~/.dpx` y `.dpx/config.toml`).
//!
//! El wizard `dpx init` crea `.dpx/config.toml` con los defaults detectados.
//! La sesión `chat` lo lee para saber qué focus/brain/mode usar si no se
//! pasaron explícitamente por CLI.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Configuración persistente del proyecto que crea `dpx init`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// Focus pack detectado o elegido por el usuario (spring-boot, react, etc.).
    /// `None` si el usuario eligió mentor genérico.
    pub focus: Option<String>,
    /// Modelo por defecto ("deepseek", "kimi", "qwen").
    #[serde(default = "default_brain")]
    pub brain: String,
    /// Modo por defecto ("pro" o "hack").
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Modo autónomo por defecto.
    #[serde(default)]
    pub auto: bool,
}

fn default_brain() -> String {
    "deepseek".to_string()
}
fn default_mode() -> String {
    "pro".to_string()
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            focus: None,
            brain: default_brain(),
            mode: default_mode(),
            auto: false,
        }
    }
}

impl ProjectConfig {
    /// Ruta del archivo de configuración dentro de `.dpx/`.
    pub fn path(cwd: &Path) -> PathBuf {
        cwd.join(".dpx").join("config.toml")
    }

    /// Carga la configuración del proyecto si existe.
    /// Si no existe, devuelve los defaults (no es error).
    pub fn load(cwd: &Path) -> Result<Self> {
        let path = Self::path(cwd);
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = fs::read_to_string(&path)
            .with_context(|| format!("No se pudo leer {}", path.display()))?;
        toml::from_str(&data)
            .with_context(|| format!("No se pudo parsear {}", path.display()))
    }

    /// Guarda la configuración en `.dpx/config.toml`.
    pub fn save(&self, cwd: &Path) -> Result<()> {
        let path = Self::path(cwd);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("No se pudo crear {}", parent.display()))?;
        }
        let data = toml::to_string_pretty(self)
            .context("No se pudo serializar la config")?;
        fs::write(&path, &data)
            .with_context(|| format!("No se pudo escribir {}", path.display()))?;
        Ok(())
    }
}

/// Directorio home del usuario.
pub fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().context("No se pudo determinar el directorio home del usuario")
}

/// Directorio global del CLI: `~/.dpx`.
#[allow(dead_code)]
pub fn global_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".dpx"))
}
