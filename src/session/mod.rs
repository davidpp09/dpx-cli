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

use anyhow::{Context, Result, anyhow};
use chrono::Local;

use crate::agent::ModelRouter;

/// La memoria viva del proyecto.
const CONTEXT: &str = "context.md";
/// Su versión anterior. Es la red: si el resumen nuevo sale mal, aquí está el bueno.
const CONTEXT_BAK: &str = "context.md.bak";
/// Bitácora append-only, una entrada fechada por sesión. Nunca se reescribe.
const HISTORY: &str = "history.md";
/// Manifiesto de archivos que el turno CREÓ. Vive FUERA del árbol de snapshots
/// (`undo/`) a propósito: si estuviera dentro, la caminata de restauración lo
/// copiaría al proyecto como si fuera contenido del usuario.
const UNDO_CREATED: &str = "undo-created";

/// Lo que deshizo un `/undo`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Undone {
    /// Archivos que existían antes y volvieron a su contenido original.
    pub restored: Vec<String>,
    /// Archivos que el turno creó de cero y por tanto se BORRAN: restaurarlos
    /// no tendría sentido, no había nada que restaurar.
    pub deleted: Vec<String>,
}

impl Undone {
    pub fn is_empty(&self) -> bool {
        self.restored.is_empty() && self.deleted.is_empty()
    }

    pub fn total(&self) -> usize {
        self.restored.len() + self.deleted.len()
    }
}

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
    ///
    /// Si el archivo vivo está vacío o ilegible, tira del respaldo. Perder la
    /// memoria acumulada del proyecto por un cierre a medias sería el peor
    /// fallo posible de dpx: es lo único que no se puede reconstruir leyendo
    /// el código.
    pub fn prior_context(&self) -> Option<String> {
        let vivo = fs::read_to_string(self.root.join(CONTEXT))
            .ok()
            .filter(|c| !c.trim().is_empty());
        vivo.or_else(|| {
            fs::read_to_string(self.root.join(CONTEXT_BAK))
                .ok()
                .filter(|c| !c.trim().is_empty())
        })
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

    /// Escribe el contexto del proyecto (memoria viva), de forma DURABLE.
    ///
    /// Tres garantías, porque este archivo es la memoria del proyecto y antes
    /// se sobrescribía a pelo con un `fs::write`:
    ///
    /// 1. Un resumen vacío se RECHAZA. Que el modelo devuelva basura no puede
    ///    costarte meses de contexto acumulado.
    /// 2. La versión anterior queda en `context.md.bak` antes de tocar nada,
    ///    así siempre hay a dónde volver.
    /// 3. La escritura es atómica (tmp + rename): un corte a mitad deja el
    ///    archivo viejo intacto, nunca uno truncado.
    pub fn write_context(&self, markdown: &str) -> Result<()> {
        if markdown.trim().is_empty() {
            return Err(anyhow!(
                "no sobrescribo la memoria del proyecto con un resumen vacío"
            ));
        }
        let path = self.root.join(CONTEXT);
        if path.exists() {
            // Si el respaldo falla no abortamos: es preferible guardar la
            // memoria nueva sin respaldo que no guardarla.
            let _ = fs::copy(&path, self.root.join(CONTEXT_BAK));
        }
        let tmp = self.root.join("context.md.tmp");
        fs::write(&tmp, markdown)
            .with_context(|| format!("No se pudo escribir {}", tmp.display()))?;
        fs::rename(&tmp, &path)
            .with_context(|| format!("No se pudo reemplazar {}", path.display()))?;
        Ok(())
    }

    /// Añade una entrada FECHADA al historial append-only del proyecto.
    ///
    /// `context.md` se regenera en cada cierre resumiendo el resumen anterior,
    /// así que se degrada como el teléfono descompuesto: lo de hace diez
    /// sesiones acaba comprimido hasta desaparecer o, peor, distorsionado.
    /// `history.md` NO se reescribe nunca — es la columna vertebral con fechas
    /// contra la que siempre se puede contrastar qué pasó y cuándo.
    pub fn append_history(&self, resumen: &str) -> Result<()> {
        let resumen = resumen.trim();
        if resumen.is_empty() {
            return Ok(()); // nada que registrar; no ensuciamos el historial
        }
        let path = self.root.join(HISTORY);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("No se pudo abrir {}", path.display()))?;
        writeln!(file, "## {}\n\n{resumen}\n", Local::now().format("%Y-%m-%d %H:%M"))
            .context("No se pudo escribir la entrada del historial")?;
        Ok(())
    }

    /// El historial completo del proyecto, si existe.
    pub fn history(&self) -> Option<String> {
        fs::read_to_string(self.root.join(HISTORY))
            .ok()
            .filter(|h| !h.trim().is_empty())
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

    /// Lee las habilidades aprendidas (modo learn), si las hay.
    pub fn read_skills(&self) -> Vec<crate::skill::Skill> {
        match fs::read_to_string(self.root.join("skills.md")) {
            Ok(md) => crate::skill::from_markdown(&md),
            Err(_) => Vec::new(),
        }
    }

    /// Guarda el mapa de habilidades del usuario.
    pub fn write_skills(&self, skills: &[crate::skill::Skill]) -> Result<()> {
        let path = self.root.join("skills.md");
        fs::write(&path, crate::skill::to_markdown(skills))
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

    /// Guarda el contenido ORIGINAL de un archivo antes de que el agente lo modifique,
    /// para que `/undo` pueda revertir el último turno. Solo preserva la primera
    /// versión del turno: si el mismo archivo se toca dos veces, la original se conserva.
    pub fn save_undo_file(&self, rel_path: &str, content: &[u8]) -> Result<()> {
        let dst = self.root.join("undo").join(rel_path.replace('\\', "/"));
        if dst.exists() {
            return Ok(()); // ya tenemos el original de este turno
        }
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("No se pudo crear directorio undo para {rel_path}"))?;
        }
        fs::write(&dst, content)
            .with_context(|| format!("No se pudo guardar snapshot de {rel_path}"))
    }

    /// Anota que un archivo NO existía antes del turno. `/undo` debe BORRARLO,
    /// no restaurarlo: no hay contenido original al que volver.
    ///
    /// Sin esto, deshacer un turno que creó archivos dejaba esos archivos en el
    /// proyecto — el turno quedaba a medio revertir y el usuario creyendo que
    /// había vuelto atrás del todo.
    pub fn mark_created(&self, rel_path: &str) -> Result<()> {
        let rel = rel_path.replace('\\', "/");
        if self.created_files().iter().any(|p| p == &rel) {
            return Ok(()); // ya anotado en este turno
        }
        let path = self.root.join(UNDO_CREATED);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("No se pudo abrir {}", path.display()))?;
        writeln!(file, "{rel}").context("No se pudo anotar el archivo creado")?;
        Ok(())
    }

    /// Archivos anotados como creados en el turno actual.
    fn created_files(&self) -> Vec<String> {
        fs::read_to_string(self.root.join(UNDO_CREATED))
            .map(|s| {
                s.lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Deshace el último turno: restaura lo que se modificó y BORRA lo que se
    /// creó.
    pub fn restore_undo(&self, cwd: &Path) -> Result<Undone> {
        let mut undone = Undone::default();

        let undo_dir = self.root.join("undo");
        if undo_dir.exists() {
            undo_restore_recursive(&undo_dir, &undo_dir, cwd, &mut undone.restored)?;
        }

        for rel in self.created_files() {
            // `safe_target` no es paranoia decorativa: el manifiesto es un
            // archivo de texto, y un `../..` colado ahí haría que /undo borrara
            // fuera del proyecto.
            let Ok(target) = crate::fs::safe_target(cwd, &rel) else {
                continue;
            };
            if target.is_file() && fs::remove_file(&target).is_ok() {
                undone.deleted.push(rel);
            }
        }
        Ok(undone)
    }

    /// Limpia el snapshot de undo. Llamar al inicio de cada turno nuevo.
    pub fn clear_undo(&self) -> Result<()> {
        let undo_dir = self.root.join("undo");
        if undo_dir.exists() {
            fs::remove_dir_all(&undo_dir)
                .with_context(|| "No se pudo limpiar el directorio undo")?;
        }
        // El manifiesto va aparte del árbol, así que hay que borrarlo aparte.
        let creados = self.root.join(UNDO_CREATED);
        if creados.exists() {
            fs::remove_file(&creados)
                .with_context(|| "No se pudo limpiar el manifiesto de archivos creados")?;
        }
        Ok(())
    }

    /// Lee la racha de aprendizaje del usuario, si existe.
    pub fn read_streak(&self) -> Option<crate::streak::Streak> {
        std::fs::read_to_string(self.root.join("streak.md"))
            .ok()
            .and_then(|md| crate::streak::from_markdown(&md))
    }

    /// Guarda la racha de aprendizaje.
    pub fn write_streak(&self, streak: &crate::streak::Streak) -> Result<()> {
        let path = self.root.join("streak.md");
        fs::write(&path, crate::streak::to_markdown(streak))
            .with_context(|| format!("No se pudo escribir {}", path.display()))
    }

    /// Lee la síntesis del último comité de hack, si existe.
    pub fn read_committee(&self) -> Option<String> {
        fs::read_to_string(self.root.join("committee.md")).ok()
    }

    /// Guarda la síntesis del comité de hack para esta sesión.
    pub fn write_committee(&self, markdown: &str) -> Result<()> {
        let path = self.root.join("committee.md");
        fs::write(&path, markdown)
            .with_context(|| format!("No se pudo escribir {}", path.display()))?;
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

/// Saca la sección "Resumen de sesión" del contexto generado, que es la parte
/// narrativa y fechable — las otras tres describen un ESTADO que cambia, y
/// apilarlas en una bitácora solo generaría ruido.
///
/// Si el modelo no respetó el formato, cae al principio del documento: más vale
/// una entrada imperfecta que un hueco en el historial.
pub fn extract_session_summary(markdown: &str) -> String {
    let mut lineas = markdown.lines();
    let encontrada = lineas.any(|l| {
        let t = l.trim_start_matches('#').trim().to_lowercase();
        t.starts_with("resumen de sesión") || t.starts_with("resumen de sesion")
    });
    if encontrada {
        // Hasta el siguiente encabezado (la sección puede ser la última).
        let cuerpo: Vec<&str> = lineas
            .take_while(|l| !l.trim_start().starts_with('#'))
            .collect();
        let cuerpo = cuerpo.join("\n");
        if !cuerpo.trim().is_empty() {
            return cuerpo.trim().to_string();
        }
    }
    markdown.trim().lines().take(6).collect::<Vec<_>>().join("\n")
}

/// Contexto de respaldo cuando el resumen con el modelo falla (p.ej. por
/// saturación). Guarda la transcripción cruda para no perder la sesión: la
/// próxima vez el mentor al menos tiene de qué partir.
pub fn fallback_context(turns: &[Turn], prior: Option<&str>) -> String {
    let mut md = String::new();
    if let Some(p) = prior
        && !p.trim().is_empty() {
            md.push_str(p.trim());
            md.push_str("\n\n---\n\n");
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

/// Caminata recursiva sobre el directorio de undo: copia cada archivo de vuelta
/// a su posición original relativa dentro de `cwd`.
fn undo_restore_recursive(
    base: &Path,
    dir: &Path,
    cwd: &Path,
    restored: &mut Vec<String>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("No se pudo leer {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            undo_restore_recursive(base, &path, cwd, restored)?;
        } else {
            let rel = path.strip_prefix(base).unwrap_or(&path);
            let dst = cwd.join(rel);
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&path, &dst)
                .with_context(|| format!("No se pudo restaurar {}", rel.display()))?;
            restored.push(rel.display().to_string().replace('\\', "/"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(nombre: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("dpx-sess-{}-{nombre}-{}", std::process::id(), line!()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn la_memoria_no_se_pierde_por_un_resumen_vacio() {
        let dir = tmp("ctx-vacio");
        let store = ProjectStore::init(&dir).unwrap();

        store.write_context("# Estado\nmemoria valiosa de meses").unwrap();
        // Un resumen vacío (modelo que devolvió basura) NO puede borrarla.
        for basura in ["", "   ", "\n\n\t"] {
            assert!(store.write_context(basura).is_err(), "aceptó `{basura:?}`");
        }
        assert!(store.prior_context().unwrap().contains("memoria valiosa"));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cada_escritura_deja_la_anterior_como_respaldo() {
        let dir = tmp("ctx-bak");
        let store = ProjectStore::init(&dir).unwrap();

        store.write_context("version uno").unwrap();
        store.write_context("version dos").unwrap();

        assert_eq!(store.prior_context().unwrap(), "version dos");
        let bak = fs::read_to_string(dir.join(".dpx").join(CONTEXT_BAK)).unwrap();
        assert_eq!(bak, "version uno", "el respaldo debe guardar la versión previa");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn un_context_truncado_se_recupera_del_respaldo() {
        let dir = tmp("ctx-recover");
        let store = ProjectStore::init(&dir).unwrap();

        store.write_context("memoria buena").unwrap();
        store.write_context("memoria nueva").unwrap();

        // Simula un cierre a mitad de escritura: el archivo vivo queda vacío.
        fs::write(dir.join(".dpx").join(CONTEXT), "").unwrap();
        assert_eq!(
            store.prior_context().as_deref(),
            Some("memoria buena"),
            "con el archivo vivo vacío hay que tirar del respaldo"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn undo_borra_lo_creado_y_restaura_lo_modificado() {
        let dir = tmp("undo");
        let store = ProjectStore::init(&dir).unwrap();

        // Un archivo que YA existía y el turno modifica.
        fs::write(dir.join("viejo.rs"), "original").unwrap();
        store.save_undo_file("viejo.rs", b"original").unwrap();
        fs::write(dir.join("viejo.rs"), "modificado").unwrap();

        // Un archivo que el turno CREA de cero.
        fs::create_dir_all(dir.join("src")).unwrap();
        fs::write(dir.join("src/nuevo.rs"), "recién creado").unwrap();
        store.mark_created("src/nuevo.rs").unwrap();

        let undone = store.restore_undo(&dir).unwrap();

        assert_eq!(undone.restored, vec!["viejo.rs".to_string()]);
        assert_eq!(undone.deleted, vec!["src/nuevo.rs".to_string()]);
        assert_eq!(undone.total(), 2);
        assert_eq!(fs::read_to_string(dir.join("viejo.rs")).unwrap(), "original");
        assert!(
            !dir.join("src/nuevo.rs").exists(),
            "el archivo creado debe desaparecer, no quedarse a medias"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn undo_no_borra_fuera_del_proyecto() {
        let dir = tmp("undo-escape");
        let store = ProjectStore::init(&dir).unwrap();

        let victima = dir.join("no-tocar.txt");
        fs::write(&victima, "archivo de fuera").unwrap();

        // Manifiesto envenenado: una ruta que se escapa del proyecto.
        fs::write(
            dir.join(".dpx").join(UNDO_CREATED),
            "../no-tocar.txt\n/etc/passwd\n",
        )
        .unwrap();

        let undone = store.restore_undo(&dir.join("sub")).unwrap();
        assert!(undone.deleted.is_empty(), "no puede borrar por rutas de escape");
        assert!(victima.exists(), "borró un archivo fuera del proyecto");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn clear_undo_limpia_tambien_el_manifiesto() {
        let dir = tmp("undo-clear");
        let store = ProjectStore::init(&dir).unwrap();

        store.mark_created("a.rs").unwrap();
        store.mark_created("a.rs").unwrap(); // idempotente
        assert_eq!(store.created_files(), vec!["a.rs".to_string()]);

        store.clear_undo().unwrap();
        assert!(
            store.created_files().is_empty(),
            "el manifiesto sobrevivió al clear: el próximo /undo borraría de más"
        );

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn el_extractor_saca_la_seccion_narrativa() {
        let md = "# Estado del proyecto\nse toca el router\n\n\
                  # Tu aprendizaje\nvimos tiers\n\n\
                  # Próximos pasos\n- medir\n\n\
                  # Resumen de sesión\nSe cambió el cerebro a flash.\nQuedó medido.\n";
        let r = extract_session_summary(md);
        assert_eq!(r, "Se cambió el cerebro a flash.\nQuedó medido.");
        // No arrastra las otras secciones: son ESTADO, no narrativa.
        assert!(!r.contains("Estado del proyecto") && !r.contains("Próximos pasos"));
    }

    #[test]
    fn el_extractor_no_deja_hueco_si_el_modelo_ignora_el_formato() {
        // Sin la sección esperada cae a las primeras líneas: una entrada
        // imperfecta es mejor que un agujero en la bitácora.
        let r = extract_session_summary("El modelo respondió a su aire.\nSegunda línea.");
        assert!(r.contains("El modelo respondió a su aire"));
        // Y sin nada, nada.
        assert!(extract_session_summary("   ").is_empty());
    }

    #[test]
    fn el_extractor_tolera_la_seccion_sin_tilde() {
        let md = "# Resumen de sesion\nfuncionó igual\n";
        assert_eq!(extract_session_summary(md), "funcionó igual");
    }

    #[test]
    fn la_bitacora_solo_acumula_y_lleva_fecha() {
        let dir = tmp("history");
        let store = ProjectStore::init(&dir).unwrap();

        assert!(store.history().is_none());
        store.append_history("primera sesión").unwrap();
        store.append_history("segunda sesión").unwrap();

        let h = store.history().unwrap();
        // Lo viejo NUNCA se pierde: ese es el punto frente a context.md.
        assert!(h.contains("primera sesión"), "{h}");
        assert!(h.contains("segunda sesión"), "{h}");
        assert!(h.contains(&Local::now().format("%Y-%m-%d").to_string()), "falta la fecha: {h}");
        assert_eq!(h.matches("## ").count(), 2, "una entrada por sesión");

        // Un resumen vacío no ensucia la bitácora.
        store.append_history("   ").unwrap();
        assert_eq!(store.history().unwrap().matches("## ").count(), 2);

        fs::remove_dir_all(&dir).ok();
    }

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
