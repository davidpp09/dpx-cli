//! Focus Packs: el conocimiento embebido por dominio.
//!
//! Cada pack aporta las "skills" de un stack concreto. El system prompt final
//! que recibe el modelo se compone de tres capas:
//!
//! 1. **Identidad** — según el modo: `learn` usa el mentor que enseña;
//!    `code`/`hack` usan el agente que hace el trabajo.
//! 2. **Focus** — el conocimiento del dominio activo (ej. Spring Boot).
//! 3. **Modo** — el rol activo (code = agente · hack = rápido con criterio ·
//!    learn = tutor socrático). Los tres piensan a fondo y hacen las cosas bien.
//!
//! Y, si existe, se le añade la **memoria del proyecto** de sesiones anteriores.

pub mod committee;
pub mod curriculum;
mod dpx;
mod node;
mod python;
mod react;
mod rust;
mod spring_boot;

use anyhow::{Result, bail};
use clap::ValueEnum;

/// Modo de trabajo de dpx: el ÚNICO eje de comportamiento. Los tres piensan a
/// fondo y hacen las cosas BIEN; lo que cambia es el ROL, no la calidad.
#[derive(Clone, Copy, Debug, PartialEq, ValueEnum)]
pub enum Mode {
    /// Agente autónomo: escribe, ejecuta, itera y corrige hasta dejarlo robusto.
    Code,
    /// Constructor rápido: defaults sensatos y mínimo boilerplate, pero con
    /// criterio — código correcto que corre ya, no chapuza.
    Hack,
    /// Tutor socrático que te hace pensar y te enseña conceptos, patrones y
    /// arquitectura. NO escribe el código por ti: te guía a escribirlo.
    Learn,
}

impl Mode {
    /// Parsea el nombre de un modo (para `/mode <id>` y la config).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "code" | "agente" => Some(Mode::Code),
            "hack" => Some(Mode::Hack),
            "learn" | "aprender" => Some(Mode::Learn),
            _ => None,
        }
    }

    /// Identificador corto en minúsculas (el de `/mode` y la config).
    pub fn name(self) -> &'static str {
        match self {
            Mode::Code => "code",
            Mode::Hack => "hack",
            Mode::Learn => "learn",
        }
    }
}

/// Metadatos de un focus pack, para listarlos.
pub struct Focus {
    pub id: &'static str,
    pub name: &'static str,
    pub tagline: &'static str,
}

/// Catálogo de enfoques disponibles. Los stacks sin pack dedicado arrancan
/// como mentor general (sin sección de dominio en el prompt).
pub fn catalog() -> Vec<Focus> {
    vec![
        Focus {
            id: "spring-boot",
            name: "Spring Boot",
            tagline: "Backend Java/Spring con criterio de arquitecto.",
        },
        Focus {
            id: "react",
            name: "React",
            tagline: "Frontend React moderno (Vite, TanStack Query, RTL).",
        },
        Focus {
            id: "node",
            name: "Node.js",
            tagline: "Backend JavaScript/TypeScript (Fastify/Express, zod).",
        },
        Focus {
            id: "python",
            name: "Python",
            tagline: "Backend Python con FastAPI (Pydantic v2, SQLAlchemy 2).",
        },
        Focus {
            id: "rust",
            name: "Rust",
            tagline: "Sistemas y CLIs en Rust (anyhow, tokio, clap).",
        },
        Focus {
            id: "gradle",
            name: "Gradle (JVM)",
            tagline: "Proyecto JVM con Gradle (sin pack dedicado aún).",
        },
        Focus {
            id: "dpx",
            name: "dpx (auto-edición)",
            tagline: "El propio dpx: arquitectura interna, UI y cómo añadirse features.",
        },
    ]
}

/// Imprime el catálogo de enfoques (comando `dpx focus`).
pub fn print_catalog() {
    println!("Enfoques disponibles:\n");
    for f in catalog() {
        println!("  \x1b[1m{:<14}\x1b[0m {}", f.id, f.tagline);
    }
    println!("\nUsa:  dpx chat --focus <id>");
}

/// Construye el system prompt completo para una sesión, según el modo.
/// La IDENTIDAD se deriva del modo: `learn` es el mentor que enseña; `code` y
/// `hack` son el agente que construye. Con `focus_id = None` (o un stack sin
/// pack dedicado) el prompt va sin sección de dominio: identidad + herramientas + modo.
pub fn system_prompt(focus_id: Option<&str>, mode: Mode, prior: Option<&str>) -> Result<String> {
    let domain = match focus_id {
        None => None,
        Some(id) => {
            if !catalog().iter().any(|f| f.id == id) {
                bail!("Enfoque desconocido: '{id}'. Ejecuta `dpx focus` para ver los disponibles.");
            }
            domain_skills(id)
        }
    };

    let mut s = String::new();
    // En `learn` manda el mentor (enseña, tú escribes); en `code`/`hack` manda
    // el agente que hace el trabajo. La calidad es la misma; cambia el rol.
    s.push_str(match mode {
        Mode::Learn => MENTOR_IDENTITY,
        Mode::Code | Mode::Hack => CODE_IDENTITY,
    });
    s.push_str("\n\n");
    s.push_str(SHARED_TOOLS);
    s.push_str("\n\n");
    s.push_str(AGENTIC_SKILLS);
    if let Some(d) = domain {
        s.push_str("\n\n");
        s.push_str(d);
    }
    s.push_str("\n\n");
    s.push_str(mode_addendum(mode));

    // En modo aprender, el temario del stack (si lo hay) guía qué enseñar y en
    // qué orden — el equivalente pedagógico del Focus Pack.
    if mode == Mode::Learn {
        s.push_str(&curriculum::prompt_section(focus_id));
    }

    if let Some(p) = prior {
        let p = p.trim();
        if !p.is_empty() {
            s.push_str(
                "\n\n# Memoria de este proyecto (sesiones anteriores)\n\
                 Esto es lo que ya sabes de este proyecto y del progreso del usuario. \
                 Retoma desde aquí: no repitas lo que ya le enseñaste, continúa desde \
                 los próximos pasos pendientes.\n\n",
            );
            s.push_str(p);
        }
    }

    Ok(s)
}

/// Las skills embebidas de un focus, si tiene pack dedicado.
fn domain_skills(focus_id: &str) -> Option<&'static str> {
    match focus_id {
        "spring-boot" => Some(spring_boot::SKILLS),
        "react" => Some(react::SKILLS),
        "node" => Some(node::SKILLS),
        "python" => Some(python::SKILLS),
        "rust" => Some(rust::SKILLS),
        "dpx" => Some(dpx::SKILLS),
        _ => None,
    }
}

/// Devuelve el nombre legible de un focus pack (para banners).
/// `None` = mentor general, sin enfoque de stack.
pub fn display_name(focus_id: Option<&str>) -> &str {
    let Some(id) = focus_id else {
        return "general";
    };
    catalog()
        .into_iter()
        .find(|f| f.id == id)
        .map(|f| f.name)
        .unwrap_or(id)
}

/// Identidad del mentor (learn): ensena, guia, deja escribir.
const MENTOR_IDENTITY: &str = "\
# Identidad
Eres DPX, un mentor senior de ingenieria. Ensenas a pensar como profesional: \
explicas el *porque*, dejas que el usuario escriba, y tienes criterio para \
senalar malas practicas aunque no te pregunten.";

/// Identidad del agente autonomo (code/hack): hace, verifica, itera.
const CODE_IDENTITY: &str = "\
# Identidad
Eres DPX CODE, un agente de ingenieria AUTONOMO. Implementas, ejecutas, verificas \
e iteras hasta que la tarea queda lista. Escribes codigo COMPLETO, lo compilas, \
reaccionas a la salida real. Autonomo pero seguro: lees y buscas libremente, \
para escribir o ejecutar el CLI pide confirmacion. Criterio de staff engineer.";

/// Herramientas y reglas comunes a los tres modos.
const SHARED_TOOLS: &str = "\
# Memoria
Tienes memoria en `.dpx/context.md`. No empieces de cero.

# Herramientas (function calling)
`read_file` `search_project` `web_search` `web_fetch` `spawn_agent` \
`write_file` `edit_file` `delete_file` `run_command` \
`git_status` `git_diff` `git_log` `git_commit`

`write_file` = archivos NUEVOS. `edit_file` = existentes (`search` = texto EXACTO del archivo real). \
`web_search` = usalo siempre, no inventes APIs ni versiones.";

/// Criterio agentico: COMO decidir entre herramientas y CUANDO parar.
const AGENTIC_SKILLS: &str = "\
# Criterio agentico
`edit_file` por fragmentos, NUNCA reescribas >200 lineas. \
`search_project` antes de leer media codebase. \
`spawn_agent`: 12x mas barato, contexto aislado. Delega en paralelo.

Edita TEMPRANO. Pide TODAS las lecturas en UN turno. \
Slice minimo que compile; `dpx:plan` si hay 2+ archivos.

Un cambio → verificar → siguiente. Tests incluidos. Build/tests ROJO: no cierres. \
Salida del comando > tu plan. Accion destructiva no pedida: pregunta.";

/// El addendum segun el modo.
fn mode_addendum(mode: Mode) -> &'static str {
    match mode {
        Mode::Code => "\
# Modo activo: CODE (agente autonomo)
- Tu HACES el trabajo: implementas, ejecutas, verificas y dejas funcionando.
- Codigo ROBUSTO: validacion, errores y tests incluidos.
- Explica cada decision clave y sus trade-offs en 1-2 frases; luego actua.",
        Mode::Hack => "\
# Modo activo: HACK (rapido CON criterio, SIN sobre-escopar)
- ESCALA al pedido: haz EXACTAMENTE lo pedido, en el MINIMO de codigo/archivos que \
lo resuelva completo. 'hola mundo' = UN archivo, no un proyecto. NO agregues tests, \
config, infra, tipos ni archivos que no se pidieron, salvo que la tarea los exija.
- Calidad NO es cantidad: 'bien hecho' = lo PEDIDO correcto, claro y corriendo, no \
hacer de mas. El criterio esta en el COMO, no en el CUANTO.
- Si algo extra suma de verdad, PROPONLO en una linea y deja que el usuario decida; \
no lo impongas.",
        Mode::Learn => LEARN_MODE,
    }
}

/// Modo APRENDER: tutor socratico. Productive struggle + conceptos reales.
const LEARN_MODE: &str = "\
# Modo activo: APRENDER (tutor socratico)
No resuelves: enseñas. El usuario aprende HACIENDO.

Metodo:
1. NO des la solucion; guia para que el usuario la escriba.
2. Pregunta que sabe y ajusta profundidad a su respuesta.
3. Pistas graduales: la minima que desbloquee.
4. Error → pregunta que lo lleve al error, no correccion directa.
5. Cierra con pregunta de repaso (retrieval practice).

Enseña el concepto con nombre real + por que existe + cuando NO usarlo. \
PATRONES, ARQUITECTURA, TRADE-OFFS. SOLID cuando aplique. \
No uses write_file/edit_file para entregar solucion; si puedes read_file su codigo.

Registro al final del turno:
```dpx:skill
dominado | concepto | stack
practicando | concepto | stack
```
Niveles: `visto` `practicando` `dominado`. Omite el bloque si no se trabajo ningun concepto.";

// Contrato de skills curados (code/hack): playbooks en `skills/*.md`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_dpx_existe_y_se_inyecta() {
        // El catálogo lo lista (si no, system_prompt lo rechazaría al validar).
        assert!(catalog().iter().any(|f| f.id == "dpx"));
        // Tiene skills de dominio.
        assert!(domain_skills("dpx").is_some());
        // Y el system_prompt con focus "dpx" no falla e incluye el pack.
        let p = system_prompt(Some("dpx"), Mode::Code, None).unwrap();
        assert!(p.contains("AUTO-EDICIÓN"), "el pack dpx debe estar en el prompt");
        assert!(p.contains("src/cli/editor.rs"), "debe incluir el grounding de UI");
    }

    #[test]
    fn focus_desconocido_falla() {
        assert!(system_prompt(Some("noexiste"), Mode::Code, None).is_err());
    }

    #[test]
    fn modo_code_usa_identidad_de_agente_y_learn_la_de_mentor() {
        // code/hack → agente que HACE; learn → mentor que enseña.
        let code = system_prompt(None, Mode::Code, None).unwrap();
        assert!(code.contains("DPX CODE"), "code debe usar la identidad de agente");
        let learn = system_prompt(None, Mode::Learn, None).unwrap();
        assert!(!learn.contains("DPX CODE"), "learn NO debe usar la identidad de agente");
        // Mode parsea sus tres nombres y rechaza basura.
        assert_eq!(Mode::parse("code"), Some(Mode::Code));
        assert_eq!(Mode::parse("hack"), Some(Mode::Hack));
        assert_eq!(Mode::parse("learn"), Some(Mode::Learn));
        assert_eq!(Mode::parse("pro"), None);
    }

    #[test]
    fn modo_learn_es_socratico_y_pide_skill() {
        let add = mode_addendum(Mode::Learn);
        // El metodo socratico: prohibe dar la solucion de entrada.
        assert!(add.contains("NO des la solucion") || add.contains("NO ESCUPAS"));
        // Ensena arquitectura/patrones, no solo sintaxis.
        assert!(add.contains("SOLID") || add.contains("arquitectura") || add.contains("ARQUITECTURA"));
        // Y registra progreso con el bloque dpx:skill.
        assert!(add.contains("dpx:skill"));
        // Se integra en el prompt completo sin romper.
        let p = system_prompt(Some("spring-boot"), Mode::Learn, None).unwrap();
        assert!(p.contains("APRENDER"));
    }
}
