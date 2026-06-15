//! Currículo por stack: el TEMARIO que el modo `learn` usa para guiar al usuario.
//!
//! Es a la enseñanza lo que el Focus Pack es al grounding: una progresión curada
//! de conceptos por stack, en orden de dependencia. El modo learn la inyecta en
//! su prompt para SUGERIR qué aprender después ("ya viste X, sigue Y") y el
//! comando `/temario` la cruza con el progreso real (`skill::Skill`) para
//! mostrar cuánto llevas. Los temas que el usuario aprenda FUERA del temario se
//! capturan igual vía `dpx:skill` — el temario da estructura, no es una jaula.

/// Un tema del temario: nombre (debe casar con el `tema` de los bloques
/// `dpx:skill` para cruzar progreso) y una línea de para-qué.
pub struct Topic {
    pub name: &'static str,
    pub blurb: &'static str,
}

/// Temario curado de un focus. Vacío si el stack no tiene uno (mentor general).
pub fn topics(focus_id: Option<&str>) -> &'static [Topic] {
    match focus_id {
        Some("spring-boot") => SPRING_BOOT,
        Some("react") => REACT,
        Some("node") => NODE,
        Some("python") => PYTHON,
        Some("rust") => RUST,
        Some("dpx") => DPX,
        _ => &[],
    }
}

macro_rules! topic {
    ($name:expr, $blurb:expr) => {
        Topic { name: $name, blurb: $blurb }
    };
}

const SPRING_BOOT: &[Topic] = &[
    topic!("Inyección de dependencias e IoC", "por qué Spring crea tus objetos por ti"),
    topic!("Arquitectura en capas (controller/service/repository)", "separar responsabilidades"),
    topic!("REST controllers y mapeo HTTP", "exponer endpoints con criterio"),
    topic!("Spring Data JPA y repositorios", "persistencia sin SQL a mano"),
    topic!("Bean Validation y manejo de errores", "validar entrada y @ControllerAdvice"),
    topic!("Perfiles y configuración", "application.yml, @Value, entornos"),
    topic!("Testing (JUnit, MockMvc, @SpringBootTest)", "probar de unidad a integración"),
    topic!("Seguridad básica (Spring Security)", "autenticación y autorización"),
];

const RUST: &[Topic] = &[
    topic!("Ownership y borrowing", "el modelo de memoria que evita data races"),
    topic!("Option y Result, manejo de errores con ?", "errores sin excepciones"),
    topic!("Structs, enums y pattern matching", "modelar el dominio con tipos"),
    topic!("Traits y genéricos", "polimorfismo sin herencia"),
    topic!("Colecciones e iteradores", "transformar datos de forma perezosa"),
    topic!("Lifetimes", "cuándo y por qué anotarlos"),
    topic!("Concurrencia y async con tokio", "tareas, async/await, select"),
    topic!("Módulos y tests", "organizar el crate y probarlo"),
];

const REACT: &[Topic] = &[
    topic!("Componentes y JSX", "la unidad de UI"),
    topic!("Props y composición", "pasar datos y componer"),
    topic!("Estado con useState", "el estado local que re-renderiza"),
    topic!("Efectos con useEffect", "sincronizar con el mundo externo"),
    topic!("Listas y keys", "renderizar colecciones bien"),
    topic!("Manejo de formularios", "inputs controlados y validación"),
    topic!("Fetching de datos (TanStack Query)", "estado de servidor sin dolor"),
    topic!("Custom hooks", "extraer y reutilizar lógica"),
];

const NODE: &[Topic] = &[
    topic!("Módulos y el event loop", "cómo corre JS en el servidor"),
    topic!("Async/await y promesas", "asincronía sin callbacks anidados"),
    topic!("HTTP server (Fastify/Express)", "rutas, request/response"),
    topic!("Middleware", "la cadena de procesamiento"),
    topic!("Validación con zod", "validar y tipar la entrada"),
    topic!("Manejo de errores", "errores async y centralizados"),
    topic!("Acceso a datos", "querys y conexiones"),
    topic!("Testing", "probar handlers y lógica"),
];

const PYTHON: &[Topic] = &[
    topic!("Tipado y Pydantic v2", "modelos validados con type hints"),
    topic!("Rutas y parámetros en FastAPI", "path, query y body"),
    topic!("Inyección de dependencias de FastAPI", "Depends y sus usos"),
    topic!("Modelos y SQLAlchemy 2.0", "persistencia moderna"),
    topic!("Validación y manejo de errores", "HTTPException y validadores"),
    topic!("Async en FastAPI", "cuándo async sí y cuándo no"),
    topic!("Testing con pytest", "fixtures y TestClient"),
];

const DPX: &[Topic] = &[
    topic!("Arquitectura por capas del CLI", "cli/agent/focus/fs y qué hace cada una"),
    topic!("El loop agéntico (run_turn)", "rondas, tool calls y confirmaciones"),
    topic!("Tool-calling nativo", "ToolDefinition, parse_call y el dispatch"),
    topic!("Persistencia en .dpx/", "context.md, plan.md, skills.md"),
    topic!("Focus packs y prompts por capas", "identidad + tools + skills + modo"),
    topic!("El editor TUI con crossterm", "modo raw, eventos y render"),
];

/// La sección de temario que se inyecta en el prompt del modo learn (para que el
/// tutor sepa qué proponer y en qué orden). Vacía si el stack no tiene temario.
pub fn prompt_section(focus_id: Option<&str>) -> String {
    let topics = topics(focus_id);
    if topics.is_empty() {
        return String::new();
    }
    let name = super::display_name(focus_id);
    let mut s = format!(
        "\n\n# Temario sugerido de {name}\n\
         Guía al usuario por estos conceptos, idealmente en orden (cada uno apoya al \
         siguiente). Cuando termines uno o el usuario lo domine, MENCIONA cuál sigue y por qué. \
         Si pide otro tema, enséñalo igual y regístralo con `dpx:skill`. Usa estos MISMOS nombres \
         de tema en los bloques `dpx:skill` para que su progreso se acumule:\n"
    );
    for (i, t) in topics.iter().enumerate() {
        s.push_str(&format!("{}. {} — {}\n", i + 1, t.name, t.blurb));
    }
    s
}

/// ¿Casa un tema del temario con una habilidad registrada? Compara por nombre
/// normalizado, tolerando que uno contenga al otro (el tutor no siempre copia
/// el nombre literal).
fn matches(topic: &str, skill_topic: &str) -> bool {
    let a = topic.trim().to_lowercase();
    let b = skill_topic.trim().to_lowercase();
    a == b || a.contains(&b) || b.contains(&a)
}

/// Renderiza el temario del focus cruzado con el progreso del usuario, para
/// `/temario`: marca cada tema con su nivel y resalta el SIGUIENTE sugerido.
pub fn render(focus_id: Option<&str>, skills: &[crate::skill::Skill]) -> String {
    use crate::skill::SkillLevel;
    use crate::ui;

    let topics = topics(focus_id);
    let name = super::display_name(focus_id);
    if topics.is_empty() {
        return format!(
            "{}\n  {}",
            ui::accent("⏺ temario"),
            ui::dim(&format!(
                "el enfoque «{name}» no tiene temario guiado. Cambia de /enfoque o aprende libre en /modo learn."
            ))
        );
    }

    // Nivel de cada tema según las skills registradas (None = aún no tocado).
    let level_of = |topic: &str| -> Option<SkillLevel> {
        skills
            .iter()
            .filter(|s| matches(topic, &s.topic))
            .map(|s| s.level)
            .max()
    };

    let done = topics.iter().filter(|t| level_of(t.name) == Some(SkillLevel::Dominado)).count();
    let mut out = String::new();
    out.push_str(&format!("{} {}\n", ui::accent("⏺ temario ·"), ui::accent(name)));
    out.push_str(&format!("  {}\n\n", ui::dim(&format!("{done}/{} temas dominados", topics.len()))));

    let mut next_suggested: Option<&str> = None;
    for t in topics {
        let lvl = level_of(t.name);
        let icon = match lvl {
            Some(l) => {
                if l == SkillLevel::Dominado { ui::green(l.icon()) } else { ui::dim(l.icon()) }
            }
            None => ui::dim("○"),
        };
        // El siguiente sugerido: el primer tema no dominado.
        let mark = if next_suggested.is_none() && lvl != Some(SkillLevel::Dominado) {
            next_suggested = Some(t.name);
            ui::accent("  ← siguiente")
        } else {
            String::new()
        };
        out.push_str(&format!("  {icon} {}{mark}\n", t.name));
    }

    if let Some(next) = next_suggested {
        out.push_str(&format!(
            "\n  {}\n",
            ui::dim(&format!("en /modo learn, pídele: «enséñame {next}»"))
        ));
    } else {
        out.push_str(&format!("\n  {}\n", ui::green("¡completaste el temario! 🎉")));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temarios_existen_para_los_stacks_con_pack() {
        for id in ["spring-boot", "react", "node", "python", "rust", "dpx"] {
            assert!(!topics(Some(id)).is_empty(), "{id} debe tener temario");
        }
        assert!(topics(None).is_empty());
        assert!(topics(Some("gradle")).is_empty());
    }

    #[test]
    fn prompt_section_lista_los_temas_o_va_vacia() {
        let s = prompt_section(Some("rust"));
        assert!(s.contains("Temario sugerido"));
        assert!(s.contains("Ownership"));
        assert!(s.contains("dpx:skill"), "debe pedir usar los mismos nombres en dpx:skill");
        assert!(prompt_section(None).is_empty());
    }
}
