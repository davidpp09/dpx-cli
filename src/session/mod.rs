//! Persistencia del proyecto en `.dpx/` (dentro de la carpeta de trabajo).
//!
//! Estructura:
//!
//! ```text
//! <proyecto>/.dpx/
//! ├── context.md            ← memoria viva: estado + aprendizaje + próximos pasos
//! └── sessions/
//!     └── 20260608-141230.jsonl   ← transcripción cruda (checkpoint por turno)
//! ```
//!
//! El `context.md` se (re)genera al cerrar la sesión limpiamente, resumiendo la
//! conversación. La transcripción `.jsonl` se escribe turno a turno, así que un
//! cierre brutal (matar la terminal) pierde como mucho el último resumen, nunca
//! lo conversado.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::Local;

use crate::agent::ModelRouter;

/// Un turno de la conversación, para checkpoint y resumen.
pub struct Turn {
    pub role: &'static str,
    pub text: String,
}

/// Maneja el directorio `.dpx/` del proyecto actual.
pub struct ProjectStore {
    root: PathBuf,
    session_file: PathBuf,
}

impl ProjectStore {
    /// Crea (si no existe) `.dpx/` en `cwd` y abre un archivo de sesión nuevo.
    pub fn init(cwd: &Path) -> Result<Self> {
        let root = cwd.join(".dpx");
        let sessions = root.join("sessions");
        fs::create_dir_all(&sessions)
            .with_context(|| format!("No se pudo crear {}", sessions.display()))?;

        let stamp = Local::now().format("%Y%m%d-%H%M%S");
        let session_file = sessions.join(format!("{stamp}.jsonl"));

        Ok(Self { root, session_file })
    }

    /// Memoria del proyecto de sesiones anteriores, si existe.
    pub fn prior_context(&self) -> Option<String> {
        fs::read_to_string(self.root.join("context.md")).ok()
    }

    /// Añade un turno a la transcripción de la sesión (una línea JSON).
    pub fn checkpoint(&self, role: &str, text: &str) -> Result<()> {
        let line = serde_json::json!({
            "ts": Local::now().to_rfc3339(),
            "role": role,
            "text": text,
        });
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.session_file)
            .with_context(|| format!("No se pudo abrir {}", self.session_file.display()))?;
        writeln!(file, "{line}").context("No se pudo escribir el checkpoint de la sesión")?;
        Ok(())
    }

    /// Escribe el contexto del proyecto (memoria viva).
    pub fn write_context(&self, markdown: &str) -> Result<()> {
        let path = self.root.join("context.md");
        fs::write(&path, markdown)
            .with_context(|| format!("No se pudo escribir {}", path.display()))?;
        Ok(())
    }

    /// Lee el plan pendiente de la sesión anterior, si existe.
    pub fn read_plan(&self) -> Option<String> {
        fs::read_to_string(self.root.join("plan.md")).ok()
    }

    /// Guarda el plan pendiente para retomarlo en la siguiente sesión.
    pub fn write_plan(&self, markdown: &str) -> Result<()> {
        let path = self.root.join("plan.md");
        fs::write(&path, markdown)
            .with_context(|| format!("No se pudo escribir {}", path.display()))?;
        Ok(())
    }

    /// Borra el plan pendiente (no hay plan activo en esta sesión).
    pub fn remove_plan(&self) -> Result<()> {
        let path = self.root.join("plan.md");
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("No se pudo borrar {}", path.display()))?;
        }
        Ok(())
    }

    /// Comandos que el usuario marcó como "permitir siempre" en este proyecto
    /// (`.dpx/allowed_commands`, uno por línea, coincidencia exacta).
    pub fn allowed_commands(&self) -> Vec<String> {
        fs::read_to_string(self.root.join("allowed_commands"))
            .map(|s| {
                s.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// ¿Este comando exacto está en la allowlist del proyecto?
    pub fn is_command_allowed(&self, cmd: &str) -> bool {
        let cmd = cmd.trim();
        self.allowed_commands().iter().any(|c| c == cmd)
    }

    /// Añade un comando a la allowlist del proyecto (idempotente).
    pub fn allow_command(&self, cmd: &str) -> Result<()> {
        if self.is_command_allowed(cmd) {
            return Ok(());
        }
        let path = self.root.join("allowed_commands");
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("No se pudo abrir {}", path.display()))?;
        writeln!(file, "{}", cmd.trim()).context("No se pudo guardar el comando permitido")?;
        Ok(())
    }
}

/// Preamble del resumidor: define exactamente las 4 secciones que el usuario pidió.
const SUMMARIZER_PREAMBLE: &str = "\
Eres el módulo de memoria de un mentor de programación. Tu trabajo es resumir una sesión de \
trabajo en un documento de contexto que el mentor leerá la próxima vez para retomar sin perder \
el hilo. Escribe en español, en Markdown, conciso y concreto. NO inventes nada que no esté en la \
conversación. Devuelve ÚNICAMENTE el Markdown, con EXACTAMENTE estas cuatro secciones:

# Estado del proyecto
Qué se está construyendo, en qué archivos/feature se trabaja y decisiones de arquitectura tomadas.

# Tu aprendizaje
Qué conceptos le enseñó el mentor al usuario y qué ya domina vs. qué le costó. La memoria del alumno.

# Próximos pasos
Lista corta de lo siguiente que toca hacer o aprender, para retomar al instante.

# Resumen de sesión
Dos o tres frases narrando qué pasó en esta sesión.";

/// Genera el contexto del proyecto a partir de la conversación de la sesión.
pub async fn summarize(
    router: &ModelRouter,
    turns: &[Turn],
    prior: Option<&str>,
) -> Result<String> {
    let mut transcript = String::new();
    for t in turns {
        transcript.push_str(&format!("[{}] {}\n\n", t.role, t.text));
    }

    let prior = prior.unwrap_or("(no había contexto previo)");
    let content = format!(
        "## Contexto previo (de sesiones anteriores)\n{prior}\n\n\
         ## Transcripción de la sesión actual\n{transcript}\n\n\
         Genera el documento de contexto actualizado integrando lo previo con lo nuevo."
    );

    router.summarize(SUMMARIZER_PREAMBLE, &content).await
}

/// Preamble del compactador: resume DENTRO de la sesión cuando el historial se
/// acerca al límite de contexto del modelo, para poder continuarla sin cortes.
const COMPACTOR_PREAMBLE: &str = "\
Eres el módulo de compactación de un mentor de programación. La conversación se acerca al límite \
de contexto del modelo: resume la transcripción en un documento corto que permita CONTINUARLA sin \
perder el hilo. En español, Markdown, conciso. Incluye: qué se está haciendo y qué falta, las \
decisiones tomadas (con su porqué), los nombres exactos de archivos/clases/comandos mencionados, y \
el estado de la última tarea en curso. Conserva más detalle de lo MÁS RECIENTE. No inventes nada.";

/// Resume la sesión en curso para compactar el historial (modelo barato).
pub async fn compact(router: &ModelRouter, turns: &[Turn]) -> Result<String> {
    let mut transcript = String::new();
    for t in turns {
        transcript.push_str(&format!("[{}] {}\n\n", t.role, t.text));
    }
    router.summarize(COMPACTOR_PREAMBLE, &transcript).await
}

/// Contexto de respaldo cuando el resumen con el modelo falla (p.ej. por
/// saturación). Guarda la transcripción cruda para no perder la sesión: la
/// próxima vez el mentor al menos tiene de qué partir.
pub fn fallback_context(turns: &[Turn], prior: Option<&str>) -> String {
    let mut md = String::new();
    if let Some(p) = prior {
        if !p.trim().is_empty() {
            md.push_str(p.trim());
            md.push_str("\n\n---\n\n");
        }
    }
    md.push_str(
        "# Resumen de sesión (sin procesar)\n\
         No se pudo generar el resumen con el modelo (probablemente saturación). \
         Se guarda la transcripción cruda de la última sesión:\n\n",
    );
    for t in turns {
        md.push_str(&format!("- **{}:** {}\n", t.role, t.text));
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_guarda_consulta_y_es_idempotente() {
        let dir = std::env::temp_dir().join(format!("dpx-store-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = ProjectStore::init(&dir).unwrap();

        assert!(!store.is_command_allowed("mvn -q compile"));
        store.allow_command("mvn -q compile").unwrap();
        assert!(store.is_command_allowed("mvn -q compile"));
        assert!(store.is_command_allowed("  mvn -q compile  "));
        assert!(!store.is_command_allowed("mvn clean deploy"));

        store.allow_command("mvn -q compile").unwrap();
        assert_eq!(store.allowed_commands().len(), 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
