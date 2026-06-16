//! Skills (playbooks) CURADOS del proyecto: archivos `skills/*.md` escritos y
//! revisados a mano que le dicen a dpx CÓMO se hacen las cosas EN ESTE proyecto,
//! paso a paso (A→B). A diferencia de [`crate::skill`] (que rastrea el aprendizaje
//! del USUARIO en modo learn), estos guían a dpx.
//!
//! Antes de cada turno, los skills que encajan con la petición se recuperan por
//! similitud (coseno, reusando [`crate::memory`]) y se inyectan al prompt — así
//! dpx SIGUE el playbook concreto en vez de explorar a ciegas (lo que lo hacía
//! quemar rondas o entregar cosas genéricas).
//!
//! Son CURADOS, no auto-aprendidos: nada se genera ni persiste solo. Para añadir
//! uno, creas un `.md` en `skills/` con frontmatter `name`/`focus`/`cuando` + los
//! pasos en el cuerpo.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::memory::cosine;

/// Un playbook curado del proyecto (un `skills/*.md`).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AgentSkill {
    /// Título corto y reconocible ("agregar un comando", "crear endpoint REST").
    pub name: String,
    /// El procedimiento: los pasos A→B del cuerpo del `.md`.
    pub body: String,
    /// Stack/focus donde aplica ("" = general).
    #[serde(default)]
    pub focus: String,
    /// "Cuándo usarlo" — frase de disparo; pesa en el recall por similitud.
    #[serde(default)]
    pub when: String,
    /// Embedding de `name + when + body` para recuperar por similitud (perezoso).
    #[serde(default)]
    pub vector: Vec<f32>,
}

/// Catálogo de skills curados cargado de `skills/*.md`. De solo lectura: no se
/// auto-persiste nada (los `.md` se editan a mano).
pub struct SkillBook {
    skills: Vec<AgentSkill>,
}

impl SkillBook {
    /// Carga los skills curados de `<skills_dir>/*.md`. Dir inexistente = vacío
    /// (degradación elegante: sin skills, el CLI ni carga el motor de embeddings).
    pub fn from_dir(skills_dir: &Path) -> Self {
        Self { skills: load_curated(skills_dir) }
    }

    /// ¿Hay skills? (Si está vacío, ni se carga el embedder.)
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Número de skills curados.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Todos los skills (para `/skills`), por nombre.
    pub fn ranked(&self) -> Vec<&AgentSkill> {
        self.skills.iter().collect()
    }

    /// Embebe (vectoriza) los skills que aún no tienen vector, usando `embed`.
    /// Se llama perezosamente la primera vez que se busca, cuando el motor de
    /// embeddings ya está cargado — así no encarecemos el arranque.
    pub fn embed_pending<F>(&mut self, mut embed: F)
    where
        F: FnMut(&str) -> Option<Vec<f32>>,
    {
        for s in &mut self.skills {
            if s.vector.is_empty() {
                let text = format!("{}\n{}\n{}", s.name, s.when, s.body);
                if let Some(v) = embed(&text) {
                    s.vector = v;
                }
            }
        }
    }

    /// Los `k` skills más parecidos a `query_vec` por encima de `min_score`,
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
}

/// Lee `dir/*.md` y los parsea a skills curados. Ordenados por nombre.
pub fn load_curated(dir: &Path) -> Vec<AgentSkill> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("skill");
        if let Some(skill) = parse_skill_md(&text, stem) {
            out.push(skill);
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Parsea un `.md` con frontmatter `--- ... ---` (`name`/`focus`/`cuando`) y el
/// cuerpo (los pasos). Sin frontmatter, usa el nombre de archivo y todo el texto.
/// Devuelve `None` si el cuerpo queda vacío.
fn parse_skill_md(text: &str, file_stem: &str) -> Option<AgentSkill> {
    let mut name = file_stem.replace('-', " ");
    let mut focus = String::new();
    let mut when = String::new();

    let trimmed = text.trim_start();
    let body = if let Some(rest) = trimmed.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                let line = line.trim();
                if let Some(v) = line.strip_prefix("name:") {
                    name = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("focus:") {
                    focus = v.trim().to_string();
                } else if let Some(v) = line.strip_prefix("cuando:").or_else(|| line.strip_prefix("when:")) {
                    when = v.trim().trim_matches('"').to_string();
                }
            }
            // Salta el cierre "\n---" y la posible línea restante del marcador.
            let after = &rest[end + 4..];
            after.trim_start_matches(['-', '\n', '\r']).trim().to_string()
        } else {
            text.trim().to_string()
        }
    } else {
        text.trim().to_string()
    };

    if body.is_empty() {
        return None;
    }
    Some(AgentSkill { name, body, focus, when, vector: Vec::new() })
}

/// ¿Es `info` (lo que sigue a la valla ```` ``` ````) una valla `dpx:learned`?
/// Ya no auto-aprendemos, pero el parser de bloques (`fs`) sigue ignorando estas
/// vallas por si un modelo emite una: no se trata su interior como código.
pub fn is_learned_fence(info: &str) -> bool {
    info.trim().starts_with("dpx:learned")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_md_extrae_frontmatter_y_cuerpo() {
        let md = "---\nname: agregar un comando\nfocus: dpx\ncuando: \"agregar o quitar un comando\"\n---\n## Pasos\n1. tabla\n2. ayuda\n";
        let s = parse_skill_md(md, "agregar-comando").expect("debe parsear");
        assert_eq!(s.name, "agregar un comando");
        assert_eq!(s.focus, "dpx");
        assert_eq!(s.when, "agregar o quitar un comando");
        assert!(s.body.contains("1. tabla"));
        assert!(s.body.starts_with("## Pasos"));
        assert!(s.vector.is_empty(), "el vector se llena perezosamente");
    }

    #[test]
    fn parse_md_sin_frontmatter_usa_el_nombre_de_archivo() {
        let s = parse_skill_md("solo el cuerpo, sin frontmatter", "testear-repo").expect("parsea");
        assert_eq!(s.name, "testear repo");
        assert_eq!(s.body, "solo el cuerpo, sin frontmatter");
        // Cuerpo vacío → None (no es un skill útil).
        assert!(parse_skill_md("---\nname: x\n---\n   ", "x").is_none());
    }

    #[test]
    fn search_trae_el_parecido_y_filtra_ruido() {
        let mut book = SkillBook { skills: Vec::new() };
        book.skills.push(AgentSkill {
            name: "endpoint".into(), body: "...".into(), focus: String::new(),
            when: String::new(), vector: vec![1.0, 0.0],
        });
        book.skills.push(AgentSkill {
            name: "front".into(), body: "...".into(), focus: String::new(),
            when: String::new(), vector: vec![0.0, 1.0],
        });
        let hits = book.search(&[1.0, 0.0], 3, 0.5);
        assert_eq!(hits.len(), 1, "solo el parecido supera el umbral");
        assert_eq!(hits[0].name, "endpoint");
    }

    #[test]
    fn embed_pending_llena_solo_los_vacios() {
        let mut book = SkillBook { skills: Vec::new() };
        book.skills.push(AgentSkill {
            name: "a".into(), body: "b".into(), focus: String::new(),
            when: String::new(), vector: Vec::new(),
        });
        book.embed_pending(|_| Some(vec![0.5, 0.5]));
        assert_eq!(book.ranked()[0].vector, vec![0.5, 0.5]);
    }
}
