//! Recuperacion de contexto, auto-delegacion y subagentes.

use std::path::Path;

use rig_core::completion::Message;

use super::{has_deepseek_key, truncate_log};
use crate::agent::tools::{self, DpxCall};
use crate::agent::{ChatReply, ModelRouter};
use crate::ui;

/// Heuristica: determina si conviene delegar una peticion a un subagente.
pub(crate) fn classify_delegation(input: &str) -> Option<&'static str> {
    let t = input.trim().to_lowercase();
    if t.split_whitespace().count() < 4 {
        return None;
    }
    const CHANGE: [&str; 20] = [
        "crea", "créa", "agrega", "añade", "anade", "cambia", "arregla", "implementa",
        "escribe", "haz ", "borra", "elimina", "refactoriza", "renombra",
        "mueve", "extrae", "genera", "construye", "compila", "deploy",
    ];
    if CHANGE.iter().any(|w| t.contains(w)) {
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
    let role = classify_delegation(input)?;
    println!("{}", ui::dim(&format!("⎿ delegando en subagente flash ({role}) para ahorrar…")));
    let task = format!(
        "El usuario preguntó: \"{input}\". Investiga en el proyecto y devuelve una conclusión \
         CONCISA con los archivos/funciones y datos concretos que respondan o ubiquen lo pedido. \
         No propongas cambios; solo informa lo que encuentres."
    );
    let conclusion = run_subagent(cwd, &task).await;
    if conclusion.trim().is_empty() || conclusion.contains("no pude lanzar el subagente") {
        return None;
    }
    Some(format!(
        "[investigación previa de un subagente flash sobre la petición (úsala como base y \
         verifica lo que necesites; ahorra que releas todo tú):\n{conclusion}\n]"
    ))
}

const SUBAGENT_MAX_ROUNDS: usize = 6;

pub(crate) async fn run_subagent(cwd: &Path, task: &str) -> String {
    if !has_deepseek_key() {
        return "[no pude lanzar el subagente: no hay DEEPSEEK_API_KEY]".to_string();
    };
    let preamble = subagent_preamble(cwd, task);
    let mentor = match ModelRouter::new().subagent_mentor(&preamble) {
        Ok(m) => m,
        Err(e) => return format!("[no pude lanzar el subagente: {e}]"),
    };

    println!(
        "\n{} {} {}",
        ui::accent("⏺ subagente"),
        ui::dim("investigando"),
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

    println!("{}", ui::dim("⎿ subagente terminó"));

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
              web_search. No escribas, ejecutes, commitees ni lances subagentes aqui; \
              limitate a investigar y devolver tu conclusion en texto.]"
            .to_string(),
    }
}
