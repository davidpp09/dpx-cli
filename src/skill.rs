//! Modelo de habilidades del usuario — el tablero de progreso del modo `learn`.
//!
//! En modo APRENDER, el tutor emite al final de su turno un bloque
//! ```` ```dpx:skill ```` con una habilidad por línea (`nivel | tema | stack`).
//! El CLI lo parsea ([`parse_block`]), lo fusiona con lo ya sabido ([`merge`],
//! que sube el nivel y refresca la fecha) y lo persiste en `.dpx/skills.md`
//! ([`to_markdown`]/[`from_markdown`]). La vista rica la pinta
//! [`ui::learn_panel`], que muestra dots de nivel, badges y repaso espaciado.
//! Mismo patrón de bloque que `dpx:plan`.

/// Días desde la última vez que se vio una habilidad NO dominada antes de
/// sugerir repasarla (repaso espaciado, simple y efectivo).
const REVIEW_AFTER_DAYS: i64 = 3;

/// Nivel de dominio de una habilidad. Ordenado: solo sube, nunca baja.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillLevel {
    /// Recién presentado: lo viste pero no lo has aplicado.
    Visto,
    /// Lo estás intentando, con ayuda.
    Practicando,
    /// Lo demostraste sin ayuda.
    Dominado,
}

impl SkillLevel {
    /// Parsea el nivel desde la palabra del bloque (tolerante a mayúsculas/acentos).
    pub fn parse(s: &str) -> Option<SkillLevel> {
        match s.trim().to_ascii_lowercase().as_str() {
            "visto" | "seen" => Some(SkillLevel::Visto),
            "practicando" | "practicing" => Some(SkillLevel::Practicando),
            "dominado" | "mastered" => Some(SkillLevel::Dominado),
            _ => None,
        }
    }

    /// Palabra estable para persistir.
    pub fn as_str(self) -> &'static str {
        match self {
            SkillLevel::Visto => "visto",
            SkillLevel::Practicando => "practicando",
            SkillLevel::Dominado => "dominado",
        }
    }

    /// Icono para el tablero.
    pub fn icon(self) -> &'static str {
        match self {
            SkillLevel::Visto => "◔",
            SkillLevel::Practicando => "◑",
            SkillLevel::Dominado => "●",
        }
    }

}

/// Una habilidad que el usuario ha tocado en modo learn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub level: SkillLevel,
    pub topic: String,
    pub stack: String,
    /// Fecha de la última vez que se trabajó (YYYY-MM-DD).
    pub last_seen: String,
}

/// La fecha de hoy en formato `YYYY-MM-DD` (zona local).
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Clave de identidad de una habilidad: tema + stack, normalizados. Así el
/// mismo tema se acumula entre sesiones aunque cambie mayúsculas/espacios.
fn key(topic: &str, stack: &str) -> (String, String) {
    (
        topic.trim().to_lowercase(),
        stack.trim().to_lowercase(),
    )
}

/// ¿La info-string de un fence es un bloque `dpx:skill`?
pub fn is_skill_fence(s: &str) -> bool {
    let t = s.trim();
    t == "dpx:skill" || t.starts_with("dpx:skill ")
}

/// Parsea un bloque ```` ```dpx:skill ```` de un texto (puede haber varios; se
/// concatenan). Cada línea: `nivel | tema | stack`. Las líneas mal formadas se
/// ignoran (best-effort, nunca paniquea). `today` sella la fecha de visto.
pub fn parse_block(text: &str, today: &str) -> Vec<Skill> {
    let mut skills = Vec::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let Some(info) = line.trim_start().strip_prefix("```") else {
            continue;
        };
        // El fence puede traer la info en la misma línea o en la siguiente.
        let mut is_skill = is_skill_fence(info);
        if !is_skill && lines.peek().is_some_and(|n| is_skill_fence(n)) {
            is_skill = true;
            lines.next();
        }
        if !is_skill {
            continue;
        }
        for body in lines.by_ref() {
            if body.trim_start().starts_with("```") {
                break;
            }
            if let Some(skill) = parse_line(body, today) {
                skills.push(skill);
            }
        }
    }
    skills
}

/// Parsea una línea `nivel | tema | stack`. `None` si no encaja.
fn parse_line(line: &str, today: &str) -> Option<Skill> {
    let parts: Vec<&str> = line.split('|').map(str::trim).collect();
    if parts.len() < 2 {
        return None;
    }
    let level = SkillLevel::parse(parts[0])?;
    let topic = parts[1].trim();
    if topic.is_empty() {
        return None;
    }
    let stack = parts.get(2).map(|s| s.trim()).filter(|s| !s.is_empty()).unwrap_or("general");
    Some(Skill {
        level,
        topic: topic.to_string(),
        stack: stack.to_string(),
        last_seen: today.to_string(),
    })
}

/// Fusiona habilidades nuevas en las existentes: si el tema+stack ya existe, el
/// nivel SUBE al máximo de ambos (nunca baja) y se refresca `last_seen`; si es
/// nuevo, se añade. Devuelve la lista combinada ordenada por stack y tema.
pub fn merge(existing: Vec<Skill>, new: Vec<Skill>) -> Vec<Skill> {
    let mut out = existing;
    for n in new {
        let k = key(&n.topic, &n.stack);
        if let Some(found) = out.iter_mut().find(|e| key(&e.topic, &e.stack) == k) {
            found.level = found.level.max(n.level);
            found.last_seen = n.last_seen;
        } else {
            out.push(n);
        }
    }
    out.sort_by(|a, b| {
        a.stack
            .to_lowercase()
            .cmp(&b.stack.to_lowercase())
            .then(a.topic.to_lowercase().cmp(&b.topic.to_lowercase()))
    });
    out
}

/// Serializa a Markdown para persistir en `.dpx/skills.md`.
pub fn to_markdown(skills: &[Skill]) -> String {
    let mut md = String::from(
        "# Habilidades del usuario\n\nFormato: `nivel | tema | stack | última vez`.\n\n",
    );
    for s in skills {
        md.push_str(&format!(
            "{} | {} | {} | {}\n",
            s.level.as_str(),
            s.topic,
            s.stack,
            s.last_seen
        ));
    }
    md
}

/// Lee las habilidades persistidas desde el Markdown de `.dpx/skills.md`.
/// Tolera líneas que no encajan (cabeceras, vacías).
pub fn from_markdown(md: &str) -> Vec<Skill> {
    let mut out = Vec::new();
    for line in md.lines() {
        let parts: Vec<&str> = line.split('|').map(str::trim).collect();
        if parts.len() < 4 {
            continue;
        }
        let Some(level) = SkillLevel::parse(parts[0]) else {
            continue;
        };
        if parts[1].is_empty() {
            continue;
        }
        out.push(Skill {
            level,
            topic: parts[1].to_string(),
            stack: parts[2].to_string(),
            last_seen: parts[3].to_string(),
        });
    }
    out
}

/// ¿Cuántos días han pasado entre dos fechas `YYYY-MM-DD`? `None` si no parsean.
pub fn days_between(from: &str, to: &str) -> Option<i64> {
    let f = chrono::NaiveDate::parse_from_str(from, "%Y-%m-%d").ok()?;
    let t = chrono::NaiveDate::parse_from_str(to, "%Y-%m-%d").ok()?;
    Some((t - f).num_days())
}

/// ¿Toca repasar esta habilidad hoy? (no dominada + vista hace ≥ N días).
pub fn needs_review(skill: &Skill, today: &str) -> bool {
    skill.level != SkillLevel::Dominado
        && days_between(&skill.last_seen, today).is_some_and(|d| d >= REVIEW_AFTER_DAYS)
}



#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsea_bloque_skill() {
        let text = "bla bla\n```dpx:skill\ndominado | Inyección de dependencias | spring-boot\nvisto | Patrón Repository | spring-boot\n```\nmás texto";
        let skills = parse_block(text, "2026-06-14");
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].level, SkillLevel::Dominado);
        assert_eq!(skills[0].topic, "Inyección de dependencias");
        assert_eq!(skills[0].stack, "spring-boot");
        assert_eq!(skills[0].last_seen, "2026-06-14");
        assert_eq!(skills[1].level, SkillLevel::Visto);
    }

    #[test]
    fn parse_line_tolera_basura_y_default_stack() {
        assert!(parse_line("no es una skill", "2026-06-14").is_none());
        assert!(parse_line("nivelraro | algo | x", "2026-06-14").is_none());
        // Sin stack → "general".
        let s = parse_line("visto | Closures |", "2026-06-14").unwrap();
        assert_eq!(s.stack, "general");
        let s2 = parse_line("practicando | Ownership", "2026-06-14").unwrap();
        assert_eq!(s2.stack, "general");
    }

    #[test]
    fn merge_sube_nivel_y_refresca_fecha_sin_duplicar() {
        let existing = vec![Skill {
            level: SkillLevel::Visto,
            topic: "Patrón Repository".into(),
            stack: "spring-boot".into(),
            last_seen: "2026-06-01".into(),
        }];
        // Mismo tema (distinta capitalización) sube a practicando y refresca fecha.
        let new = vec![Skill {
            level: SkillLevel::Practicando,
            topic: "patrón repository".into(),
            stack: "Spring-Boot".into(),
            last_seen: "2026-06-14".into(),
        }];
        let merged = merge(existing, new);
        assert_eq!(merged.len(), 1, "no debe duplicar el mismo tema");
        assert_eq!(merged[0].level, SkillLevel::Practicando);
        assert_eq!(merged[0].last_seen, "2026-06-14");

        // Un nivel inferior NO baja el dominio.
        let downgrade = vec![Skill {
            level: SkillLevel::Visto,
            topic: "Patrón Repository".into(),
            stack: "spring-boot".into(),
            last_seen: "2026-06-15".into(),
        }];
        let merged2 = merge(merged, downgrade);
        assert_eq!(merged2[0].level, SkillLevel::Practicando, "el nivel nunca baja");
    }

    #[test]
    fn markdown_ida_y_vuelta() {
        let skills = vec![
            Skill { level: SkillLevel::Dominado, topic: "DI".into(), stack: "spring-boot".into(), last_seen: "2026-06-14".into() },
            Skill { level: SkillLevel::Visto, topic: "Ownership".into(), stack: "rust".into(), last_seen: "2026-06-10".into() },
        ];
        let md = to_markdown(&skills);
        let back = from_markdown(&md);
        assert_eq!(back, skills);
    }

    #[test]
    fn repaso_espaciado_marca_lo_viejo_no_dominado() {
        let today = "2026-06-14";
        let viejo_no_dominado = Skill { level: SkillLevel::Visto, topic: "A".into(), stack: "x".into(), last_seen: "2026-06-01".into() };
        let viejo_dominado = Skill { level: SkillLevel::Dominado, topic: "B".into(), stack: "x".into(), last_seen: "2026-06-01".into() };
        let reciente = Skill { level: SkillLevel::Visto, topic: "C".into(), stack: "x".into(), last_seen: "2026-06-14".into() };
        assert!(needs_review(&viejo_no_dominado, today));
        assert!(!needs_review(&viejo_dominado, today), "lo dominado no se repasa");
        assert!(!needs_review(&reciente, today), "lo reciente no se repasa");
    }
}
