//! Skills auto-mejorables del AGENTE (estilo "Hermes"): playbooks que dpx
//! aprende y refina sobre cómo se hacen las cosas EN ESTE proyecto, para ser
//! mejor en lo que se usa a diario.
//!
//! A diferencia de [`crate::skill`] (que rastrea el aprendizaje del USUARIO en
//! modo learn), aquí el que aprende es dpx: cuando termina una tarea no trivial
//! que puede repetirse, declara un bloque ```dpx:learned``` con el procedimiento;
//! el CLI lo embebe y, si ya hay una skill bastante parecida, la REFINA (sube
//! usos y confianza, actualiza el cuerpo); si no, la crea. Antes de cada turno,
//! las skills que encajan con la petición se recuperan por similitud (coseno,
//! reusando [`crate::memory`]) y se inyectan al prompt — así dpx aplica y mejora
//! lo que ya sabe.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::memory::cosine;

/// Umbral de coseno por encima del cual una skill declarada se considera "la
/// misma" que una existente y la REFINA en vez de crear una duplicada.
const SAME_SKILL_THRESHOLD: f32 = 0.82;

/// Una skill/playbook aprendida por dpx para este proyecto.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AgentSkill {
    /// Título corto y reconocible ("Crear endpoint REST", "Testear este repo").
    pub name: String,
    /// El procedimiento aprendido: pasos/criterios reutilizables, concisos.
    pub body: String,
    /// Stack/focus donde aplica ("" = general).
    #[serde(default)]
    pub focus: String,
    /// Veces que se ha reforzado (refinado) — más usos = más fiable.
    #[serde(default)]
    pub uses: u32,
    /// Confianza 0.0–1.0; sube con cada refuerzo (saturando en 1.0).
    #[serde(default)]
    pub confidence: f32,
    /// Fecha de creación (YYYY-MM-DD).
    #[serde(default)]
    pub created: String,
    /// Fecha del último refuerzo (YYYY-MM-DD).
    #[serde(default)]
    pub updated: String,
    /// Embedding de `name + body` para recuperar por similitud.
    #[serde(default)]
    pub vector: Vec<f32>,
}

/// Resultado de incorporar una skill declarada por dpx.
pub enum Learned {
    /// Skill nueva (no había nada parecido).
    New(String),
    /// Skill ya existente, refinada (su nombre).
    Refined(String),
}

/// Almacén de skills del agente, persistido en `.dpx/agent_skills.json` (array
/// JSON: las skills MUTAN al refinarse, así que reescribimos el archivo entero,
/// no es append-only como la memoria). Carga todo en memoria al abrir.
pub struct SkillBook {
    path: PathBuf,
    skills: Vec<AgentSkill>,
}

impl SkillBook {
    /// Abre (o crea vacío) el libro de skills en `<dpx_dir>/agent_skills.json`.
    /// Un archivo corrupto se trata como vacío en vez de fallar.
    pub fn load(dpx_dir: &Path) -> Self {
        let path = dpx_dir.join("agent_skills.json");
        let skills = std::fs::read_to_string(&path)
            .ok()
            .and_then(|d| serde_json::from_str::<Vec<AgentSkill>>(&d).ok())
            .unwrap_or_default();
        Self { path, skills }
    }

    /// ¿Hay skills guardadas? (Si está vacío, el CLI ni carga el motor.)
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Número de skills aprendidas.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Todas las skills (para `/skills`), ordenadas por confianza descendente.
    pub fn ranked(&self) -> Vec<&AgentSkill> {
        let mut v: Vec<&AgentSkill> = self.skills.iter().collect();
        v.sort_by(|a, b| b.confidence.total_cmp(&a.confidence).then(b.uses.cmp(&a.uses)));
        v
    }

    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let json =
            serde_json::to_string_pretty(&self.skills).context("no pude serializar las skills")?;
        std::fs::write(&self.path, json)
            .with_context(|| format!("no pude escribir {}", self.path.display()))?;
        Ok(())
    }

    /// Las `k` skills más parecidas a `query_vec` por encima de `min_score`,
    /// de mayor a menor similitud.
    pub fn search(&self, query_vec: &[f32], k: usize, min_score: f32) -> Vec<&AgentSkill> {
        let mut scored: Vec<(f32, &AgentSkill)> = self
            .skills
            .iter()
            .filter(|s| !s.vector.is_empty())
            .map(|s| (cosine(query_vec, &s.vector), s))
            .filter(|(score, _)| *score >= min_score)
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.into_iter().take(k).map(|(_, s)| s).collect()
    }

    /// Incorpora una skill declarada por dpx: si hay una existente lo bastante
    /// parecida (por vector), la REFINA (cuerpo nuevo, +1 uso, +confianza, fecha);
    /// si no, la añade nueva. Persiste y devuelve qué ocurrió.
    pub fn reinforce_or_add(&mut self, skill: AgentSkill) -> Result<Learned> {
        let best = self
            .skills
            .iter_mut()
            .filter(|s| !s.vector.is_empty() && !skill.vector.is_empty())
            .map(|s| (cosine(&skill.vector, &s.vector), s))
            .filter(|(score, _)| *score >= SAME_SKILL_THRESHOLD)
            .max_by(|a, b| a.0.total_cmp(&b.0));

        if let Some((_, existing)) = best {
            // REFINAR: el cuerpo nuevo es la versión mejorada; sube métricas.
            existing.body = skill.body;
            if !skill.focus.is_empty() {
                existing.focus = skill.focus;
            }
            existing.vector = skill.vector;
            existing.uses = existing.uses.saturating_add(1);
            existing.confidence = (existing.confidence + 0.15).min(1.0);
            existing.updated = skill.updated;
            let name = existing.name.clone();
            self.save()?;
            return Ok(Learned::Refined(name));
        }

        // NUEVA.
        let mut skill = skill;
        skill.uses = 1;
        skill.confidence = 0.5;
        let name = skill.name.clone();
        self.skills.push(skill);
        self.save()?;
        Ok(Learned::New(name))
    }
}

/// ¿Es `info` (lo que sigue a la valla ```` ``` ````) una valla `dpx:learned`?
pub fn is_learned_fence(info: &str) -> bool {
    info.trim().starts_with("dpx:learned")
}

/// Extrae el bloque ```dpx:learned``` del texto del modelo, si lo hay: la PRIMERA
/// línea no vacía es el nombre y el resto es el cuerpo del playbook. Devuelve
/// `(name, body)`, o `None` si no hay bloque o no tiene nombre.
pub fn parse_learned_block(text: &str) -> Option<(String, String)> {
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let Some(info) = line.trim_start().strip_prefix("```") else {
            continue;
        };
        if !is_learned_fence(info) {
            continue;
        }
        // Recoge el interior hasta el cierre ```.
        let mut inner: Vec<&str> = Vec::new();
        for l in lines.by_ref() {
            if l.trim_start().starts_with("```") {
                break;
            }
            inner.push(l);
        }
        let mut it = inner.iter().map(|s| s.trim_end());
        let name = it.by_ref().find(|s| !s.trim().is_empty())?.trim().to_string();
        let body = it.collect::<Vec<_>>().join("\n").trim().to_string();
        if name.is_empty() {
            return None;
        }
        return Some((name, body));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(name: &str, body: &str, v: Vec<f32>) -> AgentSkill {
        AgentSkill {
            name: name.into(),
            body: body.into(),
            focus: "rust".into(),
            uses: 0,
            confidence: 0.0,
            created: "2026-06-14".into(),
            updated: "2026-06-14".into(),
            vector: v,
        }
    }

    #[test]
    fn parse_learned_extrae_nombre_y_cuerpo() {
        let text = "Listo, lo dejé funcionando.\n\n```dpx:learned\nCrear endpoint REST aquí\n\
                    @RestController + service/ + DTO validado\ntest con MockMvc\n```\nfin.";
        let (name, body) = parse_learned_block(text).expect("debe parsear");
        assert_eq!(name, "Crear endpoint REST aquí");
        assert!(body.contains("MockMvc"));
        // Sin bloque → None.
        assert!(parse_learned_block("solo texto normal").is_none());
    }

    #[test]
    fn nueva_y_luego_refina_la_misma() {
        let dir = std::env::temp_dir().join(format!("dpx-skills-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut book = SkillBook::load(&dir);
        assert!(book.is_empty());

        // Primera vez: NUEVA (confianza 0.5, 1 uso).
        let r = book.reinforce_or_add(skill("crear endpoint", "v1", vec![1.0, 0.0, 0.0])).unwrap();
        assert!(matches!(r, Learned::New(_)));
        assert_eq!(book.len(), 1);

        // Vector casi idéntico → REFINA la existente (no duplica), sube métricas.
        let r = book.reinforce_or_add(skill("crear endpoint v2", "v2 mejor", vec![0.99, 0.01, 0.0])).unwrap();
        assert!(matches!(r, Learned::Refined(_)));
        assert_eq!(book.len(), 1, "no debe duplicar la misma skill");
        let s = &book.ranked()[0];
        assert_eq!(s.body, "v2 mejor", "el cuerpo se refina a la versión nueva");
        assert_eq!(s.uses, 2);
        assert!(s.confidence > 0.5);

        // Vector ortogonal → skill NUEVA distinta.
        let r = book.reinforce_or_add(skill("testear repo", "cargo test", vec![0.0, 1.0, 0.0])).unwrap();
        assert!(matches!(r, Learned::New(_)));
        assert_eq!(book.len(), 2);

        // Persistió a disco.
        let reopened = SkillBook::load(&dir);
        assert_eq!(reopened.len(), 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn search_trae_la_parecida_y_filtra_ruido() {
        let dir = std::env::temp_dir().join(format!("dpx-skills2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut book = SkillBook::load(&dir);
        book.reinforce_or_add(skill("endpoint", "...", vec![1.0, 0.0])).unwrap();
        book.reinforce_or_add(skill("front", "...", vec![0.0, 1.0])).unwrap();
        let hits = book.search(&[1.0, 0.0], 3, 0.5);
        assert_eq!(hits.len(), 1, "solo la parecida supera el umbral");
        assert_eq!(hits[0].name, "endpoint");
        std::fs::remove_dir_all(&dir).ok();
    }
}
