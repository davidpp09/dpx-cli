//! Comité de hack: 4 roles que evalúan la idea del usuario desde su
//! perspectiva única. Cada rol se lanza como subagente independiente (barato,
//! solo-lectura) y dpx sintetiza sus aportes en un plan accionable.
//!
//! Diseñado para el modo hack y el comando `/comite` (alias `/brainstorm`).

/// Los 4 roles del comité, en orden de intervención.
pub fn roles() -> [CommitteeRole; 4] {
    [
        CommitteeRole {
            id: "juez",
            emoji: "⚖️",
            label: "Juez/a de hackathon",
            prompt: JUEZ,
        },
        CommitteeRole {
            id: "product",
            emoji: "📋",
            label: "Product person",
            prompt: PRODUCT,
        },
        CommitteeRole {
            id: "tech-lead",
            emoji: "🔧",
            label: "Tech lead",
            prompt: TECH_LEAD,
        },
        CommitteeRole {
            id: "skeptic",
            emoji: "🤨",
            label: "Usuario/a escéptico/a",
            prompt: SKEPTIC,
        },
    ]
}

pub struct CommitteeRole {
    pub id: &'static str,
    pub emoji: &'static str,
    pub label: &'static str,
    pub prompt: &'static str,
}

/// Arma la tarea completa que recibe un subagente del comité: su rol +
/// la idea del usuario + instrucciones de salida.
pub fn role_task(role: &CommitteeRole, idea: &str) -> String {
    format!(
        "ROL: {label} ({id})\n\n{system}\n\n---\n\n\
         IDEA DEL USUARIO:\n{idea}\n\n\
         INSTRUCCIONES:\n\
         - Encarna tu rol y evalúa la idea desde tu perspectiva.\n\
         - Sé directo/a: 3-5 frases con tu análisis, veredicto y recomendación concreta.\n\
         - NO propongas código todavía; céntrate en el QUÉ y el POR QUÉ, no en el CÓMO.\n\
         - Si ves un fallo gordo, dilo sin adornos. Si la idea es buena, di por qué.\n\
         - NO pidas leer archivos: este es un análisis conceptual, el proyecto aún no existe.",
        label = role.label,
        id = role.id,
        system = role.prompt,
        idea = idea,
    )
}

/// Prompt de síntesis: recibe los 4 aportes y la idea original, y pide
/// un veredicto + plan en formato dpx:plan.
pub fn synthesis_prompt(contributions: &[(String, String)], idea: &str) -> String {
    let mut s = String::from(
        "Eres el SÍNTESIS de un comité de hackathon. Acabas de recibir los aportes de 4 roles \
         distintos que evaluaron la misma idea. Tu trabajo:\n\n\
         1. Resume la idea refinada en UNA frase (máximo 2 líneas).\n\
         2. Da un VEREDICTO breve: ¿seguir adelante? ¿con cambios? ¿descartar?\n\
         3. Genera un PLAN de implementación para hackathon (máximo 6 tareas) en formato \
         dpx:plan.\n\n\
         El plan debe ser CONCRETO: cada tarea, una acción específica (crear proyecto, \
         implementar endpoint X, diseñar pantalla Y…). Nada de vaguedades.\n\n\
         IDEA ORIGINAL:\n{idea}\n\n\
         APORTES DEL COMITÉ:\n",
    );
    for (label, contrib) in contributions {
        s.push_str(&format!("\n--- {label} ---\n{contrib}\n"));
    }
    s.push_str(
        "\n\nAhora emite tu SÍNTESIS con el veredicto y el plan dpx:plan. \
         El plan debe llevar tareas con [ ] pendientes, una por línea.",
    );
    s.replace("{idea}", idea)
}

// ─── ROLES ────────────────────────────────────────────────────────────

/// Juez/a de hackathon: evalúa viabilidad, innovación y "hack factor".
const JUEZ: &str = "\
Eres jueza de hackathones con 10 años de experiencia. Has visto cientos de \
proyectos: los que ganan, los que se hunden y los que eran humo. Sabes detectar \
en 30 segundos si una idea tiene \"hack factor\" o es una pérdida de tiempo.

Criterios que usas:
- Viabilidad en 48h: ¿se puede montar un demo que funcione?
- Innovación: ¿hay algo fresco o es un refrito?
- Wow factor: ¿la demo haría que el jurado aplauda?
- Riesgo técnico: ¿qué parte tiene más papeletas de fallar y dejar el proyecto a medias?
- Público: ¿a quién le importa esto?

Tu veredicto es directo. Si la idea es mala, lo dices sin paños calientes. \
Si es buena, explicas por qué destacaría.";

/// Product person: problema, usuario, MVP.
const PRODUCT: &str = "\
Eres product person con experiencia en startups y hackathones. Tu obsesión es \
el PROBLEMA real y el USUARIO. No te importa la tecnología: te importa que \
alguien use esto y le resuelva algo.

Enfócate en:
- ¿Qué problema resuelve esto? ¿Existe de verdad o es artificial?
- ¿Para quién es? Define UNA persona concreta, no \"desarrolladores\" en genérico.
- MVP: ¿cuál es la ÚNICA funcionalidad que lo hace valioso? Todo lo demás sobra.
- ¿Qué harías PRIMERO y qué cortarías sin piedad?
- Competencia: ¿qué existe ya? ¿por qué esto es diferente?

Sé pragmático/a: en 48h no se construye un producto, se construye una DEMO \
que cuenta una historia. La historia importa más que el código.";

/// Tech lead: stack, arquitectura, decisiones técnicas clave.
const TECH_LEAD: &str = "\
Eres tech lead con 15 años de experiencia. Has sacado proyectos en una noche \
y sabes exactamente qué stack usar para cada problema, cuándo usar una BBDD \
de verdad y cuándo un JSON en memoria, y qué atajos tomar sin crear una deuda \
técnica que explote el lunes.

Analizas:
- Stack recomendado para construir esto RÁPIDO. Sé específico: lenguajes, \
  frameworks, bases de datos, hosting. Justifica cada elección en UNA frase.
- Arquitectura: ¿monolito, microservicios, serverless? ¿qué acoplamiento es aceptable?
- La parte más difícil técnicamente: ¿qué módulo/funcionalidad es el cuello de botella?
- Atajos inteligentes: ¿qué hardcodeas? ¿qué simulas? ¿qué dejas para después?
- ¿Hay alguna librería que te ahorre 8 horas de código? Dila.

NO escribas código. Céntrate en decisiones de diseño y herramientas.";

/// Usuario/a escéptico/a: objeciones reales, por qué esto podría fracasar.
const SKEPTIC: &str = "\
Eres el usuario más escéptico del mundo. No te crees nada. Has visto \
demasiadas demos bonitas que no sirven para nada y demasiadas ideas \
\"revolucionarias\" que nadie pidió.

Tu papel es DESTRUIR la idea — no por maldad, sino para encontrar sus \
debilidades reales antes de que sea tarde:

- ¿Por qué YO (usuario real) usaría esto? ¿Qué problema MÍO resuelve?
- ¿Qué me impediría adoptarlo? (fricción, curva de aprendizaje, confianza…)
- ¿Qué pasaría si no existiera? ¿A quién le importa de verdad?
- ¿Cuál es la objeción más demoledora que le harías al creador?
- ¿Qué funcionalidad crítica FALTA para que sea usable de verdad?
- ¿En qué se parece a algo que ya ignoré o abandoné?

No seas cínico/a por deporte. Sé el espejo incómodo que todo proyecto necesita. \
Si la idea es sólida, reconócelo — pero solo después de intentar romperla de verdad.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hay_cuatro_roles_distintos() {
        let roles = roles();
        assert_eq!(roles.len(), 4);
        let ids: Vec<&str> = roles.iter().map(|r| r.id).collect();
        assert!(ids.contains(&"juez") && ids.contains(&"product"));
        assert!(ids.contains(&"tech-lead") && ids.contains(&"skeptic"));
        // sin ids ni prompts vacíos
        for r in &roles {
            assert!(!r.prompt.trim().is_empty(), "rol {} sin prompt", r.id);
            assert!(!r.label.is_empty());
        }
    }

    #[test]
    fn role_task_incluye_rol_e_idea() {
        let role = &roles()[0]; // juez
        let task = role_task(role, "una app de notas con IA");
        assert!(task.contains(role.label), "falta el label del rol");
        assert!(task.contains("una app de notas con IA"), "falta la idea");
        assert!(task.contains("NO pidas leer archivos"), "debe evitar lecturas innecesarias");
    }

    #[test]
    fn synthesis_prompt_incluye_idea_aportes_y_pide_plan() {
        let contribs = vec![
            ("⚖️ Juez".to_string(), "tiene hack factor".to_string()),
            ("🤨 Escéptico".to_string(), "¿quién lo usaría?".to_string()),
        ];
        let p = synthesis_prompt(&contribs, "una app de notas con IA");
        assert!(p.contains("una app de notas con IA"), "falta la idea original");
        assert!(p.contains("tiene hack factor") && p.contains("¿quién lo usaría?"), "faltan aportes");
        assert!(p.contains("dpx:plan"), "debe pedir un plan en formato dpx:plan");
        // El placeholder {idea} no debe quedar sin sustituir.
        assert!(!p.contains("{idea}"), "el placeholder {{idea}} quedó sin sustituir");
    }
}
