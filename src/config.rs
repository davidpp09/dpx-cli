//! Rutas y configuración global del CLI (`~/.dpx` y `.dpx/config.toml`).
//!
//! El wizard `dpx init` crea `.dpx/config.toml` con los defaults detectados.
//! La sesión `chat` lo lee para saber qué focus/brain/mode/auto usar si no se
//! pasaron explícitamente por CLI.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Deserializer, Serialize};

/// Configuración persistente del proyecto que crea `dpx init`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// Focus pack detectado o elegido por el usuario (spring-boot, react, etc.).
    /// `None` si el usuario eligió mentor genérico.
    pub focus: Option<String>,
    /// Modelo por defecto ("deepseek").
    #[serde(default = "default_brain")]
    pub brain: String,
    /// Modo por defecto ("pro" o "hack").
    #[serde(default = "default_mode")]
    pub mode: String,
    /// Modo autónomo por defecto: "off", "reads", "writes", "all".
    /// Backward compat: si el TOML tiene `auto = false`/`true`, lo convierte.
    #[serde(deserialize_with = "deserialize_auto", default = "default_auto")]
    pub auto: String,
}

fn default_brain() -> String {
    "deepseek".to_string()
}
fn default_mode() -> String {
    "pro".to_string()
}
fn default_auto() -> String {
    "off".to_string()
}

/// Deserializa `auto` desde TOML: acepta string ("off", "reads", …) o bool
/// (true → "all", false → "off"). Así los `.dpx/config.toml` viejos no rompen.
fn deserialize_auto<'de, D>(de: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    use serde::de;

    struct AutoVisitor;
    impl<'de> de::Visitor<'de> for AutoVisitor {
        type Value = String;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str(r#"un nivel de auto ("off", "reads", "writes", "all") o bool"#)
        }

        fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
            Ok(s.to_string())
        }

        fn visit_bool<E: de::Error>(self, b: bool) -> Result<Self::Value, E> {
            Ok(if b { "all".to_string() } else { "off".to_string() })
        }
    }

    de.deserialize_any(AutoVisitor)
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            focus: None,
            brain: default_brain(),
            mode: default_mode(),
            auto: default_auto(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_son_consistentes() {
        let cfg = ProjectConfig::default();
        assert_eq!(cfg.brain, "deepseek");
        assert_eq!(cfg.mode, "pro");
        assert!(cfg.focus.is_none());
        assert_eq!(cfg.auto, "off");
    }

    #[test]
    fn backward_compat_auto_bool_false() {
        let toml = r#"
            brain = "deepseek"
            auto = false
        "#;
        let cfg: ProjectConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.auto, "off");
    }

    #[test]
    fn backward_compat_auto_bool_true() {
        let toml = r#"
            brain = "deepseek"
            auto = true
        "#;
        let cfg: ProjectConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.auto, "all");
    }

    #[test]
    fn auto_string_se_respeta() {
        let toml = r#"
            brain = "deepseek"
            auto = "writes"
        "#;
        let cfg: ProjectConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.auto, "writes");
    }

    #[test]
    fn auto_por_defecto_cuando_no_esta() {
        let toml = r#"
            brain = "deepseek"
        "#;
        let cfg: ProjectConfig = toml::from_str(toml).unwrap();
        assert_eq!(cfg.auto, "off");
    }
}
