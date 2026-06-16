//! Recuperación de contexto/skills, auto-delegación y subagentes. Extraído de `chat`.

use std::path::Path;

use rig_core::completion::Message;

use super::{ensure_embedder, subagent_brain, truncate_log};
use crate::agent::tools::{self, DpxCall};
use crate::agent::{ChatReply, ModelRouter};
use crate::ui;

/// Recupera de la memoria de largo plazo los fragmentos relevantes a `input` y
/// los formatea como un bloque de contexto para anteponer al turno. Devuelve
/// `None` si no hay memoria, si el motor no carga, o si nada supera el umbral —
/// en todos esos casos el turno sigue normal (degradación elegante, sin ruido).
pub(crate) fn recall_context(
    input: &str,
    mem: &crate::memory::MemoryStore,
    emb: &mut Option<crate::memory::Embedder>,
) -> Option<String> {
    if mem.is_empty() {
        return None; // sin memoria → ni siquiera cargamos el modelo
    }
    let engine = ensure_embedder(emb).ok()?;
    let query_vec = engine.embed_one(input).ok()?;
    let hits = mem.search(&query_vec, 3, 0.45);
    if hits.is_empty() {
        return None;
    }
    let mut s = String::from(
        "[memoria de largo plazo — fragmentos que recordaste de antes, relevantes a la \
         consulta del usuario (úsalos si ayudan; ignóralos si no):\n",
    );
    for (_, e) in hits {
        s.push_str(&format!("- {}\n", e.text));
    }
    s.push(']');
    Some(s)
}

/// Recupera los playbooks CURADOS (`skills/*.md`) que encajan con `input` y los
/// antepone al turno. `None` si no hay skills, el motor no carga, o nada supera
/// el umbral (degradación elegante, igual que la memoria).
pub(crate) fn recall_skills(
    input: &str,
    book: &mut crate::agent_skill::SkillBook,
    emb: &mut Option<crate::memory::Embedder>,
) -> Option<String> {
    if book.is_empty() {
        return None; // sin skills → ni cargamos el modelo
    }
    let engine = ensure_embedder(emb).ok()?;
    // Vectoriza perezosamente los skills curados que aún no tienen embedding
    // (el motor ya está cargado en este punto).
    book.embed_pending(|t| engine.embed_one(t).ok());
    let query_vec = engine.embed_one(input).ok()?;
    let hits = book.search(&query_vec, 3, 0.55);
    if hits.is_empty() {
        return None;
    }
    // Transparencia: di QUÉ playbook(s) se aplican, por nombre (no genérico).
    let nombres: Vec<&str> = hits.iter().map(|sk| sk.name.as_str()).collect();
    println!("{}", ui::dim(&format!("⎿ playbook: {}", nombres.join(" · "))));
    let mut s = String::from(
        "[PLAYBOOK del proyecto que aplica a esta petición. SÍGUELO paso a paso: \
         te da los archivos y el orden exactos para ir de A→B, no explores a ciegas:\n",
    );
    for sk in hits {
        s.push_str(&format!("## {}\n{}\n", sk.name, sk.body));
    }
    s.push(']');
    Some(s)
}


/// Heurística: ¿esta petición "huele" a trabajo de investigación/exploración que
/// un subagente FLASH (barato) puede resolver y devolver ya digerido, ahorrando
/// tokens del cerebro caro? Devuelve el ROL adecuado, o `None` si no conviene
/// (peticiones de cambio, triviales o ambiguas → las hace el agente principal).
pub(crate) fn classify_delegation(input: &str) -> Option<&'static str> {
    let t = input.trim().to_lowercase();
    // Demasiado corto: no compensa el overhead del subagente.
    if t.split_whitespace().count() < 4 {
        return None;
    }
    // Señales de CAMBIO → NO delegar (eso lo hace el agente principal, que escribe).
    const CHANGE: [&str; 14] = [
        "crea", "créa", "agrega", "añade", "anade", "cambia", "arregla", "implementa",
        "escribe", "haz ", "borra", "elimina", "refactoriza", "renombra",
    ];
    if CHANGE.iter().any(|w| t.contains(w)) {
        return None;
    }
    // Señales de INVESTIGACIÓN/EXPLORACIÓN → delegar a un researcher flash.
    const RESEARCH: [&str; 19] = [
        "dónde", "donde", "cómo funciona", "como funciona", "qué hace", "que hace", "para qué",
        "busca", "encuentra", "localiza", "revisa", "explica", "entiende", "investiga", "analiza",
        "where", "how does", "find ", "locate",
    ];
    RESEARCH.iter().any(|w| t.contains(w)).then_some("researcher")
}

/// Auto-delegación: si la petición es de investigación, lanza un subagente FLASH
/// (barato, contexto aislado) que la resuelve y devuelve su conclusión, para
/// anteponerla al turno. Así el trabajo de lectura/búsqueda NO lo paga el cerebro
/// caro `pro`. Devuelve `None` si no se delega (cambios, triviales, ambiguas).
pub(crate) async fn maybe_auto_delegate(input: &str, cwd: &Path) -> Option<String> {
    let role = classify_delegation(input)?;
    println!("{}", ui::dim(&format!("⎿ delegando en subagente flash ({role}) para ahorrar…")));
    let task = format!(
        "El usuario preguntó: \"{input}\". Investiga en el proyecto y devuelve una conclusión \
         CONCISA con los archivos/funciones y datos concretos que respondan o ubiquen lo pedido. \
         No propongas cambios; solo informa lo que encuentres."
    );
    let conclusion = run_subagent(cwd, &task, Some(role)).await;
    if conclusion.trim().is_empty() || conclusion.contains("no pude lanzar el subagente") {
        return None;
    }
    Some(format!(
        "[investigación previa de un subagente flash sobre la petición (úsala como base y \
         verifica lo que necesites; ahorra que releas todo tú):\n{conclusion}\n]"
    ))
}

/// Rondas máximas de un subagente: es para investigar, no para épicas. Acotado
/// para que no se desboque (su contexto y coste corren por cuenta de la sesión).
const SUBAGENT_MAX_ROUNDS: usize = 6;

/// Lanza un SUBAGENTE de investigación AISLADO: su propio historial y contexto,
/// con las herramientas de SOLO LECTURA (read_file / search_project / web_search).
/// Hace la tarea acotada y devuelve SOLO su conclusión al agente principal — el
/// grueso de archivos que leyó NO contamina el contexto del padre (menos tokens,
/// más foco). No puede escribir, ejecutar ni commitear (sin efectos secundarios),
/// así que no necesita confirmaciones ni checkpoint. El consumo de tokens del
/// subagente se contabiliza en el mismo ledger de la sesión (`/cost` lo refleja).
pub(crate) async fn run_subagent(cwd: &Path, task: &str, role: Option<&str>) -> String {
    let role = crate::agent::roles::AgentRole::parse(role);
    let Some(brain) = subagent_brain() else {
        return "[no pude lanzar el subagente: ningún cerebro tiene API key]".to_string();
    };
    let preamble = subagent_preamble(cwd, task, role);
    // Subagente barato: investigar no necesita el cerebro caro → DeepSeek flash
    // sin thinking (12× más barato que el pro).
    let mentor = match ModelRouter::new(brain).subagent_mentor(&preamble) {
        Ok(m) => m,
        Err(e) => return format!("[no pude lanzar el subagente: {e}]"),
    };

    println!(
        "\n{} {} {}",
        ui::accent("⏺ subagente"),
        role.label(),
        ui::dim(&truncate_log(task, 80))
    );

    let mut history: Vec<Message> = Vec::new();
    let mut to_send = task.to_string();
    let mut conclusion = String::new();

    for round in 1..=SUBAGENT_MAX_ROUNDS {
        let spinner = ui::Spinner::start("subagente investigando…");
        let mut sink = |_: &str| {};
        let reply = mentor.chat_stream(&to_send, &mut history, &mut sink).await;
        spinner.stop();

        let ChatReply { text, calls, usage } = match reply {
            Ok(r) => r,
            Err(e) => {
                let note = format!("[el subagente falló a mitad: {}]", ui::friendly_error(&e.to_string()));
                if conclusion.trim().is_empty() {
                    return note;
                }
                conclusion.push_str(&format!("\n{note}"));
                break;
            }
        };
        crate::token::record(&usage);
        if !text.trim().is_empty() {
            conclusion = text.clone(); // la última narración es su conclusión
        }
        // Sin más herramientas: el subagente terminó y dio su respuesta.
        if calls.is_empty() {
            break;
        }
        // Atender SOLO lectura; cualquier intento de mutar se rechaza con un
        // tool result que le recuerda su naturaleza (sin romper el protocolo).
        for call in &calls {
            let out = subagent_tool(cwd, call).await;
            history.push(Message::tool_result(call.id.clone(), out));
        }
        // Empujarlo a cerrar conforme se acerca al tope de rondas.
        to_send = if round + 1 >= SUBAGENT_MAX_ROUNDS {
            "Te quedan pocas rondas: con lo que ya sabes, da AHORA tu conclusión en texto, \
             sin pedir más herramientas.".to_string()
        } else {
            "Continúa según los resultados; cuando tengas la respuesta, dala en texto plano \
             (será lo único que reciba el agente principal), sin pedir más herramientas."
                .to_string()
        };
    }

    println!("{}", ui::dim("⎿ subagente terminó"));

    if conclusion.trim().is_empty() {
        "[el subagente no produjo una conclusión]".to_string()
    } else {
        conclusion
    }
}

/// System prompt del subagente: identidad de investigador aislado de solo
/// lectura + árbol del proyecto + la tarea concreta.
pub(crate) fn subagent_preamble(cwd: &Path, task: &str, role: crate::agent::roles::AgentRole) -> String {
    let mut p = String::new();
    p.push_str(role.identity());
    p.push_str(
        "\n\nFuiste lanzado por el agente principal de dpx para una tarea ACOTADA. Trabajas en \
         AISLAMIENTO: tu contexto NO se comparte con el agente principal, así que tu respuesta \
         final debe ser AUTOSUFICIENTE y CONCISA.\n\n\
         REGLAS:\n\
         - Eres de SOLO LECTURA: solo puedes usar read_file, search_project y web_search. NO \
           puedes escribir, editar, borrar, ejecutar comandos, commitear ni lanzar otros \
           subagentes.\n\
         - Cíñete a tu tarea y a tu rol; no te desvíes.\n\
         - Cuando tengas la respuesta, dala en TEXTO PLANO sin pedir más herramientas: será lo \
           ÚNICO que reciba el agente principal.\n\
         - Sé directo: hechos, rutas de archivo y fragmentos relevantes con su ubicación; nada \
           de relleno ni cortesías.\n\n\
         # Árbol del proyecto\n```\n",
    );
    p.push_str(&crate::fs::project_tree(cwd));
    p.push_str("```\n\n# Tu tarea\n");
    p.push_str(task);
    p
}

/// Ejecuta una tool call DE UN SUBAGENTE: solo lectura. Cualquier otra cosa se
/// rechaza con un mensaje (el subagente no tiene efectos secundarios).
pub(crate) async fn subagent_tool(cwd: &Path, call: &rig_core::message::ToolCall) -> String {
    match tools::parse_call(&call.function.name, &call.function.arguments) {
        Ok(DpxCall::Read { path, offset, limit }) => {
            println!("  {}", ui::dim(&format!("↳ subagente lee {path}")));
            match crate::fs::read_file_range(cwd, &path, offset, limit) {
                Ok(c) => c,
                Err(e) => format!("[no pude leer `{path}`: {e}]"),
            }
        }
        Ok(DpxCall::Search { pattern }) => {
            println!("  {}", ui::dim(&format!("↳ subagente busca {pattern}")));
            crate::fs::search_in_project(cwd, &pattern)
        }
        Ok(DpxCall::WebSearch { query }) => {
            println!("  {}", ui::dim(&format!("↳ subagente busca en la web: {query}")));
            match crate::agent::search::web_search(&query).await {
                Ok(r) => r,
                Err(e) => format!("[web_search falló: {e}]"),
            }
        }
        _ => "[subagente de SOLO LECTURA: solo puedes usar read_file, search_project y \
              web_search. No escribas, ejecutes, commitees ni lances subagentes aquí; \
              limítate a investigar y devolver tu conclusión en texto.]"
            .to_string(),
    }
}
