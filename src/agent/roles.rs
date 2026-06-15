//! Roster de roles para los subagentes (`spawn_agent`).
//!
//! Todos los roles son de **SOLO LECTURA** (la frontera de seguridad de los
//! subagentes: investigan y opinan, no mutan) y corren en el cerebro barato
//! (DeepSeek flash, sin thinking — 12× más barato que el pro). La idea: delegar
//! agresivamente trabajo acotado a un subagente aislado en vez de quemar el
//! contexto —y el dinero— del agente principal. Cada rol aporta una identidad
//! enfocada: un revisor piensa distinto de un investigador, y esa
//! especialización sube la fiabilidad del resultado.

/// Un rol de subagente. Cambia la IDENTIDAD del preamble, no sus permisos:
/// todos siguen siendo de solo lectura.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentRole {
    /// Localiza código, mapea flujos, recopila contexto disperso. El default.
    Researcher,
    /// Ante un cambio que pide el usuario, averigua DÓNDE y CÓMO se hace en el
    /// código (archivos/funciones a tocar, orden, cabos a no olvidar).
    Planner,
    /// Revisa un archivo o diff buscando bugs, casos borde y malas prácticas.
    Reviewer,
    /// Propone casos de test (camino feliz + bordes) para una función o módulo.
    TestDesigner,
    /// Forma hipótesis de la causa raíz de un error a partir de la evidencia.
    Debugger,
    /// Evalúa un diseño o trade-off de arquitectura y recomienda.
    Architect,
    /// Busca inconsistencias entre el código y la documentación/comentarios.
    DocAuditor,
}

impl AgentRole {
    /// Todos los roles, en orden de uso típico.
    pub fn all() -> [AgentRole; 7] {
        [
            AgentRole::Researcher,
            AgentRole::Planner,
            AgentRole::Reviewer,
            AgentRole::TestDesigner,
            AgentRole::Debugger,
            AgentRole::Architect,
            AgentRole::DocAuditor,
        ]
    }

    /// Nombre estable (lo que el modelo pasa en el parámetro `role`).
    pub fn name(self) -> &'static str {
        match self {
            AgentRole::Researcher => "researcher",
            AgentRole::Planner => "planner",
            AgentRole::Reviewer => "reviewer",
            AgentRole::TestDesigner => "test-designer",
            AgentRole::Debugger => "debugger",
            AgentRole::Architect => "architect",
            AgentRole::DocAuditor => "doc-auditor",
        }
    }

    /// Parsea el nombre de un rol. Desconocido o ausente → `Researcher`, el
    /// default seguro (un subagente sin rol claro investiga).
    pub fn parse(s: Option<&str>) -> AgentRole {
        match s.map(|x| x.trim().to_ascii_lowercase()).as_deref() {
            Some("planner") | Some("plan") | Some("scout") | Some("planificador") => {
                AgentRole::Planner
            }
            Some("reviewer") => AgentRole::Reviewer,
            Some("test-designer") | Some("test_designer") | Some("tester") => {
                AgentRole::TestDesigner
            }
            Some("debugger") => AgentRole::Debugger,
            Some("architect") => AgentRole::Architect,
            Some("doc-auditor") | Some("doc_auditor") | Some("docs") => AgentRole::DocAuditor,
            _ => AgentRole::Researcher,
        }
    }


    /// Etiqueta legible para el banner.
    pub fn label(self) -> &'static str {
        match self {
            AgentRole::Researcher => "investigador",
            AgentRole::Planner => "planificador de cambios",
            AgentRole::Reviewer => "revisor de código",
            AgentRole::TestDesigner => "diseñador de tests",
            AgentRole::Debugger => "depurador",
            AgentRole::Architect => "arquitecto",
            AgentRole::DocAuditor => "auditor de docs",
        }
    }

    /// Una línea de para-qué-sirve (alimenta la descripción de la tool, para que
    /// el agente principal sepa a cuál delegar).
    pub fn blurb(self) -> &'static str {
        match self {
            AgentRole::Researcher => {
                "localiza código, mapea un flujo o recopila contexto disperso"
            }
            AgentRole::Planner => {
                "averigua DÓNDE y CÓMO hacer un cambio que pides (archivos/funciones + orden)"
            }
            AgentRole::Reviewer => "revisa un archivo/diff: bugs, casos borde, malas prácticas",
            AgentRole::TestDesigner => "propone casos de test (feliz + bordes) para una función",
            AgentRole::Debugger => "forma hipótesis de la causa raíz de un error",
            AgentRole::Architect => "evalúa un diseño o trade-off y recomienda",
            AgentRole::DocAuditor => "busca desajustes entre el código y los docs/comentarios",
        }
    }

    /// El fragmento de IDENTIDAD que se antepone al preamble común del subagente.
    /// Define cómo piensa el rol; las reglas de solo-lectura y formato las pone
    /// el preamble compartido (`subagent_preamble`).
    pub fn identity(self) -> &'static str {
        match self {
            AgentRole::Researcher => "\
Eres un SUBAGENTE INVESTIGADOR. Tu trabajo es localizar dónde se hace algo en el código, \
mapear un flujo o recopilar contexto disperso, y devolver los hechos con rutas y fragmentos.",
            AgentRole::Planner => "\
Eres un SUBAGENTE PLANIFICADOR DE CAMBIOS. El usuario describió un cambio que quiere, a menudo \
de forma INFORMAL y SIN decir en qué archivo va. Tu trabajo es averiguar, leyendo el código, \
DÓNDE y CÓMO se hace: localiza con search_project y el mapa de símbolos los archivos y funciones \
EXACTOS a tocar, el ORDEN de los cambios, y los puntos que se suelen olvidar (registrar un \
comando/tool, su ayuda, los prompts, los tests). Devuelve un PLAN concreto y accionable, en \
forma de pasos `archivo:función → qué cambiar`, para que el agente principal lo ejecute sin \
adivinar. NO escribes ni editas: solo localizas y planificas. Si el objetivo es ambiguo, di qué \
interpretaste y cuál es el punto a confirmar.",
            AgentRole::Reviewer => "\
Eres un SUBAGENTE REVISOR DE CÓDIGO senior. Lee lo que se te indica y reporta, en orden de \
gravedad, los problemas REALES: bugs de corrección, casos borde sin cubrir, condiciones de \
carrera, manejo de errores flojo y malas prácticas. Cita archivo:línea. No inventes problemas \
ni señales cuestiones de estilo triviales.",
            AgentRole::TestDesigner => "\
Eres un SUBAGENTE DISEÑADOR DE TESTS. Lee la función o módulo indicado y propone los casos de \
test que importan: el camino feliz y los BORDES (vacío, límites, error, entradas raras). \
Describe cada caso (entrada → salida esperada) de forma que el agente principal los escriba; \
NO escribes archivos, solo diseñas.",
            AgentRole::Debugger => "\
Eres un SUBAGENTE DEPURADOR. A partir del error, el código y la evidencia que reúnas, forma las \
hipótesis MÁS PROBABLES de la causa raíz, ordénalas por probabilidad y di qué leer o ejecutar \
para confirmarlas. Razona sobre el mecanismo, no adivines.",
            AgentRole::Architect => "\
Eres un SUBAGENTE ARQUITECTO. Evalúa el diseño o el trade-off planteado: opciones reales, sus \
pros/contras concretos en ESTE proyecto, y una recomendación con su porqué. Piensa en \
mantenibilidad y coste, no en lo que está de moda.",
            AgentRole::DocAuditor => "\
Eres un SUBAGENTE AUDITOR DE DOCUMENTACIÓN. Compara el código con su documentación (README, \
comentarios, ayuda, prompts) y reporta los DESAJUSTES: lo que el código hace pero los docs no \
dicen, y lo que los docs prometen pero el código ya no cumple. Cita ambas ubicaciones.",
        }
    }
}

/// Lista `nombre — para qué sirve` de todos los roles, para la descripción de la
/// tool `spawn_agent` (así el agente principal sabe a cuál delegar).
pub fn roster_blurb() -> String {
    AgentRole::all()
        .iter()
        .map(|r| format!("'{}' ({})", r.name(), r.blurb()))
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_default_es_researcher() {
        assert_eq!(AgentRole::parse(None), AgentRole::Researcher);
        assert_eq!(AgentRole::parse(Some("desconocido")), AgentRole::Researcher);
        assert_eq!(AgentRole::parse(Some("")), AgentRole::Researcher);
    }

    #[test]
    fn parse_reconoce_roles_y_alias() {
        assert_eq!(AgentRole::parse(Some("planner")), AgentRole::Planner);
        assert_eq!(AgentRole::parse(Some("scout")), AgentRole::Planner);
        assert_eq!(AgentRole::parse(Some("reviewer")), AgentRole::Reviewer);
        assert_eq!(AgentRole::parse(Some("TEST-DESIGNER")), AgentRole::TestDesigner);
        assert_eq!(AgentRole::parse(Some("tester")), AgentRole::TestDesigner);
        assert_eq!(AgentRole::parse(Some("docs")), AgentRole::DocAuditor);
        assert_eq!(AgentRole::parse(Some(" architect ")), AgentRole::Architect);
    }

    #[test]
    fn roster_y_identidades_no_vacias() {
        let roster = roster_blurb();
        for r in AgentRole::all() {
            assert!(roster.contains(r.name()), "el roster debe listar {}", r.name());
            assert!(!r.identity().trim().is_empty());
            assert!(!r.label().is_empty());
        }
    }
}
