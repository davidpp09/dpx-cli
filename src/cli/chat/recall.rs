//! Recuperacion de contexto, auto-delegacion y subagentes.

use anyhow::Result;
use std::path::Path;

use rig_core::completion::Message;

use super::{has_deepseek_key, truncate_log};
use crate::agent::tools::{self, DpxCall};
use crate::agent::{ChatReply, ModelRouter};
use crate::ui;

/// Heuristica: determina si conviene delegar una peticion a un subagente.
/// Prueba primero el modelo flash; si falla, cae al keyword match.
pub(crate) async fn classify_delegation(input: &str) -> Option<&'static str> {
    match classify_delegation_flash(input).await {
        Ok(Some(role)) => Some(role),
        Ok(None) => None,
        Err(_) => classify_delegation_fallback(input),
    }
}

async fn classify_delegation_flash(input: &str) -> Result<Option<&'static str>> {
    const SYSTEM: &str = "Eres un clasificador. Responde UNA sola palabra.";
    let user = format!(
        "Clasifica esta petición en UNA palabra:\n\
         - research = solo investigar/leer/buscar/explicar (no toca código);\n\
         - modify = cambiar/arreglar/refactorizar/mover/borrar código que YA existe;\n\
         - new = crear algo nuevo desde cero.\n\
         Responde solo: research, modify o new.\n\nPetición: {input}"
    );
    let reply = ModelRouter::new().flash_prompt(SYSTEM, &user).await?;
    match reply.trim().to_lowercase().as_str() {
        "research" => Ok(Some("researcher")),
        "modify" => Ok(Some("mapper")),
        "new" => Ok(None),
        _ => Ok(classify_delegation_fallback(input)),
    }
}

pub(crate) fn classify_delegation_fallback(input: &str) -> Option<&'static str> {
    let t = input.trim().to_lowercase();
    if t.split_whitespace().count() < 4 {
        return None;
    }
    // Cambiar código que YA existe → mapear el terreno en flash antes de editar.
    const MODIFY: &[&str] = &[
        "cambia", "arregla", "corrige", "modifica", "refactoriza", "renombra",
        "mueve", "extrae", "borra", "elimina", "ajusta", "reemplaza", "migra",
    ];
    if MODIFY.iter().any(|w| t.contains(w)) {
        return Some("mapper");
    }
    // Crear algo NUEVO desde cero → no hay terreno que investigar (pro lo escribe).
    const CREATE: &[&str] = &[
        "crea", "créa", "agrega", "añade", "anade", "implementa", "escribe",
        "genera", "construye", "compila", "deploy", "haz ",
    ];
    if CREATE.iter().any(|w| t.contains(w)) {
        return None;
    }
    const RESEARCH: [&str; 34] = [
        "dónde", "donde", "cómo funciona", "como funciona", "qué hace", "que hace", "para qué",
        "busca", "encuentra", "localiza", "revisa", "explica", "entiende", "investiga", "analiza",
        "where", "how does", "find ", "locate", "explain", "what does",
        "por qué", "por que", "cual es", "cuál es", "cuantos", "cuántos",
        "dime que", "dime qué", "que hay en", "qué hay en", "lista", "dependencias", "versión",
    ];
    RESEARCH.iter().any(|w| t.contains(w)).then_some("researcher")
}

pub(crate) async fn maybe_auto_delegate(input: &str, cwd: &Path) -> Option<String> {
    let role = classify_delegation(input).await?;
    // `mapper` = tarea de CAMBIO sobre código existente → flash mapea el terreno
    // antes de que pro edite (descarga la lectura del cerebro pro). `researcher`
    // = pregunta → flash investiga y responde. Ambos van al cerebro BARATO.
    let (announce, task, label): (&str, String, &str) = if role == "mapper" {
        (
            "mapeando el código del cambio en flash…",
            format!(
                "El usuario va a pedir este CAMBIO: \"{input}\". NO edites nada. Mapea el terreno \
                 para que el agente principal arranque ENFOCADO: qué archivos y funciones están \
                 implicados, su estructura actual, DÓNDE se haría el cambio y qué se vería afectado \
                 (usos, llamadas, imports). Devuelve un mapa CONCISO con rutas y líneas."
            ),
            "[mapa del código relevante para el cambio (subagente flash); arranca desde aquí, \
             pero verifica antes de editar lo que toques)",
        )
    } else {
        (
            "delegando investigación en flash…",
            format!(
                "El usuario preguntó: \"{input}\". Investiga en el proyecto y devuelve una \
                 conclusión CONCISA con los archivos/funciones y datos concretos que respondan o \
                 ubiquen lo pedido. No propongas cambios; solo informa lo que encuentres."
            ),
            "[investigación previa de un subagente flash sobre la petición (úsala como base y \
             verifica lo que necesites; ahorra que releas todo tú)",
        )
    };
    println!("{}", ui::dim(&format!("⎿ {announce}")));
    let conclusion = run_subagent(cwd, &task).await;
    if conclusion.trim().is_empty() || conclusion.contains("no pude lanzar el subagente") {
        return None;
    }
    Some(format!("{label}:\n{conclusion}\n]"))
}

const SUBAGENT_MAX_ROUNDS: usize = 6;

pub(crate) async fn run_subagent(cwd: &Path, task: &str) -> String {
    run_subagent_inner(cwd, task, true).await
}

/// Igual que [`run_subagent`] pero SILENCIOSO: sin header, sin spinner por ronda
/// ni línea de cierre. Para correr varios subagentes EN PARALELO (p. ej. el
/// comité) sin que sus spinners se pisen en la misma línea — el llamador enseña
/// su propio progreso. Las trazas de lectura (`↳ subagente lee…`) sí salen: son
/// líneas sueltas (sin `\r`), así que conviven sin garabatear.
pub(crate) async fn run_subagent_quiet(cwd: &Path, task: &str) -> String {
    run_subagent_inner(cwd, task, false).await
}

async fn run_subagent_inner(cwd: &Path, task: &str, verbose: bool) -> String {
    // En modo silencioso (learn) NUNCA mostramos la actividad del subagente.
    let verbose = verbose && !crate::ui::tools_quiet();
    if !has_deepseek_key() {
        return "[no pude lanzar el subagente: no hay DEEPSEEK_API_KEY]".to_string();
    };
    let preamble = subagent_preamble(cwd, task);
    let mentor = match ModelRouter::new().subagent_mentor(&preamble) {
        Ok(m) => m,
        Err(e) => return format!("[no pude lanzar el subagente: {e}]"),
    };

    if verbose {
        println!(
            "\n{} {} {}",
            ui::accent("⏺ subagente"),
            ui::dim("investigando"),
            ui::dim(&truncate_log(task, 80))
        );
    }

    let mut history: Vec<Message> = Vec::new();
    let mut to_send = task.to_string();
    let mut conclusion = String::new();

    for round in 1..=SUBAGENT_MAX_ROUNDS {
        let spinner = verbose.then(|| ui::Spinner::start("subagente investigando…"));
        let mut sink = |_: &str| {};
        let reply = mentor.chat_stream(&to_send, &mut history, &mut sink).await;
        if let Some(s) = spinner {
            s.stop();
        }

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
            conclusion = text.clone();
        }
        if calls.is_empty() {
            break;
        }
        for call in &calls {
            let out = subagent_tool(cwd, call).await;
            history.push(Message::tool_result(call.id.clone(), out));
        }
        to_send = if round + 1 >= SUBAGENT_MAX_ROUNDS {
            "Te quedan pocas rondas: con lo que ya sabes, da AHORA tu conclusión en texto, \
             sin pedir más herramientas.".to_string()
        } else {
            "Continúa según los resultados; cuando tengas la respuesta, dala en texto plano \
             (será lo único que reciba el agente principal), sin pedir más herramientas."
                .to_string()
        };
    }

    if verbose {
        println!("{}", ui::dim("⎿ subagente terminó"));
    }

    if conclusion.trim().is_empty() {
        "[el subagente no produjo una conclusión]".to_string()
    } else {
        conclusion
    }
}

pub(crate) fn subagent_preamble(cwd: &Path, task: &str) -> String {
    let mut p = String::new();
    p.push_str("Eres un subagente de investigacion de dpx. Eres de SOLO LECTURA.");
    p.push_str(
        "\n\nFuiste lanzado por el agente principal de dpx para una tarea ACOTADA. Trabajas en \
         AISLAMIENTO: tu contexto NO se comparte con el agente principal, asi que tu respuesta \
         final debe ser AUTOSUFICIENTE y CONCISA.\n\n\
         REGLAS:\n\
         - Eres de SOLO LECTURA: solo puedes usar read_file, search_project y web_search. NO \
           puedes escribir, editar, borrar, ejecutar comandos, commitear ni lanzar otros \
           subagentes.\n\
         - Cínete a tu tarea y a tu rol; no te desvies.\n\
         - Cuando tengas la respuesta, dala en TEXTO PLANO sin pedir mas herramientas: sera lo \
           UNICO que reciba el agente principal.\n\
         - Se directo: hechos, rutas de archivo y fragmentos relevantes con su ubicacion; nada \
           de relleno ni cortesias.\n\n\
         # Arbol del proyecto\n```\n",
    );
    p.push_str(&crate::fs::project_tree(cwd));
    p.push_str("```\n\n# Tu tarea\n");
    p.push_str(task);
    p
}

pub(crate) async fn subagent_tool(cwd: &Path, call: &rig_core::message::ToolCall) -> String {
    match tools::parse_call(&call.function.name, &call.function.arguments) {
        Ok(DpxCall::Read { path, offset, limit }) => {
            if !ui::tools_quiet() {
                println!("  {}", ui::dim(&format!("↳ subagente lee {path}")));
            }
            match crate::fs::read_file_range(cwd, &path, offset, limit) {
                Ok(c) => c,
                Err(e) => format!("[no pude leer `{path}`: {e}]"),
            }
        }
        Ok(DpxCall::Search { pattern }) => {
            if !ui::tools_quiet() {
                println!("  {}", ui::dim(&format!("↳ subagente busca {pattern}")));
            }
            crate::fs::search_in_project(cwd, &pattern)
        }
        Ok(DpxCall::WebSearch { query }) => {
            if !ui::tools_quiet() {
                println!("  {}", ui::dim(&format!("↳ subagente busca en la web: {query}")));
            }
            match crate::agent::search::web_search(&query).await {
                Ok(r) => r,
                Err(e) => format!("[web_search falló: {e}]"),
            }
        }
        _ => "[subagente de SOLO LECTURA: solo puedes usar read_file, search_project y \
              web_search. No escribas, ejecutes, commitees ni lances subagentes aqui; \
              limitate a investigar y devolver tu conclusion en texto.]"
            .to_string(),
    }
}
