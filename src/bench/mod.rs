//! Banco de pruebas A/B del harness (`dpx bench`).
//!
//! Responde con datos —no con intuición— la pregunta que abrió el update 0731
//! de DeepSeek: ¿conviene que el cerebro sea `pro` sin thinking, o `flash` con
//! `reasoning_effort` alto? Corre los MISMOS casos con configuraciones
//! distintas, midiendo aciertos, latencia, tokens y costo real de cada una.
//!
//! Es una herramienta de desarrollo, no parte del producto: corre en SOLO
//! LECTURA (nunca escribe ni ejecuta comandos en tu repo) y deja los
//! transcripts en `eval/`, que está en el `.gitignore`.

mod cases;
mod report;
mod sandbox;

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use rig_core::completion::Message;
use rig_core::message::ToolCall;

use crate::agent::tools::{self, DpxCall};
use crate::agent::{ChatReply, Effort, ModelRouter};
use crate::token::Tier;

pub use cases::CASES;

/// Tope de rondas por caso. Generoso a propósito: si un arm necesita más
/// rondas que otro para llegar al mismo sitio, eso ES el resultado y hay que
/// poder verlo, no recortarlo.
const MAX_ROUNDS: usize = 8;

/// Una configuración a comparar: qué modelo y con cuánto razonamiento.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arm {
    pub label: &'static str,
    pub tier: Tier,
    pub effort: Effort,
}

/// El 2x2 completo: tier × esfuerzo. Separar los dos ejes es el punto — si
/// `flash-high` le gana a `pro-nothink`, hay que saber si el mérito fue del
/// modelo o del thinking, y eso solo lo dicen las cuatro casillas.
pub const ARMS: [Arm; 4] = [
    Arm { label: "pro-nothink", tier: Tier::Pro, effort: Effort::Off },
    Arm { label: "flash-high", tier: Tier::Flash, effort: Effort::High },
    Arm { label: "flash-nothink", tier: Tier::Flash, effort: Effort::Off },
    Arm { label: "pro-high", tier: Tier::Pro, effort: Effort::High },
];

/// Los dos arms que responden la pregunta inmediata: el default de hoy contra
/// el candidato. `--arms all` corre el 2x2 entero.
const DEFAULT_ARMS: &str = "pro-nothink,flash-high";

/// El resultado de correr UN caso con UN arm.
#[derive(Debug, Clone)]
pub struct Run {
    pub case: &'static str,
    pub arm: &'static str,
    /// `None` si el caso no tiene criterio automático (lo juzgas tú).
    pub ok: Option<bool>,
    pub missing: Vec<String>,
    pub ms: u128,
    pub rounds: usize,
    pub tool_calls: usize,
    pub input: u64,
    pub cached: u64,
    pub output: u64,
    pub reasoning: u64,
    pub cost: f64,
    pub answer: String,
    pub error: Option<String>,
    /// Sandbox donde quedó el trabajo, en los casos de escritura. Se conserva
    /// tras la corrida: leer el código que produjo cada arm vale más que
    /// cualquier tabla.
    pub sandbox: Option<PathBuf>,
}

/// Punto de entrada de `dpx bench`.
pub async fn run(
    cwd: &Path,
    arms_spec: &str,
    case_filter: Option<&str>,
    repeat: usize,
    probe_only: bool,
) -> Result<()> {
    if !crate::agent::has_key() {
        return Err(anyhow!(
            "el banco necesita DEEPSEEK_API_KEY (ponla en .env o en ~/.dpx/.env)"
        ));
    }
    let arms = parse_arms(if arms_spec.trim().is_empty() { DEFAULT_ARMS } else { arms_spec })?;

    // La sonda va SIEMPRE primero: si la API no acepta un `reasoning_effort`,
    // los números de ese arm no significan nada y más vale saberlo antes de
    // gastar en la suite entera.
    let probe = probe_efforts(&arms).await;
    report::print_probe(&probe);
    if probe_only {
        return Ok(());
    }

    let selected: Vec<&cases::Case> = CASES
        .iter()
        .filter(|c| case_filter.is_none_or(|f| c.name.contains(f)))
        .collect();
    if selected.is_empty() {
        return Err(anyhow!("ningún caso coincide con `{}`", case_filter.unwrap_or("")));
    }
    let repeat = repeat.max(1);

    let out_dir = eval_dir(cwd)?;
    report::print_header(&arms, selected.len(), repeat, &out_dir);

    // El árbol del repo se calcula UNA vez: es idéntico para todos los casos de
    // lectura. Los de escritura necesitan el de su propio sandbox.
    let preamble_lectura = cases::preamble(cwd, false);

    let mut runs: Vec<Run> = Vec::new();
    for case in &selected {
        for arm in &arms {
            for rep in 1..=repeat {
                report::print_running(case.name, arm.label, rep, repeat);
                let run = if case.is_write() {
                    match prepare_sandbox(&out_dir, case, arm.label, rep) {
                        Ok(root) => {
                            let preamble = cases::preamble(&root, true);
                            run_case(&root, arm, case, &preamble, true).await
                        }
                        Err(e) => failed_run(case.name, arm.label, e.to_string()),
                    }
                } else {
                    run_case(cwd, arm, case, &preamble_lectura, false).await
                };
                report::print_run(&run);
                report::write_transcript(&out_dir, &run, rep)?;
                runs.push(run);
            }
        }
    }

    report::print_table(&runs, &arms);
    let summary = report::summary_markdown(&runs, &arms, &probe);
    let path = out_dir.join("summary.md");
    std::fs::write(&path, &summary)
        .with_context(|| format!("no pude escribir {}", path.display()))?;
    report::print_footer(&out_dir);
    Ok(())
}

/// Traduce `--arms`: lista de etiquetas separadas por coma, o `all`.
fn parse_arms(spec: &str) -> Result<Vec<Arm>> {
    if spec.trim().eq_ignore_ascii_case("all") {
        return Ok(ARMS.to_vec());
    }
    spec.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|label| {
            ARMS.iter().find(|a| a.label == label).copied().ok_or_else(|| {
                anyhow!(
                    "arm desconocido: `{label}`. Disponibles: {} (o `all`)",
                    ARMS.iter().map(|a| a.label).collect::<Vec<_>>().join(", ")
                )
            })
        })
        .collect()
}

/// Resultado de sondear un `reasoning_effort` contra la API real.
#[derive(Debug, Clone)]
pub struct Probe {
    /// Etiqueta sintética `<tier>-<esfuerzo>`, p. ej. `pro-max`.
    pub arm: String,
    pub model: String,
    pub effort: Effort,
    /// `Err` = la API rechazó la configuración.
    pub accepted: Result<(), String>,
    /// Tokens de razonamiento reportados en el `usage`. Cuidado: que venga en
    /// `0` NO prueba que no razonó — el cliente puede no estar mapeando el
    /// campo. Por eso la sonda mira también [`Probe::output_tokens`].
    pub reasoning_tokens: u64,
    /// Tokens de salida totales. Señal indirecta pero independiente del
    /// mapeo: con thinking encendido, el mismo problema cuesta más salida.
    pub output_tokens: u64,
}

/// Prompt de la sonda: tiene que ser un problema que EXIJA razonar y cuya
/// respuesta quepa en una línea. Si fuera trivial ("17 por 23"), un modelo
/// pensante contestaría sin pensar y la sonda no distinguiría nada.
const PROBE_PROMPT: &str = "¿Cuántos números de tres cifras son múltiplos de 7 y terminan en 3? \
                            Responde solo con el número.";

/// La escalera completa de esfuerzo. Se sondea ENTERA, no solo los esfuerzos
/// de los arms elegidos: `max` es lo que dpx manda en modo `learn`, así que hay
/// que verificarlo aunque no estés comparando ese arm. `Off` primero, porque es
/// el baseline contra el que se miden los demás.
const LADDER: [Effort; 3] = [Effort::Off, Effort::High, Effort::Max];

/// Manda el mismo problema con cada configuración para ver qué acepta la API de
/// verdad. Zanja si `reasoning_effort: "max"` existe o se ignora en silencio.
async fn probe_efforts(arms: &[Arm]) -> Vec<Probe> {
    let router = ModelRouter::new();
    let mut tiers: Vec<Tier> = Vec::new();
    for arm in arms {
        if !tiers.contains(&arm.tier) {
            tiers.push(arm.tier);
        }
    }

    let mut out = Vec::new();
    for tier in tiers {
        for effort in LADDER {
            let model = crate::agent::model_id(tier);
            let label = format!("{}-{}", tier.label(), effort.label());
            let fallo = |e: String| Probe {
                arm: label.clone(),
                model: model.clone(),
                effort,
                accepted: Err(e),
                reasoning_tokens: 0,
                output_tokens: 0,
            };
            let probe = match router.tuned_mentor(tier, effort, "Responde muy corto.", 0.0, Vec::new())
            {
                Err(e) => fallo(e.to_string()),
                Ok(mentor) => {
                    let mut history: Vec<Message> = Vec::new();
                    match mentor.chat_stream(PROBE_PROMPT, &mut history, &mut |_| {}).await {
                        Ok(reply) => Probe {
                            arm: label.clone(),
                            model: model.clone(),
                            effort,
                            accepted: Ok(()),
                            reasoning_tokens: reply.usage.map(|u| u.reasoning_tokens).unwrap_or(0),
                            output_tokens: reply.usage.map(|u| u.output_tokens).unwrap_or(0),
                        },
                        Err(e) => fallo(e.to_string()),
                    }
                }
            };
            out.push(probe);
        }
    }
    out
}

/// Herramientas de los casos de escritura. Ni `run_command` ni `delete_file` ni
/// git: el banco mide editar código, no ejecutar shell, y cuanto menos superficie
/// tenga el sandbox, menos formas hay de que una corrida se salga del carril.
const WRITE_TOOLS: [&str; 4] = ["read_file", "search_project", "write_file", "edit_file"];

/// Un `Run` que ni siquiera pudo arrancar (falló el sandbox o el cliente).
fn failed_run(case: &'static str, arm: &'static str, error: String) -> Run {
    Run {
        case,
        arm,
        ok: Some(false),
        missing: Vec::new(),
        ms: 0,
        rounds: 0,
        tool_calls: 0,
        input: 0,
        cached: 0,
        output: 0,
        reasoning: 0,
        cost: 0.0,
        answer: String::new(),
        error: Some(error),
        sandbox: None,
    }
}

/// Crea y siembra el sandbox de una corrida de escritura. Vive dentro de la
/// carpeta de la corrida, junto a los transcripts, para poder comparar después
/// el código que produjo cada arm.
fn prepare_sandbox(
    out_dir: &Path,
    case: &cases::Case,
    arm: &str,
    rep: usize,
) -> Result<PathBuf> {
    let root = out_dir.join("sandbox").join(format!("{}__{arm}__{rep}", case.name));
    std::fs::create_dir_all(&root)
        .with_context(|| format!("no pude crear el sandbox {}", root.display()))?;
    case.seed(&root)?;
    Ok(root)
}

/// Corre un caso con un arm: loop de tool calling hasta que el modelo contesta
/// en texto sin pedir más herramientas, o hasta agotar [`MAX_ROUNDS`].
///
/// `root` es el repo (casos de lectura) o el sandbox de esta corrida (casos de
/// escritura). Todas las herramientas se resuelven contra esa raíz.
async fn run_case(
    root: &Path,
    arm: &Arm,
    case: &'static cases::Case,
    preamble: &str,
    writes: bool,
) -> Run {
    let mut run = Run {
        case: case.name,
        arm: arm.label,
        ok: None,
        missing: Vec::new(),
        ms: 0,
        rounds: 0,
        tool_calls: 0,
        input: 0,
        cached: 0,
        output: 0,
        reasoning: 0,
        cost: 0.0,
        answer: String::new(),
        error: None,
        sandbox: writes.then(|| root.to_path_buf()),
    };

    let toolset = if writes {
        tools::definitions_named(&WRITE_TOOLS)
    } else {
        tools::definitions_read_only()
    };

    // Temperatura fija en todos los arms: si varía, la comparación deja de ser
    // manzana con manzana.
    let mentor =
        match ModelRouter::new().tuned_mentor(arm.tier, arm.effort, preamble, 0.2, toolset) {
            Ok(m) => m,
            Err(e) => {
                run.error = Some(e.to_string());
                return run;
            }
        };

    let started = Instant::now();
    let mut history: Vec<Message> = Vec::new();
    let mut to_send = case.prompt.to_string();

    for round in 1..=MAX_ROUNDS {
        run.rounds = round;
        let reply = mentor.chat_stream(&to_send, &mut history, &mut |_| {}).await;
        let ChatReply { text, calls, usage } = match reply {
            Ok(r) => r,
            Err(e) => {
                run.error = Some(e.to_string());
                break;
            }
        };
        if let Some(u) = usage {
            run.input += u.input_tokens;
            run.cached += u.cached_input_tokens;
            run.output += u.output_tokens;
            run.reasoning += u.reasoning_tokens;
        }
        if !text.trim().is_empty() {
            run.answer = text;
        }
        if calls.is_empty() {
            break;
        }
        run.tool_calls += calls.len();

        // Las lecturas son independientes: van EN PARALELO. Las escrituras NO:
        // dos ediciones al mismo archivo en la misma ronda se pisarían, y el
        // resultado dependería de quién termine antes. En cuanto hay escrituras
        // el orden lo decide el modelo, y se respeta.
        let results: Vec<String> = if writes {
            let mut out = Vec::with_capacity(calls.len());
            for call in &calls {
                out.push(exec_tool(root, call, true).await);
            }
            out
        } else {
            futures::future::join_all(calls.iter().map(|call| exec_tool(root, call, false))).await
        };
        // El protocolo exige un tool_result por cada tool_call, emparejado por id.
        for (call, out) in calls.iter().zip(results) {
            history.push(Message::tool_result(call.id.clone(), out));
        }

        to_send = if round + 1 >= MAX_ROUNDS {
            "Te queda una ronda: responde AHORA en texto con lo que ya sabes, sin pedir más \
             herramientas."
                .to_string()
        } else {
            "Continúa con los resultados; cuando tengas la respuesta, dala en texto plano sin \
             pedir más herramientas."
                .to_string()
        };
    }

    run.ms = started.elapsed().as_millis();
    run.cost = crate::token::estimate_cost(arm.tier, run.input, run.cached, run.output);
    if case.scored() {
        // En los casos de escritura esto mira el DISCO, no lo que el modelo
        // dijo haber hecho: "ya lo renombré" no vale si el archivo dice otra cosa.
        run.missing = case.failures(&run.answer, root);
        run.ok = Some(run.missing.is_empty() && run.error.is_none());
    }
    run
}

/// Ejecutor de herramientas del banco, confinado a `root`.
///
/// Sin `writes`, solo lectura: aunque el modelo se invente un `run_command`,
/// aquí no hay nada que lo atienda. Con `writes`, se añaden `write_file` y
/// `edit_file` — y `root` es siempre un sandbox desechable, nunca tu repo.
/// `safe_target` remata la contención: ni rutas absolutas ni `..`.
async fn exec_tool(root: &Path, call: &ToolCall, writes: bool) -> String {
    let parsed = match tools::parse_call(&call.function.name, &call.function.arguments) {
        Ok(c) => c,
        Err(e) => return format!("[llamada inválida: {e}]"),
    };
    match parsed {
        DpxCall::Read { path, offset, limit } => {
            match crate::fs::read_file_range(root, &path, offset, limit) {
                Ok(c) => c,
                Err(e) => format!("[no pude leer `{path}`: {e}]"),
            }
        }
        DpxCall::Search { pattern } => crate::fs::search_in_project(root, &pattern),
        DpxCall::WebSearch { query } => match crate::agent::search::web_search(&query).await {
            Ok(r) => r,
            Err(e) => format!("[web_search falló: {e}]"),
        },
        DpxCall::Write { path, content } if writes => {
            let write = crate::fs::FileWrite { path: path.clone(), content };
            match crate::fs::apply(root, &write) {
                Ok(_) => format!("[escrito `{path}`]"),
                Err(e) => format!("[no pude escribir `{path}`: {e}]"),
            }
        }
        DpxCall::Edit { path, search, replace } if writes => {
            let Some(actual) = crate::fs::current_content(root, &path) else {
                return format!("[no pude leer `{path}` para editarlo]");
            };
            let edit = crate::fs::FileEdit { path: path.clone(), search, replace };
            match crate::fs::apply_edit(&actual, &edit) {
                Ok(nuevo) => {
                    let write = crate::fs::FileWrite { path: path.clone(), content: nuevo };
                    match crate::fs::apply(root, &write) {
                        Ok(_) => format!("[editado `{path}`]"),
                        Err(e) => format!("[no pude guardar `{path}`: {e}]"),
                    }
                }
                // El mensaje importa: es lo que le permite al modelo corregir
                // el SEARCH en la siguiente ronda en vez de quedarse atascado.
                Err(e) => format!("[la edición de `{path}` no aplicó: {e}]"),
            }
        }
        _ if writes => "[en el banco solo hay read_file, search_project, write_file y edit_file]"
            .to_string(),
        _ => "[el banco de pruebas es de solo lectura: usa read_file, search_project o \
              web_search]"
            .to_string(),
    }
}

/// Carpeta de salida: `eval/bench-<fecha>`. `eval/` ya está en el `.gitignore`.
fn eval_dir(cwd: &Path) -> Result<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let dir = cwd.join("eval").join(format!("bench-{stamp}"));
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("no pude crear {}", dir.display()))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_arms_resuelve_etiquetas_y_all() {
        let dos = parse_arms(DEFAULT_ARMS).unwrap();
        assert_eq!(dos.len(), 2);
        assert_eq!(dos[0].tier, Tier::Pro);
        assert_eq!(dos[0].effort, Effort::Off);
        assert_eq!(dos[1].tier, Tier::Flash);
        assert_eq!(dos[1].effort, Effort::High);

        assert_eq!(parse_arms("all").unwrap().len(), ARMS.len());
        assert_eq!(parse_arms("ALL").unwrap().len(), ARMS.len());
        // Tolera espacios y comas sueltas.
        assert_eq!(parse_arms(" flash-high , ").unwrap().len(), 1);
    }

    #[test]
    fn parse_arms_rechaza_lo_desconocido_con_las_opciones() {
        let e = parse_arms("pro-turbo").unwrap_err().to_string();
        assert!(e.contains("pro-turbo"), "{e}");
        assert!(e.contains("flash-high"), "el error debe listar los arms válidos: {e}");
    }

    #[test]
    fn el_2x2_cubre_las_cuatro_casillas() {
        for tier in [Tier::Pro, Tier::Flash] {
            for effort in [Effort::Off, Effort::High] {
                assert!(
                    ARMS.iter().any(|a| a.tier == tier && a.effort == effort),
                    "falta la casilla {tier:?}/{effort:?}"
                );
            }
        }
        // Etiquetas únicas: son la clave con la que se seleccionan.
        for a in ARMS {
            assert_eq!(ARMS.iter().filter(|o| o.label == a.label).count(), 1);
        }
    }
}
