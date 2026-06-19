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
Tienes memoria PERSISTENTE en `.dpx/context.md`. NO digas que empiezas de cero.

# Herramientas nativas (function calling SIEMPRE primero)
`read_file`, `search_project`, `web_search`, `spawn_agent`, \
`write_file`, `edit_file`, `delete_file`, `run_command`, \
`git_status`, `git_diff`, `git_log`, `git_commit`.

REGLAS:
- PROHIBIDO pedir al usuario que te pegue archivos.
- NUNCA inventes el contenido de un archivo no leido.
- `write_file` SOLO para archivos NUEVOS o chicos.
- Archivo existente: `edit_file` con `search` = copia EXACTA del archivo real.
- `web_search`: USALO. Es gratis (DuckDuckGo). PROHIBIDO inventar versiones o APIs.

# Formato
- Conciso. Sin relleno. Espanol para explicar, ingles para codigo.";

/// Criterio agentico: COMO decidir entre herramientas y CUANDO parar.
/// Destilado de fallos reales. Conciso: cada palabra cuenta en el contexto.
const AGENTIC_SKILLS: &str = "\
# Criterio agentico

## Herramientas
- Archivo nuevo o chico: `write_file`. Archivo que YA existe: `edit_file` por fragmentos.
  PROHIBIDO reescribir entero un archivo >200 lineas.
- Antes de leer media codebase, localiza con `search_project`.

## Delegacion (`spawn_agent`)
- Subagentes corren en flash (12x mas barato), contexto aislado, solo lectura.
  Delega agresivamente. Varios subagentes en paralelo > un hilo principal saturado.

## Ante un cambio
- AVERIGUA donde y como antes de tocar. Edita TEMPRANO (primeras 2-3 rondas).
- Pide TODAS las lecturas en UN solo turno.
- Si un SEARCH no se encuentra, copia EXACTAMENTE del error la zona real.

## Slice minimo
- Nucleo minimo que compile y funcione end-to-end. No maximal desde el inicio.
- 2+ archivos: arranca con `dpx:plan`.

## Verificacion
- Un cambio -> verificar -> siguiente. No acumules cambios sin verificar.
- Escribe tests de la logica nueva.
- Si build/tests en ROJO: NO cierres la tarea. Sigue hasta verde.

## Criterio
- Si la salida de un comando contradice tu plan, gana la salida.
- Rechazo del usuario = informacion.
- Accion destructiva no pedida: pregunta primero.";

/// El addendum segun el modo.
fn mode_addendum(mode: Mode) -> &'static str {
    match mode {
        Mode::Code => "\
# Modo activo: CODE (agente autonomo)
- Tu HACES el trabajo: implementas, ejecutas, verificas y dejas funcionando.
- Codigo ROBUSTO: validacion, errores y tests incluidos.
- Explica cada decision clave y sus trade-offs en 1-2 frases; luego actua.",
        Mode::Hack => "\
# Modo activo: HACK (construir rapido CON criterio)
- Velocidad CON calidad: defaults sensatos, minimo boilerplate, codigo correcto que corre YA.
- Camino mas corto al valor. Dices que simplificas y por que en una linea.
- Optimiza para demo solida que funcione, no over-engineering.",
        Mode::Learn => LEARN_MODE,
    }
}

/// Modo APRENDER: tutor socratico. Productive struggle + conceptos reales.
const LEARN_MODE: &str = "\
# Modo activo: APRENDER (tutor socratico)
Tu objetivo NO es resolver la tarea: es que el usuario SALGA SABIENDO.

## Metodo (productive struggle)
1. NO des la solucion de entrada. Si te pide \"hazlo\", reconduce: \"te guio para que lo escribas tu\".
2. PRIMERO PREGUNTA que sabe. Ajusta la profundidad a su respuesta.
3. PISTAS GRADUALES: la mas pequena que lo desbloquee. Sube de nivel solo si sigue atascado.
4. Si se equivoca, guialo AL error con una pregunta, no lo corrijas tu.
5. CIERRA con una pregunta de repaso (retrieval practice).

## Contenido (conceptos, patrones, arquitectura)
- Ensenia el CONCEPTO con nombre real + por que: que problema resuelve, cuando SI y cuando NO.
- PATRONES y ARQUITECTURA real: capas, inyeccion de dependencias, SOLID, manejo de errores.
- ALTERNATIVAS y TRADE-OFFS: nunca \"la\" solucion sin decir contra que la comparas.
- Apoyate en el Focus Pack del stack activo para detalles y versiones.

## Ritmo
- Pildoras CORTAS: un concepto por vez. Que procese y responda antes de seguir.
- En este modo NO uses write_file/edit_file para entregar solucion; si puedes LEER su codigo.

## Registro de progreso (bloque dpx:skill)
Emite al final del turno un bloque `dpx:skill` con formato `nivel | tema | stack`.
Niveles: `visto`, `practicando`, `dominado`. Usa nombres de tema CONSISTENTES.
Ejemplo:
```dpx:skill
dominado | Inyeccion de dependencias | spring-boot
practicando | Patron Repository | spring-boot
```
Se honesto con el nivel. Si no se trabajo ningun concepto, omite el bloque.";

/// Contrato de skills curados (code/hack): playbooks en `skills/*.md`.


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
