//! El loop conversacional (REPL) del mentor, con UI estilo Claude Code,
//! respuestas en streaming y comandos (`/help`, `/clear`, `/focus`, …).
//!
//! La entrada (multilínea, autocompletado, pegados, historial) vive en el
//! editor propio sobre crossterm: ver `super::editor`.

use std::env;
use std::path::Path;

use anyhow::Result;
use rig_core::completion::Message;
use rig_core::completion::message::{ToolResultContent, UserContent};

use super::editor::{InputEditor, ReadResult};
use crate::agent::tools::{self, DpxCall};
use crate::agent::{Brain, ChatReply, Mentor, ModelRouter};
use crate::cli::AutoMode;
use crate::focus::{self, Mode};
use crate::session::{self, ProjectStore, Turn};
use crate::ui;

mod helpers;
mod actions;
mod commands;
mod recall;
mod committee;
mod review;
pub(crate) use actions::*;
pub(crate) use commands::{build_quiz_prompt, handle_command, remember};
pub(crate) use recall::{maybe_auto_delegate, recall_context, recall_skills, run_subagent};
#[cfg(test)]
pub(crate) use recall::{classify_delegation, subagent_preamble, subagent_tool};
pub(crate) use committee::run_comite_command;
pub(crate) use helpers::{
    canonical_cmd, command_in_mode, mode_label, mode_label_compact, reject_command, short_path,
};

/// El cerebro con el que lanzar un subagente: DeepSeek si tiene API key (el
/// subagente investiga; necesita un cerebro vivo).
fn subagent_brain() -> Option<Brain> {
    Brain::all().into_iter().find(|b| b.has_key())
}

pub async fn run(
    focus: Option<String>,
    mode: Mode,
    brain: Brain,
    auto: AutoMode,
) -> Result<()> {
    let cwd = env::current_dir()?;
    // ¿Es la PRIMERA vez en este proyecto? (antes de crear `.dpx/`). De esto
    // dependen el onboarding de configuración y el brainstorm inicial de hack.
    let fresh_project = !cwd.join(".dpx").exists();
    let store = ProjectStore::init(&cwd)?;
    // Tiñe TODA la UI con el color del modo activo antes de pintar nada (logo
    // incluido): cada modo tiene su propia identidad visual.
    ui::set_mode_theme(mode);
    let skin = ui::skin();

    ui::install_ctrl_c_handler();
    ui::logo();

    let mut auto = auto;
    // Primer arranque: pantalla de configuración (como `init`); el modo ya lo
    // fijó el subcomando. Adopta el focus/auto elegidos. Sin `.dpx` no hay
    // memoria previa, así que `prior = None` y no se corre `startup_flow`.
    let (focus_id, prior) = if fresh_project {
        let cfg = crate::cli::init::onboarding(&cwd, mode, brain)?;
        auto = AutoMode::parse(&cfg.auto).unwrap_or(auto);
        (focus.or(cfg.focus), None)
    } else {
        // Con `--focus` explícito se respeta tal cual; sin él, el arranque
        // inteligente resuelve el enfoque y si se retoma el contexto previo.
        match focus {
            Some(f) => (Some(f), store.prior_context()),
            None => startup_flow(&cwd, &store),
        }
    };

    // Plan pendiente de la sesión anterior (`.dpx/plan.md`): se muestra como
    // checklist y se inyecta junto con la memoria para que el mentor lo
    // continúe. Si el usuario rechazó retomar la memoria, también se respeta
    // aquí (prior = None ⇒ el plan no se inyecta esta sesión).
    let prior = resume_plan(&store, prior);

    // Estado mutable de la sesión (los comandos pueden cambiarlo en caliente).
    let mut focus_id = focus_id;
    let mut mode = mode;
    let mut brain = brain;
    let mut router = ModelRouter::new(brain);
    let mut mentor = build_mentor(&router, focus_id.as_deref(), mode, prior.as_deref())?;

    ui::welcome(
        focus::display_name(focus_id.as_deref()),
        mode_label(mode),
        router.brain_label(),
        &short_path(&cwd),
    );
    if prior.is_some() {
        println!("  {}", ui::dim("memoria · retomando contexto de sesiones anteriores"));
    }
    // Cada modo arranca con su propio cartel: deja claro en cuál estás.
    ui::mode_banner(mode);
    // Modo hack: panel con el estado del proyecto, plan y progreso.
    if mode == Mode::Hack {
        let proyecto = store.prior_context().map(|ctx| {
            ctx.lines()
                .find(|l| !l.trim().is_empty() && !l.trim().starts_with('#'))
                .unwrap_or("proyecto nuevo")
                .trim()
                .to_string()
        });
        let proyecto_disp = proyecto.as_deref();
        let plan_data = store.read_plan().and_then(|p| crate::fs::parse_plan(&p));
        let en_que_voy = plan_data.as_ref().and_then(|plan| {
            plan.iter().find(|(done, _)| !done).map(|(_, text)| text.as_str())
        });
        let committee = store.read_committee();
        ui::hack_panel(
            proyecto_disp,
            en_que_voy.unwrap_or("sin enfoque definido aún"),
            committee.as_deref(),
            plan_data.as_deref(),
        );
    }
    println!(
        "\n{}",
        ui::dim("escribe tu mensaje · @archivo lee código · Shift+Enter salto de línea · Ctrl-C cancela · /salir")
    );

    // Hooks del proyecto (.dpx/hooks.toml): se disparan ante eventos del ciclo
    // de vida. OnSessionStart se ejecuta aquí, al inicio.
    let hooks = store.load_hooks();
    crate::cli::hooks::run_hooks(&hooks, &crate::cli::hooks::HookEvent::OnSessionStart, None, &cwd);

    let mut history: Vec<Message> = Vec::new();
    let mut turns: Vec<Turn> = Vec::new();

    // Etiqueta del proyecto para el título de la pestaña (carpeta actual).
    let proj_label = cwd
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "proyecto".to_string());
    ui::title_idle(&proj_label);

    // Editor de entrada propio (crossterm): multilínea, Tab, pegados, historial.
    let mut ed = InputEditor::new(cwd.clone());

    // Modo HACK en proyecto NUEVO: la primera idea que escribas pasa por el
    // comité (lluvia de ideas) antes de ponerse a construir. De ahí sale un
    // plan y se deciden las siguientes cosas.
    let mut pending_brainstorm = fresh_project && mode == Mode::Hack;
    if pending_brainstorm {
        println!(
            "\n{}",
            ui::accent("proyecto nuevo · cuéntame tu idea y la paso por el comité (lluvia de ideas)")
        );
        println!(
            "{}",
            ui::dim("  4 roles (juez · product · tech lead · escéptico) la evalúan y te devuelven un plan; de ahí decidimos el resto.")
        );
    }

    // Memoria semántica de largo plazo. El store se carga barato (lee el jsonl);
    // el motor de embeddings se carga PEREZOSAMENTE solo si de verdad se usa
    // (primer /recordar o primera recuperación con memoria existente), para no
    // penalizar el arranque de quien no usa la memoria.
    let mut mem_store = crate::memory::MemoryStore::load(store.dpx_dir());
    let mut embedder: Option<crate::memory::Embedder> = None;

    // Skills del proyecto: CURADOS (`skills/*.md`, locales) + EMPOTRADOS del stack
    // activo (vienen en dpx). Carga barata; el embedder solo si de verdad se usan.
    let mut skillbook = crate::agent_skill::SkillBook::from_dir(&cwd.join("skills"));
    skillbook.extend(
        crate::focus::builtin_playbooks(focus_id.as_deref())
            .iter()
            .map(|(n, w, b)| {
                crate::agent_skill::AgentSkill::builtin(n, w, b, focus_id.as_deref().unwrap_or(""))
            })
            .collect(),
    );
    // Playbooks GENERALES (arquitectura, CSS/UI, lógica de negocio): cross-stack,
    // se cargan siempre y se inyectan cuando la petición encaja con alguno.
    skillbook.extend(
        crate::focus::general_playbooks()
            .iter()
            .map(|(n, w, b)| crate::agent_skill::AgentSkill::builtin(n, w, b, "general"))
            .collect(),
    );

    loop {
        // De vuelta en el prompt: la pestaña muestra dpx en reposo.
        ui::title_idle(&proj_label);
        let bar = ui::format_input_status(
            focus::display_name(focus_id.as_deref()),
            mode_label_compact(mode),
            router.brain_label(),
            auto,
        );

        match ed.read_input(&bar) {
            Ok(ReadResult::Line(line)) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }

                // Comandos (slash) y salida directa: solo en entradas de UNA
                // línea (un pegado expandido puede empezar con '/').
                let single_line = !input.contains('\n');
                if single_line && matches!(input, "salir" | "exit" | "/salir" | "/exit" | ":q") {
                    break;
                }
                // /compact es async (llama al modelo barato): se atiende aquí,
                // no en handle_command (que es síncrono).
                if single_line && (input == "/compactar" || input == "/compact") {
                    compact_now(&router, &mut history, &turns).await;
                    continue;
                }

                // /comite (alias /brainstorm): lanza el comité de hack para
                // evaluar una idea desde 4 roles y devolver un plan. SOLO en modo hack.
                if single_line
                    && let Some(idea) = input
                        .strip_prefix("/comité ")
                        .or_else(|| input.strip_prefix("/comite "))
                        .or_else(|| input.strip_prefix("/brainstorm "))
                {
                    if command_in_mode("comite", mode) {
                        run_comite_command(&cwd, idea, &skin, &store, &mut turns).await;
                    } else {
                        reject_command("comite", mode);
                    }
                    continue;
                }

                // /recordar <texto>: guarda algo en la memoria de largo plazo. Se
                // traerá solo cuando sea relevante a una consulta futura.
                if single_line && let Some(text) = input.strip_prefix("/recordar ") {
                    let text = text.trim();
                    if text.is_empty() {
                        println!("{}", ui::dim("uso: /recordar <algo que quieras que dpx recuerde>"));
                    } else {
                        match remember(text, &mut mem_store, &mut embedder, "nota") {
                            Ok(()) => println!(
                                "{}",
                                ui::dim(&format!(
                                    "⎿ recordado · {} en memoria · lo traeré cuando sea relevante",
                                    mem_store.len()
                                ))
                            ),
                            Err(e) => println!("{} {}", ui::dim("no pude recordar:"), ui::dim(&e.to_string())),
                        }
                    }
                    continue;
                }

                // /skills: los playbooks del proyecto (curados + built-in del stack),
                // y un DOCTOR que audita el catálogo con el propio motor de
                // embeddings: gatillos vacíos, cuerpos triviales y colisiones de
                // gatillo (dos skills tan parecidos que dpx dispararía el equivocado).
                if single_line
                    && matches!(input, "/habilidades" | "/skills" | "/skill")
                {
                    ui::skills_list(&skillbook.ranked());
                    let mut warnings = skillbook.lint();
                    if let Ok(engine) = ensure_embedder(&mut embedder) {
                        skillbook.embed_pending(|t| engine.embed_one(t).ok());
                        for (a, b, score) in skillbook.overlaps(0.90) {
                            warnings.push(format!(
                                "«{a}» ↔ «{b}» colisionan ({:.0}% de parecido)",
                                score * 100.0
                            ));
                        }
                    }
                    ui::skills_doctor(&warnings);
                    continue;
                }

                // /quiz [tema]: el tutor te interroga (retrieval practice) sobre lo
                // que ya viste. Es un turno normal con un prompt sintético, así que
                // la conversación sigue (puedes responder y te da feedback).
                if single_line
                    && let Some(rest) = input
                        .strip_prefix("/examen")
                        .or_else(|| input.strip_prefix("/quiz"))
                    && (rest.is_empty() || rest.starts_with(' '))
                {
                    if !command_in_mode("quiz", mode) {
                        reject_command("quiz", mode);
                        continue;
                    }
                    let topic = rest.trim();
                    let prompt = build_quiz_prompt(&store, focus_id.as_deref(), topic);
                    store.checkpoint("user", input)?;
                    turns.push(Turn { role: "user", text: input.to_string() });
                    ui::clear_cancel();
                    let outcome = run_turn(
                        &mentor, &mut history, &cwd, &skin,
                        &mut |p| ed.confirm_line(p), &store, &prompt, auto,
                    ).await;
                    match outcome {
                        TurnOutcome::Reply(full) => {
                            store.checkpoint("assistant", &full)?;
                            turns.push(Turn { role: "assistant", text: full });
                        }
                        TurnOutcome::Empty => {}
                        TurnOutcome::ModelFailed(err) => {
                            ui::panel("⚠ error del modelo", &ui::friendly_error(&err));
                        }
                    }
                    continue;
                }

                if single_line && let Some(cmd) = input.strip_prefix('/') {
                    if matches!(cmd, "salir" | "exit" | "q") {
                        break;
                    }
                    // Comandos personalizados del proyecto (.dpx/commands.toml)
                    let ccmds = store.load_custom_commands();
                    if let Some(prompt) =
                        super::commands::dispatch_custom_command(&ccmds, cmd).await
                    {
                        store.checkpoint("user", input)?;
                        turns.push(Turn {
                            role: "user",
                            text: input.to_string(),
                        });
                        ui::clear_cancel();
                        let outcome = run_turn(
                            &mentor, &mut history, &cwd, &skin,
                            &mut |p| ed.confirm_line(p), &store,
                            &prompt, auto,
                        ).await;
                        match outcome {
                            TurnOutcome::Reply(full) => {
                                store.checkpoint("assistant", &full)?;
                                turns.push(Turn { role: "assistant", text: full });
                            }
                            TurnOutcome::Empty => {}
                            TurnOutcome::ModelFailed(err) => {
                                ui::panel("⚠ error del modelo", &ui::friendly_error(&err));
                            }
                        }
                        continue;
                    }
                    handle_command(
                        cmd, &skin, &store, &prior, &mut router, &mut mentor, &mut focus_id,
                        &mut mode, &mut brain, &mut auto, &mut history, &cwd,
                        turns.len(),
                    );
                    continue;
                }

                // Modo hack recién creado: la PRIMERA idea pasa por el comité
                // (lluvia de ideas) en vez de un turno normal. De ahí sale el plan.
                if pending_brainstorm {
                    pending_brainstorm = false;
                    run_comite_command(&cwd, input, &skin, &store, &mut turns).await;
                    continue;
                }

                store.checkpoint("user", input)?;
                turns.push(Turn { role: "user", text: input.to_string() });

                // Memoria de largo plazo: recupera fragmentos relevantes a la
                // consulta y los antepone al turno (si hay memoria y algo encaja).
                let mut turn_input = match recall_context(input, &mem_store, &mut embedder) {
                    Some(ctx) => {
                        println!("{}", ui::dim("⎿ recordando contexto relevante…"));
                        format!("{ctx}\n\n{input}")
                    }
                    None => input.to_string(),
                };
                // Skills curados (code/hack): trae los playbooks `skills/*.md`
                // que encajan con la petición y los antepone para que dpx los
                // siga paso a paso. En learn no aplica (no ejecuta).
                if mode != Mode::Learn
                    && let Some(sk) = recall_skills(input, &mut skillbook, &mut embedder)
                {
                    turn_input = format!("{sk}\n\n{turn_input}");
                }
                // Auto-delegación: si la petición es de investigación, un subagente
                // FLASH (barato, contexto aislado) la resuelve y antepone su
                // conclusión — el trabajo de lectura NO lo paga el cerebro caro.
                if mode != Mode::Learn
                    && let Some(research) = maybe_auto_delegate(input, &cwd).await
                {
                    turn_input = format!("{research}\n\n{turn_input}");
                }

                // Turno (puede ser agéntico: el mentor pide archivos con dpx:read,
                // se los leemos y continúa, hasta dar su respuesta final).
                // Si el cerebro no llega a responder nada (sin saldo, cuota, caído),
                // se degrada al siguiente con API key y se reintenta UNA vez.
                ui::clear_cancel();
                // Snapshot del consumo ANTES del turno: el delta = lo que costó.
                let tok_before = crate::token::totals();
                let mut outcome = run_turn(
                    &mentor,
                    &mut history,
                    &cwd,
                    &skin,
                    &mut |p| ed.confirm_line(p),
                    &store,
                    &turn_input,
                    auto,
                )
                .await;
                if let TurnOutcome::ModelFailed(err) = &outcome {
                    let err = err.clone();
                    if let Some(next) = fallback_brain(brain) {
                        println!(
                            "\n{} {} no responde → probando con {}",
                            ui::accent("⏺"),
                            router.brain_label(),
                            next.label()
                        );
                        println!("  {}", ui::dim(&ui::friendly_error(&err)));
                        let new_router = ModelRouter::new(next);
                        match build_mentor(
                            &new_router, focus_id.as_deref(), mode, prior.as_deref(),
                        ) {
                            Ok(m) => {
                                router = new_router;
                                mentor = m;
                                brain = next;
                                outcome = run_turn(
                                    &mentor,
                                    &mut history,
                                    &cwd,
                                    &skin,
                                    &mut |p| ed.confirm_line(p),
                                    &store,
                                    &turn_input,
                                    auto,
                                )
                                .await;
                            }
                            Err(e) => println!("{} {e}", ui::dim("no pude cambiar de cerebro:")),
                        }
                    }
                }
                match outcome {
                    TurnOutcome::Reply(full) => {
                        // (Los skills son curados en skills/*.md; ya no se
                        // auto-aprende nada de la respuesta — eso daba genérico.)
                        store.checkpoint("assistant", &full)?;
                        turns.push(Turn { role: "assistant", text: full });
                    }
                    TurnOutcome::Empty => {}
                    TurnOutcome::ModelFailed(err) => {
                        ui::panel("⚠ error del modelo", &ui::friendly_error(&err));
                    }
                }

                // Consumo real del turno (in/out + % de caché + costo aprox).
                if let Some(line) = crate::token::turn_line(tok_before) {
                    println!("{}", ui::dim(&format!("  {line}")));
                }
                // Aviso si se rebasó el presupuesto de la sesión (si hay uno).
                if crate::token::over_budget() {
                    println!(
                        "{}",
                        ui::accent(&format!(
                            "  ⚠ presupuesto superado · {}",
                            crate::token::budget_status().unwrap_or_default()
                        ))
                    );
                }

                // Compactación automática al acercarse al límite de contexto.
                if estimate_tokens(&history) > compact_threshold(brain) {
                    println!(
                        "\n{}",
                        ui::dim("el contexto se acerca al límite · compactando automáticamente")
                    );
                    compact_now(&router, &mut history, &turns).await;
                }
            }
            // Ctrl-C: cancela la entrada en curso, no sale (para no perder la sesión).
            Ok(ReadResult::Interrupted) => {
                println!("{}", ui::dim("(Ctrl-C — escribe /salir o pulsa Ctrl-D para terminar)"));
            }
            // Ctrl-D: salida limpia.
            Ok(ReadResult::Eof) => break,
            Err(e) => {
                eprintln!("[error de entrada] {e}");
                break;
            }
        }
    }

    close_session(&router, &store, &turns, prior.as_deref(), &mut mem_store, &mut embedder).await;
    // Devuelve el título de la pestaña a algo neutro al salir.
    ui::set_title("dpx");
    Ok(())
}

/// Arranque inteligente cuando no se pasó `--focus`.
///
/// CASO A — hay `.dpx/context.md`: muestra un resumen corto y pregunta si
/// continuar; si el usuario dice que no, la memoria no se inyecta esta sesión.
/// CASO B — proyecto nuevo: detecta el stack por los archivos de la raíz y
/// arranca directo, sin más preguntas.
///
/// Devuelve `(focus resuelto, contexto previo a inyectar)`; focus `None` =
/// mentor genérico sin skills de dominio.
fn startup_flow(cwd: &Path, store: &ProjectStore) -> (Option<String>, Option<String>) {
    let detected = crate::fs::detect_stack(cwd).map(str::to_string);

    match store.prior_context() {
        Some(ctx) => {
            let project = cwd
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "proyecto".to_string());
            let last_step = context_snippet(&ctx, "Próximos pasos")
                .unwrap_or_else(|| "sesión anterior".to_string());
            ui::resume_banner(&project, &last_step);
            if let Some(estado) = context_snippet(&ctx, "Estado del proyecto") {
                println!("  {}", ui::dim(&estado));
            }
            if let Some(resumen) = context_snippet(&ctx, "Resumen de sesión") {
                println!("  {}", ui::dim(&resumen));
            }
            if ask_continue() {
                (detected, Some(ctx))
            } else {
                println!("  {}", ui::dim("ok, esta sesión arranca sin la memoria anterior"));
                (detected, None)
            }
        }
        None => {
            match detected.as_deref() {
                Some(id) => ui::detected_banner(focus::display_name(Some(id))),
                None => println!("\n  {}", ui::dim("⏺ stack no reconocido · mentor genérico")),
            }
            (detected, None)
        }
    }
}

/// Primera línea con contenido bajo un encabezado `#` del context.md (acotada),
/// para el resumen de arranque.
fn context_snippet(context: &str, heading: &str) -> Option<String> {
    const MAX_CHARS: usize = 90;
    let mut in_section = false;
    for line in context.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix('#') {
            in_section = rest.trim_start_matches('#').trim().eq_ignore_ascii_case(heading);
            continue;
        }
        if in_section {
            let clean = t.trim_start_matches(['-', '*']).trim();
            if clean.is_empty() {
                continue;
            }
            if clean.chars().count() <= MAX_CHARS {
                return Some(clean.to_string());
            }
            let cut: String = clean.chars().take(MAX_CHARS).collect();
            return Some(format!("{cut}…"));
        }
    }
    None
}

/// Pregunta `¿continuar? [S/n]` por stdin (rustyline aún no existe en el arranque).
/// Enter o cualquier cosa que no sea "n"/"no" cuenta como sí.
fn ask_continue() -> bool {
    // Sin TTY (stdin pipeado / dpx manejado por script): NO consumas una línea
    // de stdin — sería el mensaje del usuario, no la respuesta a este prompt.
    // Usa el default (continuar) y sigue, para que dpx sea manejable headless.
    if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        println!("  {}", ui::dim("(sin TTY · retomo el contexto por defecto)"));
        return true;
    }
    print!("  ¿continuar? [S/n] ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return true;
    }
    !matches!(answer.trim().to_lowercase().as_str(), "n" | "no")
}

/// Presupuesto inicial de rondas agénticas por turno (lee/escribe/ejecuta →
/// itera). Al agotarse, el turno NO muere: se le pregunta al usuario si dpx
/// sigue (checkpoint humano) y el presupuesto se amplía en bloques de este
/// tamaño. En modo auto se amplía solo, hasta [`AUTO_MAX_ROUNDS`].
const MAX_TURN_ROUNDS: usize = 8;

/// Tope duro de rondas en modo auto (sin humano que frene, algo tiene que hacerlo).
const AUTO_MAX_ROUNDS: usize = 32;

/// Reintentos de ronda cuando la conexión se corta a MITAD de un turno (texto
/// ya emitido): antes esto mataba el turno entero — la queja nº 1 del usuario.
const MAX_STREAM_RETRIES: usize = 2;

/// Rondas EXTRA que el green-gate fuerza cuando el modelo intenta cerrar con el
/// build/tests en rojo. Acotado: si tras estos intentos sigue rojo, se cierra
/// con un aviso ROJO honesto en vez de fingir verde (mejor "incompleto" claro
/// que un falso "listo"). No queremos un bucle infinito contra un fallo real.
const GATE_MAX_FORCED: usize = 3;

/// Rondas EXTRA que el gate de AUTORREVISIÓN fuerza cuando el revisor encuentra
/// problemas P0/P1. Acotado para no ciclar contra un revisor exigente: tras
/// estos intentos se cierra (el humano del modo auto verá el último estado).
const SELF_REVIEW_MAX_ROUNDS: usize = 2;

/// Resultado de un turno completo.
enum TurnOutcome {
    /// Respuesta (posiblemente parcial) del asistente, para memoria.
    Reply(String),
    /// El modelo respondió vacío: nada que guardar.
    Empty,
    /// El modelo no llegó a responder nada (falló la primera ronda):
    /// candidato a degradar a otro cerebro y reintentar.
    ModelFailed(String),
}

/// Umbral de compactación automática: al superar el 75% de la ventana del
/// cerebro ACTIVO, el historial se resume con el modelo barato. Depende del
/// cerebro (DeepSeek: 128k).
fn compact_threshold(brain: Brain) -> usize {
    brain.context_budget() * 3 / 4
}

/// Mensajes recientes que se conservan intactos al compactar (los últimos
/// intercambios suelen ser a los que el usuario se refiere con "eso", "ahí").
const KEEP_RECENT_MESSAGES: usize = 4;

/// Compacta el historial en caliente: resumen barato de la sesión + los últimos
/// mensajes intactos. Best-effort: si el resumen falla, el historial no se toca.
async fn compact_now(
    router: &ModelRouter,
    history: &mut Vec<Message>,
    turns: &[Turn],
) {
    if history.is_empty() {
        println!("{}", ui::dim("no hay conversación que compactar"));
        return;
    }
    let before = estimate_tokens(history);
    let spinner = ui::Spinner::start("Compactando contexto…");
    let summary = session::compact(router, turns).await;
    spinner.stop();
    match summary {
        Ok(md) => {
            rebuild_history(history, &md);
            println!(
                "{} contexto compactado · ~{}k → ~{}k tokens",
                ui::accent("⏺"),
                before / 1000,
                estimate_tokens(history) / 1000
            );
        }
        Err(e) => println!(
            "{} {}",
            ui::dim("no pude compactar:"),
            ui::friendly_error(&e.to_string())
        ),
    }
}

/// Reconstruye el historial tras compactar: [resumen + ack] y los últimos
/// [`KEEP_RECENT_MESSAGES`] mensajes intactos.
fn rebuild_history(history: &mut Vec<Message>, summary: &str) {
    let keep_from = history.len().saturating_sub(KEEP_RECENT_MESSAGES);
    let mut recent = history.split_off(keep_from);
    
    // Evitar dejar tool_results o tool_calls huérfanos sin su pareja previa,
    // ya que eso causa un error 400 Bad Request en la mayoría de las APIs.
    // Nota: rig-core serializa los tool_results con "type":"toolresult".
    while !recent.is_empty() {
        if let Ok(json) = serde_json::to_string(&recent[0])
            && (json.contains(r#""type":"toolresult""#) || json.contains(r#""tool_calls""#)) {
                recent.remove(0);
                continue;
            }
        break;
    }

    history.clear();
    history.push(Message::user(format!(
        "[CONTEXTO COMPACTADO] La conversación previa se resumió para liberar espacio. \
         Resumen para continuar sin perder el hilo:\n\n{summary}"
    )));
    history.push(Message::assistant(
        "Entendido: tengo presente ese contexto y continúo desde ahí.",
    ));
    history.extend(recent);
}

/// Tamaño (en chars) a partir del cual el cuerpo de un tool result VIEJO se
/// considera voluminoso y se elide. Por debajo no compensa: el stub pesaría casi
/// lo mismo que el contenido.
const TOOL_OUTPUT_ELIDE_OVER: usize = 1200;

/// Cantidad de mensajes finales que NUNCA se tocan al elidir: los últimos tool
/// results suelen ser justo los que el modelo está usando para la ronda actual.
const KEEP_RECENT_TOOL_OUTPUTS: usize = 6;

/// Marca con la que empieza un stub de elisión (para no volver a elidir lo ya
/// elidido en turnos posteriores → tras la primera pasada, el prefijo del
/// historial se reestabiliza y el caché de contexto vuelve a pegar).
const ELIDED_PREFIX: &str = "[salida elidida";

/// Compactación LIGERA: elide el cuerpo de los tool results viejos y voluminosos
/// (archivos leídos / salidas de comandos de rondas anteriores que el modelo ya
/// no necesita enteros), dejando un stub corto. CLAVE: no borra mensajes ni
/// cambia ids → el emparejamiento tool_call/tool_result queda intacto (los
/// huérfanos son justo lo que dispara los 400 de las APIs). Solo toca tool
/// results de TEXTO largos que NO estén entre los últimos
/// [`KEEP_RECENT_TOOL_OUTPUTS`] mensajes. Devuelve cuántos elidió.
fn prune_tool_outputs(history: &mut [Message]) -> usize {
    let cutoff = history.len().saturating_sub(KEEP_RECENT_TOOL_OUTPUTS);
    let mut elided = 0;
    for msg in history.iter_mut().take(cutoff) {
        let Message::User { content } = msg else { continue };
        for uc in content.iter_mut() {
            let UserContent::ToolResult(tr) = uc else { continue };
            for trc in tr.content.iter_mut() {
                let ToolResultContent::Text(t) = trc else { continue };
                if t.text.len() > TOOL_OUTPUT_ELIDE_OVER && !t.text.starts_with(ELIDED_PREFIX) {
                    let n = t.text.chars().count();
                    t.text = format!(
                        "{ELIDED_PREFIX} para ahorrar contexto: ~{n} caracteres de una ronda \
                         anterior. Si necesitas ese contenido otra vez, vuelve a leer el archivo \
                         o re-ejecuta la acción.]"
                    );
                    elided += 1;
                }
            }
        }
    }
    elided
}

/// Se resuelve cuando el usuario pide cancelar (Ctrl-C fuera del prompt).
/// Para correr en `tokio::select!` contra la espera del modelo.
async fn wait_for_cancel() {
    loop {
        if ui::cancel_requested() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// El siguiente cerebro disponible (con API key) distinto del actual, en orden
/// de utilidad agéntica, para degradar cuando el activo no responde.
fn fallback_brain(current: Brain) -> Option<Brain> {
    Brain::all()
        .into_iter()
        .find(|b| b.name() != current.name() && b.has_key())
}

/// Lo ÚNICO que `run_turn` necesita del modelo — costura de testabilidad: en
/// producción lo implementa [`Mentor`]; en tests, un fake con respuestas
/// guionadas que permite ejercitar el loop agéntico completo sin red.
trait TurnBrain {
    async fn chat_stream(
        &self,
        input: &str,
        history: &mut Vec<Message>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<ChatReply>;
}

impl TurnBrain for Mentor {
    async fn chat_stream(
        &self,
        input: &str,
        history: &mut Vec<Message>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<ChatReply> {
        Mentor::chat_stream(self, input, history, on_delta).await
    }
}

/// Ejecuta un turno completo, posiblemente agéntico. En cada ronda: renderiza la
/// narración, aplica escrituras (con confirmación), y atiende lecturas (libres) y
/// ejecuciones `dpx:run` (con confirmación); si hubo acciones, devuelve los
/// resultados al modelo y vuelve a iterar, hasta que da su respuesta final.
/// `ask` responde las confirmaciones (producción: el editor; tests: respuestas fijas).
#[allow(clippy::too_many_arguments)]
async fn run_turn(
    mentor: &impl TurnBrain,
    history: &mut Vec<Message>,
    cwd: &Path,
    skin: &termimad::MadSkin,
    ask: &mut dyn FnMut(&str) -> Option<String>,
    store: &ProjectStore,
    user_input: &str,
    auto: crate::cli::AutoMode,
) -> TurnOutcome {
    let mut to_send = attach_files(cwd, user_input);
    let mut full = String::new();
    let mut round = 0usize;
    let mut round_budget = MAX_TURN_ROUNDS;
    let mut stream_retries = 0usize;
    // GREEN-GATE: veredicto determinista de la ÚLTIMA verificación automática
    // (build/tests que inyecta dpx). `true` = quedó en rojo. Bloquea el cierre
    // del turno hasta que vuelva a verde. `gate_forced` acota cuántas rondas
    // extra forzamos para que un build irreparable no cicle eternamente.
    let mut verify_red = false;
    let mut gate_forced = 0usize;
    // AUTORREVISIÓN: archivos que el turno cambió (para que el revisor compare el
    // diff contra lo pedido) y cuántas rondas de corrección por revisión llevamos.
    let mut changed_paths: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut review_forced = 0usize;
    // Checkpoint del turno: captura el estado de cada archivo antes de tocarlo
    // (vía fs::apply/delete_file) y lo apila al terminar para que `/undo` pueda
    // revertir TODO lo que dpx escribió este turno. Se commitea al soltar el
    // guard, pase lo que pase (incluido un return temprano).
    let _checkpoint = crate::checkpoint::TurnGuard::begin();
    // Archivos ya inyectados en ESTE turno: si el modelo vuelve a pedir el mismo
    // en otra ronda, su contenido ya está en el historial → no lo re-mandamos
    // (ahorro de tokens en loops agénticos que releen lo mismo).
    let mut read_paths_seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // Compactación ligera al empezar el turno: elide el cuerpo de los tool
    // results viejos y voluminosos de turnos anteriores (su contenido ya cumplió
    // su función) para no arrastrar archivos enteros ronda tras ronda. Conserva
    // ids/estructura → emparejamiento tool intacto. Una vez por turno: acota el
    // trasiego del caché de contexto (cada elisión rompe el prefijo una sola vez).
    let pruned = prune_tool_outputs(history);
    if pruned > 0 {
        println!(
            "{}",
            ui::dim(&format!("  ↯ {pruned} salida(s) de herramienta antiguas elididas · ahorro de contexto"))
        );
    }

    loop {
        round += 1;
        // Verbos rotativos + título de pestaña según la fase del turno.
        let spinner = if round == 1 { ui::Spinner::thinking() } else { ui::Spinner::working() };
        ui::title_busy(if round == 1 { "pensando" } else { "trabajando" });
        // Carrera contra la cancelación: Ctrl-C durante la espera aborta el turno
        // (soltar el future corta la petición; el historial no se tocó aún).
        let mut on_delta = |_: &str| {};
        let result = tokio::select! {
            r = mentor.chat_stream(&to_send, history, &mut on_delta) => Some(r),
            _ = wait_for_cancel() => None,
        };
        spinner.stop();
        let Some(result) = result else {
            ui::clear_cancel();
            println!("{}", ui::dim("✗ turno cancelado (Ctrl-C)"));
            return if full.trim().is_empty() {
                TurnOutcome::Empty
            } else {
                TurnOutcome::Reply(full)
            };
        };

        let ChatReply { text: reply, calls, usage } = match result {
            Ok(r) => r,
            Err(e) => {
                if full.trim().is_empty() {
                    return TurnOutcome::ModelFailed(e.to_string());
                }
                // La conexión murió a MITAD del turno con trabajo ya hecho.
                // Si el error es transitorio, la ronda se reintenta avisando
                // al modelo (su respuesta cortada NO quedó en el historial)
                // en vez de matar el turno entero.
                if crate::agent::is_transient_error(&e.to_string())
                    && stream_retries < MAX_STREAM_RETRIES
                {
                    stream_retries += 1;
                    println!(
                        "{}",
                        ui::dim("la conexión se cortó a mitad del turno · reintentando la ronda")
                    );
                    to_send = "[Tu respuesta anterior se cortó a mitad por un error de red y \
                               NO quedó registrada: ninguna acción que pidieras en ella se \
                               ejecutó. Continúa la tarea re-emitiendo desde donde ibas.]"
                        .to_string();
                    continue;
                }
                ui::panel("⚠ error del modelo", &ui::friendly_error(&e.to_string()));
                return TurnOutcome::Reply(full);
            }
        };
        full.push_str(&reply);
        full.push('\n');

        // Consumo real de tokens de esta ronda (in/out/cached) al ledger de sesión.
        crate::token::record(&usage);

        // 1. Narración de esta ronda (sin bloques de acción) → Markdown progresivo.
        let body = crate::fs::strip_action_blocks(&reply);
        if !body.trim().is_empty() {
            ui::render_reply(skin, &body);
        }

        // Plan/checklist: si el modelo emitió un `dpx:plan`, lo pintamos vivo (☐/☑).
        if let Some(plan) = crate::fs::parse_plan(&reply) {
            ui::checklist(&plan);
        }

        // Modo learn: si el tutor registró habilidades (bloque `dpx:skill`), las
        // fusiona con el progreso del usuario y las persiste (`/progreso` las ve).
        let learned = crate::skill::parse_block(&reply, &crate::skill::today());
        if !learned.is_empty() {
            let merged = crate::skill::merge(store.read_skills(), learned.clone());
            if store.write_skills(&merged).is_ok() {
                println!(
                    "{}",
                    ui::dim(&format!(
                        "⎿ progreso actualizado: {} concepto(s) · usa /progreso",
                        learned.len()
                    ))
                );
            }
        }

        // CUARENTENA: una respuesta con bloques dpx malformados no es de fiar —
        // un fence roto puede convertir texto en "comandos" fantasma (pasó en
        // vivo: un ejemplo de `mvn` dentro de un edit roto casi se ejecuta).
        // Si hay CUALQUIER bloque malformado, NINGÚN bloque de texto de esta
        // respuesta se ejecuta; el modelo re-emite. Las tool calls nativas no
        // se ven afectadas (llegan validadas por el API, sin parseo de fences).
        let mut report = ActionReport::default();
        let malformed = crate::fs::detect_malformed_actions(&reply);
        let quarantined = !malformed.is_empty();
        if quarantined {
            println!(
                "\n{}",
                ui::dim("⚠ bloques dpx malformados · cuarentena: ningún bloque de texto de esta respuesta se ejecuta")
            );
            report.followup(
                "[CUARENTENA: tu respuesta contenía bloques dpx:* malformados, así que NINGUNA \
                 acción en bloques de texto de esa respuesta se ejecutó (ni siquiera las bien \
                 formadas). Re-emite TODAS tus acciones pendientes, preferiblemente como tool \
                 calls nativas (inmunes a este problema).]"
                    .to_string(),
            );
            for warning in malformed {
                report.followup(warning);
            }
        }

        // 2. Cambios propuestos en esta ronda (escrituras, ediciones quirúrgicas y
        //    borrados): confirmar y aplicar YA, guardando el resultado de cada uno
        //    para informárselo al modelo (que no asuma que algo se aplicó si el
        //    usuario lo rechazó o falló).
        let writes = if quarantined { Vec::new() } else { crate::fs::parse_writes(&reply) };
        report.absorb(process_writes(cwd, &writes, &mut *ask, auto));

        let edits = if quarantined { Vec::new() } else { crate::fs::parse_edits(&reply) };
        report.absorb(process_edits(cwd, &edits, &mut *ask, auto));

        let deletes = if quarantined { Vec::new() } else { crate::fs::parse_deletes(&reply) };
        report.absorb(process_deletes(cwd, &deletes, &mut *ask));

        // Hooks PostToolUse: tras aplicar cambios, ejecutar comandos
        // (p.ej. cargo fmt después de writes/edits).
        let all_tools_used = {
            let mut names: Vec<&str> = Vec::new();
            if !writes.is_empty() { names.push("write_file"); }
            if !edits.is_empty() { names.push("edit_file"); }
            if !deletes.is_empty() { names.push("delete_file"); }
            names
        };
        if !all_tools_used.is_empty() {
            let hooks = store.load_hooks();
            for name in &all_tools_used {
                crate::cli::hooks::run_hooks(&hooks, &crate::cli::hooks::HookEvent::PostToolUse, Some(name), cwd);
            }
        }

        // 2b. Tool calls nativas (function calling): la vía estructurada y
        //     preferida. Se atienden SIEMPRE y cada una deja su tool result en
        //     el historial — sin un resultado por llamada, la siguiente
        //     petición al proveedor sería inválida (tool_calls colgados).
        let mut s_writes: Vec<crate::fs::FileWrite> = Vec::new();
        let mut s_edits: Vec<crate::fs::FileEdit> = Vec::new();
        let mut cancelled_at: Option<usize> = None;
        for (i, call) in calls.iter().enumerate() {
            let outcome =
                run_tool_call(cwd, store, &mut *ask, call, &mut s_writes, &mut s_edits, auto)
                    .await;
            let (text, cancelled) = match outcome {
                ToolOutcome::Done(t) => (t, false),
                ToolOutcome::Cancelled(t) => (t, true),
            };
            // Caja negra: la tool call y su resultado quedan en la
            // transcripción de la sesión (.dpx/sessions) para autopsias.
            let _ = store.checkpoint(
                "tool",
                &format!(
                    "{}({}) → {}",
                    call.function.name,
                    truncate_log(&call.function.arguments.to_string(), 160),
                    truncate_log(&text, 300)
                ),
            );
            history.push(Message::tool_result(call.id.clone(), text));
            if cancelled {
                cancelled_at = Some(i);
                break;
            }
        }
        if let Some(done) = cancelled_at {
            // Las llamadas que quedaron sin atender también necesitan su
            // resultado en el historial, aunque sea "no se ejecutó".
            for call in &calls[done + 1..] {
                history.push(Message::tool_result(
                    call.id.clone(),
                    "[no ejecutado: el usuario canceló el turno]".to_string(),
                ));
            }
            ui::clear_cancel();
            println!("{}", ui::dim("✗ comando interrumpido (Ctrl-C) · turno cancelado"));
            return if full.trim().is_empty() {
                TurnOutcome::Empty
            } else {
                TurnOutcome::Reply(full)
            };
        }

        // 3. Hooks PostToolUse para tool calls nativas que modificaron.
        let native_modifiers: Vec<&str> = calls
            .iter()
            .map(|c| c.function.name.as_str())
            .filter(|n| matches!(*n, "write_file" | "edit_file" | "delete_file"))
            .collect();
        if !native_modifiers.is_empty() {
            let hooks = store.load_hooks();
            for name in native_modifiers {
                crate::cli::hooks::run_hooks(&hooks, &crate::cli::hooks::HookEvent::PostToolUse, Some(name), cwd);
            }
        }

        // 4. Lecturas, búsquedas y ejecuciones (también en cuarentena si aplica:
        //    el `mvn` fantasma de un fence roto era justamente un run parseado).
        let mut reads = Vec::new();
        if !quarantined {
            for r in crate::fs::parse_reads(&reply) {
                if !reads.contains(&r) {
                    reads.push(r);
                }
            }
        }

        let searches = if quarantined { Vec::new() } else { crate::fs::parse_searches(&reply) };
        let mut runs = if quarantined { Vec::new() } else { crate::fs::parse_runs(&reply) };

        // AUTORREVISIÓN: acumula los archivos que esta ronda cambió (en modo auto
        // ya se aplicaron sin preguntar) para que el revisor los compare contra lo
        // pedido al cerrar. Dedup por el HashSet.
        for p in writes
            .iter()
            .map(|w| &w.path)
            .chain(s_writes.iter().map(|w| &w.path))
            .chain(edits.iter().map(|e| &e.path))
            .chain(s_edits.iter().map(|e| &e.path))
        {
            changed_paths.insert(p.clone());
        }

        // Verificación de build automática: si escribimos código fuente y hay un
        // proyecto Maven/Gradle/Cargo, dpx propone compilar y realimenta los
        // errores al modelo para que itere (escribe → compila → corrige), sin que
        // tenga que pedirlo. Se omite si el modelo ya pidió el mismo build tool.
        let mut auto_built = false;
        let mut auto_tested = false;
        // Frontera: lo que va DESPUÉS de este índice en `runs` es verificación
        // que inyectó dpx (no comandos del modelo) → es lo que vigila el gate.
        let runs_before_verify = runs.len();
        if crate::fs::touches_build(&writes)
            || crate::fs::edits_touch_build(&edits)
            || crate::fs::touches_build(&s_writes)
            || crate::fs::edits_touch_build(&s_edits)
        {
            let already = runs
                .iter()
                .any(|r| r.contains("mvn") || r.contains("gradle") || r.contains("cargo"));
            if !already {
                // Modo full-auto (`/auto all`): el agente verifica DE VERDAD.
                // En Rust corre clippy ESTRICTO (`detect_build` → `-D warnings`)
                // ADEMÁS de la suite de tests: `cargo test` NO deniega warnings,
                // así que sin clippy el agente deja pasar código muerto/lints que
                // luego revienta el CI (pasó en vivo). En otros stacks la suite ya
                // compila, no hace falta el paso extra.
                if auto.commands() {
                    if cwd.join("Cargo.toml").exists()
                        && let Some(lint_cmd) = crate::fs::detect_build(cwd) {
                            runs.push(lint_cmd);
                            auto_built = true;
                        }
                    if let Some(test_cmd) = crate::fs::detect_test(cwd) {
                        runs.push(test_cmd);
                        auto_tested = true;
                    }
                }
                // Modos menos autónomos: basta el compile/lint-check, más rápido y
                // sin los efectos secundarios de los tests.
                if !auto_tested
                    && !auto_built
                    && let Some(build_cmd) = crate::fs::detect_build(cwd) {
                        runs.push(build_cmd);
                        auto_built = true;
                    }
            }
        }

        let has_requests =
            !reads.is_empty() || !searches.is_empty() || !runs.is_empty() || !calls.is_empty();
        let wants_more = has_requests || report.needs_followup;

        // Presupuesto de rondas agotado con la tarea aún VIVA: checkpoint en
        // vez de muerte silenciosa (la causa nº 1 de turnos "muertos a la
        // mitad"). En manual pregunta; en auto amplía solo hasta el tope duro.
        if wants_more
            && round >= round_budget
            && !extend_rounds(&mut *ask, round, auto, &mut round_budget)
        {
            // Si encima la verificación quedó en ROJO, no lo disfraces de pausa
            // tranquila: dilo claro para que el usuario sepa que el build/tests
            // siguen rotos al detenerse.
            if verify_red {
                println!(
                    "{}",
                    ui::red("🔴 turno detenido CON EL BUILD/TESTS EN ROJO · revisa y retoma")
                );
            } else {
                println!(
                    "{}",
                    ui::dim("⏸ turno detenido · el plan y la memoria quedan guardados para retomar")
                );
            }
            break;
        }
        if wants_more {
            let mut ctx = String::new();
            if !report.notes.is_empty() {
                ctx.push_str("\n--- resultado de los cambios propuestos ---\n");
                for note in &report.notes {
                    ctx.push_str(note);
                    ctx.push('\n');
                }
                ctx.push_str("--- fin ---\n");
            }
            if !reads.is_empty() {
                ui::title_busy("explorando código");
            }
            for r in &reads {
                ui::action_read(r);
                // Ya leído en este turno: su contenido está más arriba en la
                // conversación, no lo duplicamos (ahorro de tokens).
                if !read_paths_seen.insert(r.clone()) {
                    ctx.push_str(&format!(
                        "\n--- `{r}` ya lo leíste antes en este turno; su contenido está más arriba. No lo volví a pegar. ---\n"
                    ));
                    continue;
                }
                match crate::fs::read_file(cwd, r) {
                    Ok(c) => ctx.push_str(&format!("\n--- `{r}` ---\n{c}\n--- fin `{r}` ---\n")),
                    Err(e) => ctx.push_str(&format!("\n[no pude leer `{r}`: {e}]\n")),
                }
            }
            if !searches.is_empty() {
                ui::title_busy("buscando en el proyecto");
            }
            for s in &searches {
                println!("{}", ui::accent(&format!("  ⎁ buscando: {}", s)));
                let out = crate::fs::search_in_project(cwd, s);
                ctx.push_str(&format!("\n--- resultados de búsqueda para `{s}` ---\n{out}\n--- fin ---\n"));
            }
            if auto_tested && auto_built {
                println!("\n{}", ui::dim("dpx verifica con el linter estricto y la suite de tests…"));
            } else if auto_tested {
                println!("\n{}", ui::dim("dpx verifica que el proyecto compile y pase los tests…"));
            } else if auto_built {
                println!("\n{}", ui::dim("dpx verifica el proyecto (compila/linter)…"));
            }
            // Si esta ronda corre verificación automática, partimos de VERDE y
            // cualquier comando de verificación que falle lo pone en rojo (OR de
            // todos: clippy Y tests deben pasar). Si la ronda no verifica, el
            // veredicto previo se conserva (un read no "limpia" un build roto).
            if auto_built || auto_tested {
                verify_red = false;
            }
            for (idx, cmd) in runs.iter().enumerate() {
                match confirm_run(&mut *ask, store, cwd, cmd, auto) {
                    RunDecision::Blocked(reason) => {
                        ctx.push_str(&format!(
                            "\n[dpx BLOQUEÓ el comando `{cmd}`: {reason}. Está prohibido vía dpx:run; \
                             NO lo vuelvas a proponer, busca otra forma o pídele al usuario que lo haga a mano.]\n"
                        ));
                        continue;
                    }
                    RunDecision::Refused => {
                        println!("{}", ui::dim("omitido."));
                        ctx.push_str(&format!("\n[el usuario rechazó ejecutar `{cmd}`]\n"));
                        continue;
                    }
                    RunDecision::Run => {}
                }
                let (out_text, cancelled, exit_code) = execute_run(cwd, cmd);
                ctx.push_str(&format!("\n--- salida de `{cmd}` ---\n{out_text}\n--- fin ---\n"));
                if cancelled {
                    ui::clear_cancel();
                    println!("{}", ui::dim("✗ comando interrumpido (Ctrl-C) · turno cancelado"));
                    return if full.trim().is_empty() {
                        TurnOutcome::Empty
                    } else {
                        TurnOutcome::Reply(full)
                    };
                }
                // GREEN-GATE: si este comando es la verificación que inyectó dpx
                // (no uno del modelo) y su exit code no fue 0, el turno queda en
                // rojo. `Some(0)` es la única señal de verde; `None` (ni arrancó)
                // o cualquier otro código cuenta como rojo.
                let is_auto_verify = (auto_built || auto_tested) && idx >= runs_before_verify;
                if is_auto_verify && exit_code != Some(0) {
                    verify_red = true;
                }
            }
            // Si solo hubo tool calls nativas, sus resultados ya viajan en el
            // historial como tool results: el prompt es solo la instrucción.
            to_send = if ctx.trim().is_empty() {
                "Los resultados de tus herramientas ya están en la conversación. Continúa con la \
                 tarea según esos resultados; si necesitas más acciones, pídelas; si ya \
                 terminaste, da tu respuesta final."
                    .to_string()
            } else {
                format!(
                    "Resultado de las acciones que pediste:\n{ctx}\n\nContinúa con la tarea según esto. \
                     Si necesitas más acciones (leer/ejecutar/escribir), pídelas; si ya terminaste, da \
                     tu respuesta final."
                )
            };
            continue;
        }
        // 🟢 GREEN-GATE: el modelo no pidió más acciones → se da por terminado.
        // Pero si la última verificación automática quedó en ROJO, no cerramos en
        // falso: lo empujamos a una ronda de corrección. Acotado por
        // `GATE_MAX_FORCED` para no ciclar contra un fallo que no sabe resolver;
        // agotados los intentos, cerramos con un aviso ROJO honesto (mejor un
        // "incompleto" claro que un falso "listo" con el build roto).
        if verify_red {
            if gate_forced < GATE_MAX_FORCED {
                gate_forced += 1;
                println!(
                    "{}",
                    ui::red("🔴 build/tests en ROJO · no cierro el turno hasta verde")
                );
                to_send = "[GREEN-GATE: la última verificación automática (build/tests) quedó en \
                           ROJO, pero diste el turno por terminado. NO lo está: vuelve a los errores \
                           reportados arriba y arréglalos hasta que el proyecto compile y los tests \
                           pasen en verde. Si un error no lo causaste tú o no se puede arreglar, dilo \
                           EXPLÍCITAMENTE y explica por qué; no lo dejes pasar en silencio.]"
                    .to_string();
                continue;
            }
            println!(
                "{}",
                ui::red("🔴 INCOMPLETO · el build/tests sigue en rojo tras varios intentos de arreglo")
            );
        }
        // 🔎 AUTORREVISIÓN (solo modo auto, build VERDE): antes de cerrar un turno
        // autónomo que cambió código, un revisor compara los cambios contra lo que
        // pidió el usuario. Si hay P0/P1, se fuerza una ronda de corrección — el
        // green-gate pero de COMPORTAMIENTO, no de compilación. Acotado por
        // `SELF_REVIEW_MAX_ROUNDS`. En interactivo el humano ES la revisión.
        if !verify_red
            && auto.commands()
            && !changed_paths.is_empty()
            && review_forced < SELF_REVIEW_MAX_ROUNDS
            && let Some(critique) = review::run_self_review(cwd, user_input, &changed_paths).await
        {
            review_forced += 1;
            println!(
                "{}",
                ui::accent("⏺ autorrevisión: hay P0/P1 que corregir antes de cerrar")
            );
            to_send = format!(
                "[AUTORREVISIÓN: un revisor evaluó tus cambios contra lo que pidió el usuario y \
                 encontró problemas que DEBES corregir antes de cerrar. Arréglalos; si crees que el \
                 revisor se equivoca en algo, explícalo con evidencia en vez de ignorarlo.\n\n{critique}\n]"
            );
            continue;
        }
        break;
    }

    if full.trim().is_empty() {
        TurnOutcome::Empty
    } else {
        TurnOutcome::Reply(full)
    }
}

/// Reinstala dpx desde el código del proyecto actual SIN cerrar la sesión —
/// cierra el ciclo de automejora (dpx se edita, se prueba y se INSTALA).
/// En Windows no se puede sobrescribir un exe en ejecución (os error 5), pero
/// SÍ renombrarlo: se aparta el binario vivo y `cargo install` escribe el
/// nuevo. Si la instalación falla, se restaura el binario original.
fn self_update(cwd: &Path) {
    // Solo dentro del repo de dpx: en otro proyecto Rust, `cargo install`
    // instalaría ESE binario y dpx quedaría apartado sin reemplazo.
    let manifest = std::fs::read_to_string(cwd.join("Cargo.toml")).unwrap_or_default();
    if !manifest.contains("name = \"dpx-cli\"") {
        println!(
            "{}",
            ui::dim("/actualizar solo funciona dentro del repo de dpx (este proyecto no es dpx-cli)")
        );
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            eprintln!("{} no pude localizar mi propio binario: {e}", ui::accent("⏺ error"));
            return;
        }
    };
    let parked = exe.with_extension("old.exe");
    let _ = std::fs::remove_file(&parked);
    if let Err(e) = std::fs::rename(&exe, &parked) {
        eprintln!("{} no pude apartar el binario en uso: {e}", ui::accent("⏺ error"));
        return;
    }

    println!("{}", ui::accent("⏺ recompilando e instalando dpx…"));
    let t = std::time::Instant::now();
    ui::clear_cancel();
    let mut shown = 0usize;
    let out = crate::fs::run_command_streaming(
        cwd,
        "cargo install --path . --force",
        600, // un build release desde cero tarda más que el timeout normal
        &mut |line| {
            shown += 1;
            if shown <= 12 {
                println!("{}", ui::dim(&format!("  │ {line}")));
            }
        },
        &|| ui::cancel_requested(),
    );
    ui::action_time(t.elapsed());

    if out.output.contains("exit code: 0") {
        println!(
            "{} {}",
            ui::accent("✓ dpx actualizado."),
            ui::dim("esta sesión sigue con el binario viejo · sal con /salir y vuelve a abrir dpx")
        );
    } else {
        // Rollback: que `dpx` siga existiendo en el PATH pase lo que pase.
        let _ = std::fs::remove_file(&exe);
        if let Err(e) = std::fs::rename(&parked, &exe) {
            eprintln!(
                "{} la instalación falló Y no pude restaurar el binario ({e}): ejecuta a mano `cargo install --path . --force`",
                ui::accent("⏺ error")
            );
            return;
        }
        println!(
            "{}",
            ui::dim("la instalación falló (¿no compila?) · binario anterior restaurado, dpx sigue funcionando")
        );
    }
}

/// Checkpoint al agotar el presupuesto de rondas: en manual pregunta al
/// usuario (Enter o `s` = seguir, `n` = parar); en modo auto amplía solo
/// hasta [`AUTO_MAX_ROUNDS`]. Si se continúa, amplía el presupuesto en otro
/// bloque de [`MAX_TURN_ROUNDS`].
fn extend_rounds(
    ask: &mut dyn FnMut(&str) -> Option<String>,
    round: usize,
    auto: crate::cli::AutoMode,
    budget: &mut usize,
) -> bool {
    // Guardrail de gasto: si se rebasó el presupuesto de tokens, el modo auto
    // NO sigue solo — cae al prompt manual para que decidas si quemar más.
    if auto.extends() && !crate::token::over_budget() {
        if round >= AUTO_MAX_ROUNDS {
            println!(
                "\n{}",
                ui::dim(&format!(
                    "⏸ tope del modo auto ({AUTO_MAX_ROUNDS} rondas) · revisa el avance y relanza la tarea"
                ))
            );
            return false;
        }
        println!(
            "\n{}",
            ui::dim(&format!("auto {round} rondas y la tarea sigue · ampliando presupuesto"))
        );
        *budget += MAX_TURN_ROUNDS;
        return true;
    }
    if auto.extends() && crate::token::over_budget() {
        println!(
            "\n{}",
            ui::accent(&format!(
                "⏸ presupuesto de tokens superado ({}) · auto en pausa, tú decides",
                crate::token::budget_status().unwrap_or_default()
            ))
        );
    }
    println!();
    let go = match ask(&format!("⏸ {round} rondas y dpx sigue trabajando · ¿continuar? [S/n] ")) {
        Some(ans) => !matches!(ans.trim().to_lowercase().as_str(), "n" | "no"),
        None => false,
    };
    if go {
        *budget += MAX_TURN_ROUNDS;
    }
    go
}

/// Recorta un texto para la transcripción (una tool call con un archivo
/// entero de resultado no debe inflar el jsonl).
fn truncate_log(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let cut: String = s.chars().take(max_chars).collect();
    format!("{cut}… [recortado]")
}

/// Resultado de atender una tool call nativa: el texto que va al historial
/// como tool result, y si la ejecución se canceló (aborta el turno).
enum ToolOutcome {
    Done(String),
    Cancelled(String),
}

/// Atiende UNA tool call: lecturas/búsquedas libres (incluida la web),
/// escrituras/ediciones/borrados con diff + confirmación (la misma maquinaria
/// que los bloques de texto) y comandos con el sandbox de `confirm_run`. Las
/// escrituras y ediciones se acumulan en `writes`/`edits` para el auto-build.
/// Async por `web_search` (HTTP).
async fn run_tool_call(
    cwd: &Path,
    store: &ProjectStore,
    ask: &mut dyn FnMut(&str) -> Option<String>,
    call: &rig_core::message::ToolCall,
    writes: &mut Vec<crate::fs::FileWrite>,
    edits: &mut Vec<crate::fs::FileEdit>,
    auto: crate::cli::AutoMode,
) -> ToolOutcome {
    match tools::parse_call(&call.function.name, &call.function.arguments) {
        Err(e) => {
            println!("\n{}", ui::dim(&format!("⚠ tool call inválida: {e}")));
            ToolOutcome::Done(format!("[ERROR: {e}]"))
        }
        Ok(DpxCall::Read { path, offset, limit }) => {
            ui::action_read(&path);
            ToolOutcome::Done(match crate::fs::read_file_range(cwd, &path, offset, limit) {
                Ok(c) => c,
                Err(e) => format!("[no pude leer `{path}`: {e}]"),
            })
        }
        Ok(DpxCall::Search { pattern }) => {
            println!("{}", ui::accent(&format!("  ⎁ buscando: {pattern}")));
            ToolOutcome::Done(crate::fs::search_in_project(cwd, &pattern))
        }
        Ok(DpxCall::WebSearch { query }) => {
            println!("{}", ui::accent(&format!("  ⌕ buscando en la web: {query}")));
            ToolOutcome::Done(match crate::agent::search::web_search(&query).await {
                Ok(results) => results,
                Err(e) => format!("[web_search falló: {e}]"),
            })
        }
        Ok(DpxCall::Spawn { task, role }) => {
            ToolOutcome::Done(run_subagent(cwd, &task, role.as_deref()).await)
        }
        Ok(DpxCall::LspDiagnostics { path }) => {
            println!("{}", ui::accent(&format!("  language server: {path}")));
            // El cliente LSP bloquea (espera diagnósticos con timeout): lo sacamos
            // del hilo async para no congelar el runtime.
            let cwd2 = cwd.to_path_buf();
            let p = path.clone();
            let out = match tokio::task::spawn_blocking(move || crate::lsp::diagnostics(&cwd2, &p)).await {
                Ok(Ok(s)) => s,
                Ok(Err(e)) => format!("[lsp_diagnostics: {e}]"),
                Err(e) => format!("[lsp_diagnostics se interrumpió: {e}]"),
            };
            ToolOutcome::Done(out)
        }
        Ok(DpxCall::FindReferences { path, line, symbol }) => {
            println!("{}", ui::accent(&format!("  find_references: {symbol} ({path}:{line})")));
            // El cliente LSP bloquea (indexa + espera respuesta): fuera del runtime.
            let cwd2 = cwd.to_path_buf();
            let (p, s) = (path.clone(), symbol.clone());
            let out = match tokio::task::spawn_blocking(move || {
                crate::lsp::references(&cwd2, &p, line, &s)
            })
            .await
            {
                Ok(Ok(text)) => text,
                Ok(Err(e)) => format!("[find_references: {e}]"),
                Err(e) => format!("[find_references se interrumpió: {e}]"),
            };
            ToolOutcome::Done(out)
        }
        Ok(DpxCall::RenameSymbol { path, line, symbol, new_name }) => {
            println!(
                "{}",
                ui::accent(&format!("  rename_symbol: {symbol} → {new_name} ({path}:{line})"))
            );
            let cwd2 = cwd.to_path_buf();
            let (p, s, nn) = (path.clone(), symbol.clone(), new_name.clone());
            let result = tokio::task::spawn_blocking(move || {
                crate::lsp::rename(&cwd2, &p, line, &s, &nn)
            })
            .await;
            match result {
                Ok(Ok((rename_writes, notes))) => {
                    if rename_writes.is_empty() {
                        ToolOutcome::Done(
                            "[rename_symbol: el language server no devolvió cambios (¿símbolo no \
                             renombrable, o aún indexando? reintenta en unos segundos)]"
                                .to_string(),
                        )
                    } else {
                        println!(
                            "{}",
                            ui::dim(&format!("  ⎿ {} archivo(s) afectado(s)", rename_writes.len()))
                        );
                        // Pasa por el flujo normal: diff + confirmación + checkpoint
                        // (/undo) + auto-build (green-gate) por cada archivo tocado.
                        let report = process_writes(cwd, &rename_writes, ask, auto);
                        let mut out = report.notes.clone();
                        out.extend(notes);
                        for w in rename_writes {
                            writes.push(w);
                        }
                        ToolOutcome::Done(out.join("\n"))
                    }
                }
                Ok(Err(e)) => ToolOutcome::Done(format!("[rename_symbol: {e}]")),
                Err(e) => ToolOutcome::Done(format!("[rename_symbol se interrumpió: {e}]")),
            }
        }
        Ok(DpxCall::Write { path, content }) => {
            let w = crate::fs::FileWrite { path, content };
            let report = process_writes(cwd, std::slice::from_ref(&w), ask, auto);
            writes.push(w);
            ToolOutcome::Done(report.notes.join("\n"))
        }
        Ok(DpxCall::Edit { path, search, replace }) => {
            let e = crate::fs::FileEdit { path, search, replace };
            let report = process_edits(cwd, std::slice::from_ref(&e), ask, auto);
            edits.push(e);
            ToolOutcome::Done(report.notes.join("\n"))
        }
        Ok(DpxCall::Delete { path }) => {
            let report = process_deletes(cwd, &[path], ask);
            ToolOutcome::Done(report.notes.join("\n"))
        }
        Ok(DpxCall::GitStatus) => ToolOutcome::Done(run_git(cwd, &["status", "--short"])),
        Ok(DpxCall::GitDiff { path }) => {
            let mut args = vec!["diff"];
            if let Some(p) = &path {
                args.push(p);
            }
            ToolOutcome::Done(run_git(cwd, &args))
        }
        Ok(DpxCall::GitLog { n }) => {
            let count = format!("-{}", n.unwrap_or(10).min(50));
            ToolOutcome::Done(run_git(cwd, &["log", "--oneline", &count]))
        }
        Ok(DpxCall::GitCommit { message }) => {
            // git_commit MUTA el repo: confirma (o auto). El mensaje se pasa
            // como arg ÚNICO (run_git no parte por espacios), así que mensajes
            // con espacios funcionan bien.
            println!("\n{} {}", ui::accent("⏺ commit:"), ui::accent(&message));
            let ok = auto.commands() || matches!(ask("¿crear commit? [s/N] "), Some(a) if is_yes(&a));
            if !ok {
                println!("{}", ui::dim("omitido."));
                ToolOutcome::Done("[el usuario rechazó crear el commit]".to_string())
            } else {
                // Hooks PreCommit: si fallan, el commit se cancela.
                let hooks = store.load_hooks();
                if !crate::cli::hooks::run_hooks(&hooks, &crate::cli::hooks::HookEvent::PreCommit, None, cwd) {
                    println!("{}", ui::dim("commit cancelado: el hook PreCommit falló"));
                    return ToolOutcome::Done("[commit cancelado: el hook PreCommit falló]".to_string());
                }
                let add = run_git(cwd, &["add", "-A"]);
                let commit = run_git(cwd, &["commit", "-m", &message]);
                ToolOutcome::Done(format!("git add -A:\n{add}\ngit commit:\n{commit}"))
            }
        }
        Ok(DpxCall::Run { command }) => match confirm_run(ask, store, cwd, &command, auto) {
            RunDecision::Blocked(reason) => ToolOutcome::Done(format!(
                "[dpx BLOQUEÓ el comando `{command}`: {reason}. Está prohibido vía run_command; \
                 NO lo vuelvas a proponer, busca otra forma o pídele al usuario que lo haga a mano.]"
            )),
            RunDecision::Refused => {
                println!("{}", ui::dim("omitido."));
                ToolOutcome::Done(format!("[el usuario rechazó ejecutar `{command}`]"))
            }
            RunDecision::Run => {
                let (out, cancelled, _exit) = execute_run(cwd, &command);
                if cancelled { ToolOutcome::Cancelled(out) } else { ToolOutcome::Done(out) }
            }
        },
        Ok(DpxCall::McpTool { name, args }) => {
            println!("{}", ui::accent(&format!("  MCP: {name}")));
            match crate::mcp::McpManager::call_tool(&name, &args) {
                Ok(out) => ToolOutcome::Done(out),
                Err(e) => ToolOutcome::Done(format!("[error MCP {name}: {e}]")),
            }
        },
    }
}

/// Carga (o reutiliza) el motor de embeddings. Centraliza la carga perezosa.
fn ensure_embedder(
    emb: &mut Option<crate::memory::Embedder>,
) -> anyhow::Result<&mut crate::memory::Embedder> {
    if emb.is_none() {
        *emb = Some(crate::memory::Embedder::new()?);
    }
    Ok(emb.as_mut().expect("recién inicializado"))
}




fn build_mentor(
    router: &ModelRouter,
    focus_id: Option<&str>,
    mode: Mode,
    prior: Option<&str>,
) -> Result<Mentor> {
    let mut preamble = focus::system_prompt(focus_id, mode, prior)?;
    // Le damos el árbol del proyecto para que sepa qué archivos puede pedir leer.
    let cwd = env::current_dir().unwrap_or_default();
    preamble.push_str(
        "\n\n# Herramientas: tool calls nativas PRIMERO\n\
         Tienes herramientas NATIVAS (function calling): emite tool calls, no describas \
         acciones en prosa ni uses los bloques dpx:* de texto salvo que el function calling \
         no esté disponible.\n\n\
         # Árbol del proyecto actual\n\
         Estos son los archivos que existen AHORA en el proyecto. Léelos con `read_file` \
         o bórralos con `delete_file` cuando los necesites (no le pidas al usuario que lo haga):\n\n```\n",
    );
    preamble.push_str(&crate::fs::project_tree(&cwd));
    preamble.push_str("```\n");

    // Mapa de símbolos: qué define cada archivo (fn/struct/class/def…), para que
    // el modelo lea SOLO los archivos que necesita en vez de adivinar o leerlos
    // enteros — menos tokens y mejor orientación. Estable por sesión (cacheable).
    let symbols = crate::fs::symbol_map(&cwd);
    if !symbols.trim().is_empty() {
        preamble.push_str(
            "\n# Mapa de símbolos del proyecto\n\
             Qué define cada archivo (índice para leer solo lo necesario, no es exhaustivo):\n\n```\n",
        );
        preamble.push_str(&symbols);
        preamble.push_str("```\n");
    }

    // Grounding: las dependencias/versiones REALES del proyecto, para que no
    // invente crates/starters ni versiones inexistentes.
    if let Some((name, content)) = crate::fs::build_manifest(&cwd) {
        preamble.push_str(&format!(
            "\n# Manifiesto de build (`{name}`)\n\
             Estas son las dependencias y versiones REALES del proyecto. Trátalas como la \
             verdad: NO inventes dependencias, starters ni versiones que no estén aquí. Si hace \
             falta una nueva, dilo de forma explícita y propón añadirla.\n\n```\n{content}\n```\n"
        ));
    }

    router.mentor(&preamble, mode)
}


/// Estimación tosca de los tokens consumidos por el historial (≈ 4 chars/token).
/// Sirve solo para la barra de contexto de `/status`, no es exacta.
fn estimate_tokens(history: &[Message]) -> usize {
    serde_json::to_string(history).map(|s| s.len() / 4).unwrap_or(0)
}

/// Parsea una cantidad de tokens del comando `/budget`: `"50000"`, `"100k"`,
/// `"1.5k"`, `"2m"`. `None` si no es un número válido.
fn parse_token_count(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    let (num, mult) = if let Some(n) = s.strip_suffix('k') {
        (n, 1_000.0)
    } else if let Some(n) = s.strip_suffix('m') {
        (n, 1_000_000.0)
    } else {
        (s.as_str(), 1.0)
    };
    let val: f64 = num.trim().parse().ok()?;
    if !val.is_finite() || val < 0.0 {
        return None;
    }
    Some((val * mult).round() as u64)
}

/// Construye las filas de cerebros para `/status` y `/models`, marcando el activo
/// y consultando si cada uno tiene su API key en el entorno.
fn brain_rows(active: Brain) -> Vec<ui::BrainRow> {
    Brain::all()
        .into_iter()
        .map(|b| ui::BrainRow {
            name: b.name(),
            capability: b.capability(),
            has_key: b.has_key(),
            active: b.name() == active.name(),
        })
        .collect()
}

/// Cierre limpio: genera y persiste el contexto del proyecto.
/// Si hay un plan pendiente guardado y la memoria se está retomando, lo
/// muestra como checklist y lo añade al contexto que se inyecta al modelo,
/// con la instrucción de continuarlo.
fn resume_plan(store: &ProjectStore, prior: Option<String>) -> Option<String> {
    let prior = prior?;
    let Some(plan_md) = store.read_plan() else {
        return Some(prior);
    };
    if let Some(items) = crate::fs::parse_plan(&plan_md) {
        ui::checklist(&items);
        println!("  {}", ui::dim("plan pendiente de la sesión anterior · lo retomamos"));
    }
    Some(format!(
        "{prior}\n\n{plan_md}\nEste plan quedó pendiente de la sesión anterior: re-emítelo \
         (actualizado) con un bloque dpx:plan en tu primera respuesta y continúa desde la \
         primera tarea sin hacer."
    ))
}

/// Cierre del ciclo del plan: si la sesión emitió un `dpx:plan` con tareas
/// pendientes, se guarda en `.dpx/plan.md` para retomarlo; si quedó completo,
/// se limpia. Si la sesión no emitió plan, se conserva el que ya hubiera.
fn persist_plan(store: &ProjectStore, turns: &[Turn]) {
    match crate::fs::extract_last_plan(turns) {
        Some(plan) if plan.iter().any(|(done, _)| !done) => {
            let pending = plan.iter().filter(|(done, _)| !done).count();
            match store.write_plan(&crate::fs::plan_to_markdown(&plan)) {
                Ok(()) => println!(
                    "{}",
                    ui::dim(&format!(
                        "plan pendiente guardado en .dpx/plan.md · {pending} tareas por hacer"
                    ))
                ),
                Err(e) => eprintln!("{} no pude guardar el plan: {e}", ui::accent("⏺")),
            }
        }
        Some(_) => {
            // Plan completado: ciclo cerrado, fuera el archivo.
            if let Err(e) = store.remove_plan() {
                eprintln!("{} no pude limpiar el plan: {e}", ui::accent("⏺"));
            }
        }
        None => {}
    }
}

async fn close_session(
    router: &ModelRouter,
    store: &ProjectStore,
    turns: &[Turn],
    prior: Option<&str>,
    mem: &mut crate::memory::MemoryStore,
    emb: &mut Option<crate::memory::Embedder>,
) {
    // Hooks OnSessionEnd: se ejecutan antes del resumen, incluso si la sesión
    // está vacía (un hook puede querer correr igual).
    {
        let hooks = store.load_hooks();
        let cwd = store.project_dir();
        crate::cli::hooks::run_hooks(&hooks, &crate::cli::hooks::HookEvent::OnSessionEnd, None, cwd);
    }

    if turns.is_empty() {
        println!("\n{}", ui::dim("sesión vacía: no hay nada que recordar. Hasta luego."));
        return;
    }

    // El plan se persiste aparte del resumen (y antes: si el modelo del
    // resumen falla, el plan no se pierde).
    persist_plan(store, turns);

    let spinner = ui::Spinner::start("Guardando memoria…");
    let summary = session::summarize(router, turns, prior).await;
    spinner.stop();

    match summary {
        Ok(md) => match store.write_context(&md) {
            Ok(()) => {
                println!(
                    "{} contexto guardado en .dpx/context.md · la próxima vez retomo desde aquí.",
                    ui::accent("⏺")
                );
                // Auto-ingesta: el resumen de esta sesión entra en la memoria de
                // largo plazo, para recordarlo por SEMÁNTICA en sesiones futuras
                // (no solo lo que el usuario marcó con /recordar). Best-effort:
                // si el motor de embeddings no carga, no pasa nada.
                if remember(&md, mem, emb, "resumen").is_ok() {
                    println!("{}", ui::dim("⎿ sesión añadida a tu memoria de largo plazo"));
                }
            }
            Err(e) => eprintln!("{} no pude escribir el contexto: {e}", ui::accent("⏺")),
        },
        // Si el resumen falla (p.ej. modelo saturado), no perdemos la sesión:
        // guardamos la transcripción cruda como respaldo.
        Err(e) => {
            eprintln!("{} no pude generar el resumen: {}", ui::dim("⏺ aviso"), ui::friendly_error(&e.to_string()));
            let raw = session::fallback_context(turns, prior);
            match store.write_context(&raw) {
                Ok(()) => println!("{} guardé la transcripción cruda como respaldo.", ui::accent("⏺")),
                Err(e2) => eprintln!("{} tampoco pude guardar el respaldo: {e2}", ui::accent("⏺")),
            }
        }
    }
}


/// Lee las referencias `@archivo` del mensaje y construye el contexto que se
/// envía al modelo (el mensaje original con los archivos adjuntos antes).
fn attach_files(cwd: &Path, input: &str) -> String {
    let refs = extract_refs(input);
    if refs.is_empty() {
        return input.to_string();
    }
    let mut context = String::new();
    for r in &refs {
        match crate::fs::read_file(cwd, r) {
            Ok(content) => {
                println!(
                    "{}",
                    ui::dim(&format!("  ⎁ leído @{r} ({} líneas)", content.lines().count()))
                );
                context.push_str(&format!(
                    "\n--- Contenido de `{r}` ---\n{content}\n--- fin de `{r}` ---\n"
                ));
            }
            Err(e) => println!("{}", ui::dim(&format!("  ⚠ no pude leer @{r}: {e}"))),
        }
    }
    if context.is_empty() {
        input.to_string()
    } else {
        format!("Archivos adjuntos por el usuario para que los tengas en cuenta:\n{context}\n\n{input}")
    }
}

/// Extrae las rutas referenciadas con `@` en el mensaje.
fn extract_refs(input: &str) -> Vec<String> {
    input
        .split_whitespace()
        .filter_map(|t| t.strip_prefix('@'))
        .map(|p| {
            p.trim_end_matches(['.', ',', ';', ':', ')', '?', '!'])
                .to_string()
        })
        .filter(|p| !p.is_empty())
        .collect()
}


#[cfg(test)]
mod tests;
