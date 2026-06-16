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

                // /skills: las skills (playbooks) que dpx ha aprendido y refinado
                // en este proyecto, con sus usos y confianza.
                if single_line
                    && matches!(input, "/habilidades" | "/skills" | "/skill")
                {
                    ui::skills_list(&skillbook.ranked());
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
                    println!("{}", ui::dim("⎿ aplicando playbook del proyecto…"));
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

        // Verificación de build automática: si escribimos código fuente y hay un
        // proyecto Maven/Gradle/Cargo, dpx propone compilar y realimenta los
        // errores al modelo para que itere (escribe → compila → corrige), sin que
        // tenga que pedirlo. Se omite si el modelo ya pidió el mismo build tool.
        let mut auto_built = false;
        let mut auto_tested = false;
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
            println!(
                "{}",
                ui::dim("⏸ turno detenido · el plan y la memoria quedan guardados para retomar")
            );
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
            for cmd in &runs {
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
                let (out_text, cancelled) = execute_run(cwd, cmd);
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
                let (out, cancelled) = execute_run(cwd, &command);
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

/// Construye el prompt sintético de `/quiz`: instruye al tutor a hacer un repaso
/// de recuperación (retrieval practice) sobre lo que el usuario ya vio. Si se da
/// un tema, se centra en él; si no, usa las habilidades registradas; y si no hay
/// ninguna, repasa lo básico del temario del stack.
fn build_quiz_prompt(store: &ProjectStore, focus_id: Option<&str>, topic: &str) -> String {
    let base = "Ponme a prueba con un QUIZ de repaso (retrieval practice): UNA pregunta a la \
                vez, espera mi respuesta, y dame feedback corto antes de la siguiente. No me des \
                las respuestas de entrada. Empieza ya con la primera pregunta.";
    if !topic.is_empty() {
        return format!("Hazme un quiz sobre «{topic}». {base}");
    }
    let skills = store.read_skills();
    if !skills.is_empty() {
        let temas: Vec<String> = skills
            .iter()
            .filter(|s| s.level != crate::skill::SkillLevel::Visto)
            .chain(skills.iter())
            .map(|s| s.topic.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        return format!(
            "Hazme un quiz sobre estos temas que ya hemos trabajado: {}. {base}",
            temas.join(", ")
        );
    }
    let topics = crate::focus::curriculum::topics(focus_id);
    if let Some(first) = topics.first() {
        format!("Aún no hemos registrado temas. Hazme un quiz introductorio sobre «{}». {base}", first.name)
    } else {
        format!("Hazme un quiz sobre los fundamentos de lo que estoy aprendiendo. {base}")
    }
}

/// Guarda un recuerdo en la memoria de largo plazo: embebe el texto y lo
/// persiste. Carga el motor de embeddings perezosamente la primera vez.
fn remember(
    text: &str,
    mem: &mut crate::memory::MemoryStore,
    emb: &mut Option<crate::memory::Embedder>,
    kind: &str,
) -> anyhow::Result<()> {
    let engine = ensure_embedder(emb)?;
    let vector = engine.embed_one(text)?;
    mem.add(crate::memory::MemoryEntry {
        text: text.to_string(),
        vector,
        kind: kind.to_string(),
        created: crate::skill::today(),
    })
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

/// Recupera de la memoria de largo plazo los fragmentos relevantes a `input` y
/// los formatea como un bloque de contexto para anteponer al turno. Devuelve
/// `None` si no hay memoria, si el motor no carga, o si nada supera el umbral —
/// en todos esos casos el turno sigue normal (degradación elegante, sin ruido).
fn recall_context(
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
fn recall_skills(
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
fn classify_delegation(input: &str) -> Option<&'static str> {
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
async fn maybe_auto_delegate(input: &str, cwd: &Path) -> Option<String> {
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
async fn run_subagent(cwd: &Path, task: &str, role: Option<&str>) -> String {
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
fn subagent_preamble(cwd: &Path, task: &str, role: crate::agent::roles::AgentRole) -> String {
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
async fn subagent_tool(cwd: &Path, call: &rig_core::message::ToolCall) -> String {
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

/// Lanza el comité de hack desde el REPL: banner, 4 roles + síntesis,
/// y muestra el resultado (que incluye el bloque dpx:plan).
async fn run_comite_command(
    cwd: &Path,
    idea: &str,
    skin: &termimad::MadSkin,
    store: &ProjectStore,
    turns: &mut Vec<Turn>,
) {
    let idea = idea.trim();
    if idea.is_empty() {
        println!(
            "{} {}",
            ui::dim("uso:"),
            ui::dim("/comité <descripción de la idea>")
        );
        return;
    }
    println!(
        "\n{} {}",
        ui::accent("⏺ comité de hack"),
        ui::dim(&format!("· evaluando: {idea}"))
    );
    println!("{}", ui::dim("  consultando 4 roles, uno por uno…"));
    let synthesis = run_committee(cwd, idea).await;
    println!();
    ui::print_markdown(skin, "veredicto del comité", &synthesis);
    let _ = store.checkpoint("user", &format!("/comite {idea}"));
    turns.push(Turn {
        role: "user",
        text: format!("/comite {idea}"),
    });
    let _ = store.checkpoint("assistant", &synthesis);
    turns.push(Turn {
        role: "assistant",
        text: synthesis.clone(),
    });
    // Persistir la síntesis para que el panel de hack la muestre.
    let _ = store.write_committee(&synthesis);

    // Handoff: la fase de diseño terminó, ahora toca construir.
    println!();
    println!(
        "{} {}",
        ui::accent("⏺"),
        ui::accent("plan listo · ahora vete a /code y lo construyo allá")
    );
    println!(
        "{}",
        ui::dim("  en /code tengo acceso completo: escribo, ejecuto, pruebo y arreglo hasta que funcione.")
    );
}

/// Lanza el COMITÉ DE HACK: 4 subagentes secuenciales — juez, product,
/// tech lead, usuario escéptico — cada uno evalúa la idea desde su rol.
/// Luego un subagente de síntesis produce un veredicto + plan dpx:plan.
/// Devuelve el texto de la síntesis (que incluye el bloque dpx:plan).
async fn run_committee(cwd: &Path, idea: &str) -> String {
    let roles = crate::focus::committee::roles();
    let mut contributions: Vec<(String, String)> = Vec::new();

    for role in &roles {
        println!("{} · {}", ui::accent("  comité"), ui::dim(role.label));
        let task = crate::focus::committee::role_task(role, idea);
        let contrib = run_subagent(cwd, &task, None).await;
        contributions.push((role.label.to_string(), contrib));
    }

    println!(
        "{comite} · {synth}",
        comite = ui::accent("  comité"),
        synth = ui::dim("sintetizando aportes…")
    );
    let synthesis_task = crate::focus::committee::synthesis_prompt(&contributions, idea);
    run_subagent(cwd, &synthesis_task, None).await
}

/// Ejecuta `git` con los args dados (cada uno SIN partir por espacios — clave
/// para que un mensaje de commit con espacios viaje como un solo argumento) en
/// `cwd` y devuelve su salida (stdout + stderr si falla). Para las
/// herramientas nativas git_*.
fn run_git(cwd: &Path, args: &[&str]) -> String {
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(cwd);
    cmd.args(args);
    match cmd.output() {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).to_string();
            if !out.status.success() {
                let err = String::from_utf8_lossy(&out.stderr);
                text.push_str(&format!("\n[git salió con {}: {}]", out.status, err.trim()));
            } else if text.trim().is_empty() {
                text = "(sin salida)".to_string();
            }
            text
        }
        Err(e) => format!("[error al ejecutar git {}: {}]", args.join(" "), e),
    }
}

/// Ejecuta un comando YA confirmado: salida en vivo (acotada en pantalla; el
/// modelo recibe todo), timeout, cancelación por Ctrl-C y diagnóstico
/// automático de fallos. Devuelve (texto para el modelo, ¿se canceló?).
fn execute_run(cwd: &Path, cmd: &str) -> (String, bool) {
    let t = std::time::Instant::now();
    // La pestaña muestra el comando en curso (acotado para que quepa).
    let shown_cmd: String = cmd.chars().take(48).collect();
    ui::title_busy(&shown_cmd);
    ui::clear_cancel();
    let mut shown = 0usize;
    let out = crate::fs::run_command_streaming(
        cwd,
        cmd,
        crate::fs::RUN_TIMEOUT_SECS,
        &mut |line| {
            shown += 1;
            if shown <= 30 {
                println!("{}", ui::dim(&format!("  │ {line}")));
            } else if shown == 31 {
                println!("{}", ui::dim("  │ … (el resto va al modelo)"));
            }
        },
        &|| ui::cancel_requested(),
    );
    ui::action_time(t.elapsed());
    let mut text = out.output.clone();
    if !out.cancelled
        && (text.contains("exit code: 1")
            || text.contains("BUILD FAILURE")
            || text.contains("ERROR"))
        && let Some(diag) = crate::agent::diagnostic::diagnose_failure(&text)
    {
        ui::diagnostic_panel(&diag.hint, &diag.suggestions);
        text.push_str(&format!(
            "\n[DIAGNÓSTICO AUTOMÁTICO DPX]: {}\nSugerencias: {:?}\n",
            diag.hint, diag.suggestions
        ));
    }
    (text, out.cancelled)
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

/// Despacha un comando del REPL. Muta el estado de la sesión cuando aplica.
#[allow(clippy::too_many_arguments)]
fn handle_command(
    cmd: &str,
    skin: &termimad::MadSkin,
    store: &ProjectStore,
    prior: &Option<String>,
    router: &mut ModelRouter,
    mentor: &mut Mentor,
    focus_id: &mut Option<String>,
    mode: &mut Mode,
    brain: &mut Brain,
    auto: &mut crate::cli::AutoMode,
    history: &mut Vec<Message>,
    cwd: &Path,
    turn_count: usize,
) {
    let mut parts = cmd.splitn(2, char::is_whitespace);
    // Normaliza el comando a su clave canónica: los nombres son en ESPAÑOL
    // (`/ayuda`, `/enfoque`, `/cerebro`…) y los ingleses quedan como alias.
    let name = canonical_cmd(parts.next().unwrap_or(""));
    let arg = parts.next().map(str::trim).filter(|s| !s.is_empty());

    // Filtro por modo: los comandos específicos de un modo (p.ej. /auto en
    // code/hack, /progreso en learn) se rechazan con una pista en los demás.
    if !command_in_mode(name, *mode) {
        reject_command(name, *mode);
        return;
    }

    match name {
        "help" | "?" | "" => ui::print_help(*mode),

        "status" => ui::status_panel(
            env!("CARGO_PKG_VERSION"),
            &short_path(cwd),
            focus::display_name(focus_id.as_deref()),
            mode_label(*mode),
            router.brain_label(),
            &brain_rows(*brain),
            turn_count,
            prior.is_some(),
            estimate_tokens(history),
            brain.context_budget(),
        ),

        "models" | "model" => ui::models_list(&brain_rows(*brain)),

        // Modo autónomo: cambios y comandos SEGUROS sin preguntar. Las puertas
        // duras (peligrosos, prohibidos, guards anti-truncado, borrados) se
        // mantienen SIEMPRE, con o sin auto.
        "auto" => {
            if let Some(a) = arg {
                if let Some(m) = crate::cli::AutoMode::parse(a) {
                    *auto = m;
                } else {
                    println!("{} {}", ui::dim("modo auto desconocido:"), a);
                }
            } else {
                *auto = if *auto == crate::cli::AutoMode::All {
                    crate::cli::AutoMode::Off
                } else {
                    crate::cli::AutoMode::All
                };
            }
            if *auto != crate::cli::AutoMode::Off {
                println!(
                    "{} {}",
                    ui::accent(&format!("⏺ auto ACTIVADO ({})", auto.label())),
                    ui::dim("· usa /auto off para volver a manual")
                );
            } else {
                println!("{}", ui::accent("⏺ auto desactivado · cada cambio vuelve a confirmarse"));
            }
        }

        // dpx se reinstala a sí mismo desde este repo (cierra el ciclo de automejora).
        "update" => self_update(cwd),

        "clear" => {
            history.clear();
            crate::token::reset();
            println!(
                "{}",
                ui::dim("conversación reiniciada · olvida el contexto de esta sesión")
            );
        }

        // Consumo de tokens REAL de la sesión (de la API, no estimado), con el
        // % servido desde el caché de contexto y el costo aproximado.
        "cost" => match crate::token::session_summary() {
            Some(s) => {
                println!("\n{}  {}", ui::accent("⏺ tokens · sesión"), ui::dim(&s));
                println!(
                    "  {}",
                    ui::dim("caché alto = más barato · sube el % manteniendo estable el inicio del prompt")
                );
            }
            None => println!(
                "{}",
                ui::dim("aún no hay consumo registrado (el proveedor no reportó tokens todavía)")
            ),
        },

        // Tope de tokens de la sesión: `/budget 100k`, `/budget off`, o sin arg
        // para ver el estado. Al superarlo, el modo auto se pausa y pregunta.
        "budget" => match arg.map(str::to_ascii_lowercase).as_deref() {
            None => match crate::token::budget_status() {
                Some(s) => println!("{} {}", ui::accent("⏺ presupuesto:"), ui::dim(&s)),
                None => println!(
                    "{}",
                    ui::dim("sin tope de tokens · usa `/budget 100k` para poner uno (`/budget off` lo quita)")
                ),
            },
            Some("off") | Some("0") | Some("none") => {
                crate::token::set_budget(0);
                println!("{}", ui::accent("⏺ presupuesto de tokens desactivado"));
            }
            Some(s) => match parse_token_count(s) {
                Some(n) => {
                    crate::token::set_budget(n);
                    println!(
                        "{} {}",
                        ui::accent("⏺ presupuesto →"),
                        ui::dim(&format!("{n} tok · al superarlo, auto se pausa y pregunta"))
                    );
                }
                None => println!("{} {}", ui::dim("no entendí la cantidad:"), s),
            },
        },

        // Deshace los cambios de archivos del último turno de dpx (restaura lo
        // que existía, borra lo que creó). No toca git ni tus cambios propios.
        "undo" => match crate::checkpoint::undo() {
            Some((restored, deleted)) => {
                println!(
                    "{} {}",
                    ui::accent("⏺ deshecho"),
                    ui::dim(&format!(
                        "{restored} archivo(s) restaurado(s), {deleted} borrado(s) · el modelo aún cree que los hizo: dile qué revertiste"
                    ))
                );
            }
            None => println!(
                "{}",
                ui::dim("nada que deshacer (dpx no ha modificado archivos esta sesión)")
            ),
        },

        // Muestra TODO lo que dpx cambió en la sesión (base vs. actual), para
        // revisar antes de confiar/commitear. Solo lectura.
        "diff" => {
            let changes = crate::checkpoint::session_changes();
            if changes.is_empty() {
                println!(
                    "{}",
                    ui::dim("dpx no ha cambiado archivos esta sesión (o ya los revertiste)")
                );
            } else {
                println!(
                    "\n{} {}",
                    ui::accent("⏺ cambios de la sesión"),
                    ui::dim(&format!("({} archivo(s))", changes.len()))
                );
                for ch in &changes {
                    let rel = ch.path.strip_prefix(cwd).unwrap_or(&ch.path);
                    match &ch.now {
                        None => println!("\n{} {}", ui::red("borrado:"), rel.display()),
                        Some(now) => {
                            println!("\n{} {}", ui::accent("●"), rel.display());
                            ui::preview_diff(ch.before.as_deref(), now);
                        }
                    }
                }
            }
        }

        "panel" => {
            let has_context = store.prior_context().is_some();
            let plan_progress = store.read_plan()
                .as_deref()
                .and_then(crate::fs::parse_plan)
                .map(|items| {
                    let done = items.iter().filter(|(d, _)| *d).count();
                    (done, items.len())
                });
            let skills = store.read_skills();
            let dominados = skills.iter()
                .filter(|s| s.level == crate::skill::SkillLevel::Dominado)
                .count();
            let total_skills = skills.len();
            let recuerdos = crate::memory::MemoryStore::load(store.dpx_dir()).len();
            let skills_dir = store
                .dpx_dir()
                .parent()
                .map(|p| p.join("skills"))
                .unwrap_or_else(|| std::path::PathBuf::from("skills"));
            let agent_skills = crate::agent_skill::SkillBook::from_dir(&skills_dir).len();
            ui::project_panel(has_context, plan_progress, dominados, total_skills, recuerdos, agent_skills);
        }

        "context" => match store.prior_context() {
            Some(c) => ui::print_markdown(skin, "⏺ memoria del proyecto", &c),
            None => println!("{}", ui::dim("aún no hay memoria guardada para este proyecto")),
        },

        "progreso" | "progress" => {
            let skills = store.read_skills();
            ui::learn_panel(&skills, &crate::skill::today());
        }

        "temario" | "curriculum" => {
            let skills = store.read_skills();
            println!(
                "\n{}",
                crate::focus::curriculum::render(focus_id.as_deref(), &skills)
            );
        }

        "focus" => match arg {
            None => {
                println!("{} {}", ui::dim("enfoque actual:"), focus::display_name(focus_id.as_deref()));
                focus::print_catalog();
            }
            Some(f) => match build_mentor(router, Some(f), *mode, prior.as_deref()) {
                Ok(m) => {
                    *mentor = m;
                    *focus_id = Some(f.to_string());
                    println!("{} {}", ui::accent("⏺ enfoque →"), focus::display_name(Some(f)));
                }
                Err(e) => println!("{} {e}", ui::dim("no pude cambiar de enfoque:")),
            },
        },

        // Cambia de modo en caliente. `/mentor` y `/code` son alias históricos:
        // `/mentor` → learn (enseña), `/code` → code (hace).
        "mode" | "mentor" | "code" => {
            let new = match name {
                "mentor" => Some(Mode::Learn),
                "code" => Some(Mode::Code),
                _ => arg.and_then(Mode::parse),
            };
            match new {
                Some(m) if m == *mode => {
                    println!("{} {}", ui::dim("ya estás en modo:"), mode_label(m));
                }
                Some(m) => match build_mentor(router, focus_id.as_deref(), m, prior.as_deref()) {
                    Ok(agent) => {
                        *mentor = agent;
                        *mode = m;
                        // Retiñe la UI al color del nuevo modo y anúncialo.
                        ui::set_mode_theme(m);
                        println!("{} {}", ui::accent("⏺ modo →"), mode_label(m));
                    }
                    Err(e) => println!("{} {e}", ui::dim("error:")),
                },
                None => println!(
                    "{} (actual: {})",
                    ui::dim("uso: /modo code|hack|learn"),
                    mode_label(*mode)
                ),
            }
        }

        "brain" => {
            // Easter egg: `/brain fable` (o mythos) rinde tributo. No son cerebros
            // reales — dpx no usa Anthropic — así que no cambian nada.
            let id = arg.map(|a| a.to_ascii_lowercase());
            if matches!(
                id.as_deref(),
                Some("fable") | Some("fable5") | Some("fable 5") | Some("mythos") | Some("mythos5")
            ) {
                ui::fable_tribute();
            } else {
                match arg.and_then(Brain::parse) {
                    Some(b) => {
                        let new_router = ModelRouter::new(b);
                        match build_mentor(&new_router, focus_id.as_deref(), *mode, prior.as_deref()) {
                            Ok(agent) => {
                                *router = new_router;
                                *mentor = agent;
                                *brain = b;
                                println!("{} {}", ui::accent("⏺ cerebro →"), router.brain_label());
                            }
                            Err(e) => println!("{} {e}", ui::dim("no pude cambiar de cerebro:")),
                        }
                    }
                    None => println!(
                        "{} (actual: {})",
                        ui::dim("dpx usa solo DeepSeek"),
                        router.brain_label()
                    ),
                }
            }
        }

        other => println!(
            "{}",
            ui::dim(&format!("comando desconocido: /{other} — escribe /help"))
        ),
    }
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

/// Resultado de aplicar los cambios propuestos por el modelo en una ronda
/// (escrituras, ediciones, borrados), para realimentárselo en la siguiente.
#[derive(Default)]
struct ActionReport {
    notes: Vec<String>,
    /// Hubo rechazos o fallos: el modelo debe enterarse y reaccionar.
    needs_followup: bool,
}

impl ActionReport {
    fn ok(&mut self, note: String) {
        self.notes.push(note);
    }

    fn followup(&mut self, note: String) {
        self.notes.push(note);
        self.needs_followup = true;
    }

    fn absorb(&mut self, other: ActionReport) {
        self.notes.extend(other.notes);
        self.needs_followup |= other.needs_followup;
    }
}

/// Procesa las propuestas de escritura: por cada archivo muestra un preview
/// compacto y lo escribe SOLO si el usuario confirma. Acepta `a`/`todos` para
/// escribir todos los restantes sin volver a preguntar. `ask` abstrae la
/// pregunta interactiva (rustyline en producción, respuestas fijas en tests).
fn process_writes(
    cwd: &std::path::Path,
    writes: &[crate::fs::FileWrite],
    ask: &mut dyn FnMut(&str) -> Option<String>,
    auto: crate::cli::AutoMode,
) -> ActionReport {
    let mut report = ActionReport::default();
    if writes.is_empty() {
        return report;
    }
    if writes.len() > 1 {
        println!(
            "\n{}",
            ui::dim(&format!("dpx propone escribir {} archivos:", writes.len()))
        );
    }

    let mut write_all = false;
    for w in writes {
        let exists = crate::fs::exists(cwd, &w.path);
        let verbo = if exists { "sobrescribir" } else { "crear" };
        let extra = if exists { " · ya existe" } else { "" };
        println!(
            "\n{} {} {}  {}",
            ui::accent("⏺"),
            verbo,
            ui::accent(&w.path),
            ui::dim(&format!("({} líneas{extra})", w.line_count())),
        );

        // Guard anti-truncado: un write que ENCOGE drásticamente un archivo
        // grande suele ser la salida del modelo cortada a mitad (se quedó sin
        // tokens), no un cambio intencional. Y reescribir COMPLETO un archivo
        // grande (aunque no encoja) viola la doctrina de edit_file y corre el
        // mismo riesgo. Ambos avisan y preguntan SIEMPRE (ni `a=todos` salta).
        let current = crate::fs::current_content(cwd, &w.path);
        let shrink = shrink_warning(current.as_deref(), &w.content);
        let big_rewrite = shrink.is_none() && big_rewrite_warning(current.as_deref()).is_some();
        if let Some((old, new)) = shrink {
            println!(
                "  {}",
                ui::red(&format!(
                    "⚠ posible truncado: el archivo pasaría de {old} a {new} líneas — revisa el final del diff"
                ))
            );
        } else if big_rewrite {
            println!(
                "  {}",
                ui::red("⚠ reescritura completa de un archivo grande — la doctrina dice edit_file para archivos existentes")
            );
        }

        // En modo auto los writes limpios se aplican directo (mostrando el
        // diff igual, por transparencia); los GUARDS siguen preguntando
        // SIEMPRE: un posible truncado no se auto-acepta jamás.
        if auto.writes() && shrink.is_none() && !big_rewrite {
            ui::preview_diff(current.as_deref(), &w.content);
            match crate::fs::apply(cwd, w) {
                Ok(path) => {
                    println!("{} escrito (auto): {}", ui::accent("⏺"), short_path(&path));
                    report.ok(format!("[escrito: {}]", w.path));
                }
                Err(e) => {
                    eprintln!("{} {e}", ui::accent("⏺ error"));
                    report.followup(format!("[ERROR al escribir {}: {e}]", w.path));
                }
            }
            continue;
        }

        let accepted = (write_all && shrink.is_none() && !big_rewrite) || {
            ui::preview_diff(current.as_deref(), &w.content);
            match ask("¿escribir? [s/N/a=todos] ") {
                Some(ans) => {
                    let a = ans.trim().to_lowercase();
                    if a == "a" || a == "todos" {
                        write_all = true;
                        true
                    } else {
                        is_yes(&a)
                    }
                }
                None => false,
            }
        };

        if accepted {
            match crate::fs::apply(cwd, w) {
                Ok(path) => {
                    println!("{} escrito: {}", ui::accent("⏺"), short_path(&path));
                    report.ok(format!("[escrito: {}]", w.path));
                }
                Err(e) => {
                    eprintln!("{} {e}", ui::accent("⏺ error"));
                    report.followup(format!("[ERROR al escribir {}: {e}]", w.path));
                }
            }
        } else if let Some((old, new)) = shrink {
            println!("{}", ui::dim("omitido."));
            report.followup(format!(
                "[el usuario rechazó escribir {} — AVISO de dpx: tu contenido reduciría el \
                 archivo de {old} a {new} líneas; casi seguro tu salida quedó TRUNCADA. NO \
                 reescribas archivos grandes enteros: aplica cambios pequeños con edit_file/dpx:edit.]",
                w.path
            ));
        } else if big_rewrite {
            println!("{}", ui::dim("omitido."));
            report.followup(format!(
                "[el usuario rechazó escribir {} — AVISO de dpx: propusiste reescribir COMPLETO \
                 un archivo existente grande; la regla es edit_file/dpx:edit para archivos que \
                 ya existen. Re-propón el cambio como edits quirúrgicos pequeños.]",
                w.path
            ));
        } else {
            println!("{}", ui::dim("omitido."));
            report.followup(format!("[el usuario rechazó escribir {}]", w.path));
        }
    }
    report
}

/// Detecta un write sospechoso de venir truncado: el archivo existe, es
/// grande, y el contenido nuevo lo encoge más de un 40%. Devuelve
/// `(líneas actuales, líneas propuestas)` cuando hay que avisar.
fn shrink_warning(current: Option<&str>, new_content: &str) -> Option<(usize, usize)> {
    /// Por debajo de esto un encogimiento es creíble (refactor, limpieza).
    const MIN_LINES: usize = 80;
    let old = current?.lines().count();
    let new = new_content.lines().count();
    (old >= MIN_LINES && new * 10 < old * 6).then_some((old, new))
}

/// Detecta la reescritura completa de un archivo existente grande (la
/// doctrina dice `edit_file`): mismo riesgo de truncado, aunque no encoja.
fn big_rewrite_warning(current: Option<&str>) -> Option<usize> {
    /// El umbral de la doctrina de AGENTIC_SKILLS (~200 líneas).
    const MIN_LINES: usize = 200;
    let old = current?.lines().count();
    (old >= MIN_LINES).then_some(old)
}

/// Procesa las ediciones quirúrgicas (`dpx:edit`): localiza el bloque SEARCH de
/// forma literal, muestra el diff +/- contra el archivo actual y aplica SOLO si
/// el usuario confirma. Si el bloque no aparece, error claro y no se toca nada.
fn process_edits(
    cwd: &std::path::Path,
    edits: &[crate::fs::FileEdit],
    ask: &mut dyn FnMut(&str) -> Option<String>,
    auto: crate::cli::AutoMode,
) -> ActionReport {
    let mut report = ActionReport::default();
    for e in edits {
        println!("\n{} editar {}", ui::accent("⏺"), ui::accent(&e.path));
        let Some(current) = crate::fs::current_content(cwd, &e.path) else {
            eprintln!(
                "{} no existe `{}` (para crear archivos es dpx:write)",
                ui::accent("⏺ error"),
                e.path
            );
            report.followup(format!(
                "[ERROR en dpx:edit: no existe `{}` — para crear archivos usa dpx:write]",
                e.path
            ));
            continue;
        };
        match crate::fs::apply_edit(&current, e) {
            Ok(new_content) => {
                ui::preview_diff(Some(&current), &new_content);
                // En auto, las ediciones quirúrgicas (pequeñas por diseño) se
                // aplican directo tras mostrar el diff.
                let accepted =
                    auto.writes() || matches!(ask("¿aplicar? [s/N] "), Some(a) if is_yes(&a));
                if accepted {
                    let w = crate::fs::FileWrite { path: e.path.clone(), content: new_content };
                    match crate::fs::apply(cwd, &w) {
                        Ok(path) => {
                            println!("{} editado: {}", ui::accent("⏺"), short_path(&path));
                            report.ok(format!("[edición aplicada: {}]", e.path));
                        }
                        Err(err) => {
                            eprintln!("{} {err}", ui::accent("⏺ error"));
                            report.followup(format!("[ERROR al editar {}: {err}]", e.path));
                        }
                    }
                } else {
                    println!("{}", ui::dim("omitido."));
                    report.followup(format!("[el usuario rechazó la edición de {}]", e.path));
                }
            }
            Err(err) => {
                eprintln!("{} {err}", ui::accent("⏺ error"));
                report.followup(format!(
                    "[ERROR en dpx:edit {}: {err}. Lee el archivo con dpx:read y reintenta con el texto EXACTO]",
                    e.path
                ));
            }
        }
    }
    report
}

/// Procesa los borrados (`dpx:delete`), cada uno bajo confirmación.
fn process_deletes(
    cwd: &std::path::Path,
    deletes: &[String],
    ask: &mut dyn FnMut(&str) -> Option<String>,
) -> ActionReport {
    let mut report = ActionReport::default();
    for d in deletes {
        println!("\n{} borrar {}", ui::accent("⏺"), ui::accent(d));
        if matches!(ask("¿borrar? [s/N] "), Some(a) if is_yes(&a)) {
            match crate::fs::delete_file(cwd, d) {
                Ok(()) => {
                    println!("{} borrado: {}", ui::accent("⏺"), d);
                    report.ok(format!("[borrado: {d}]"));
                }
                Err(e) => {
                    eprintln!("{} error al borrar: {e}", ui::accent("⏺"));
                    report.followup(format!("[ERROR al borrar {d}: {e}]"));
                }
            }
        } else {
            println!("{}", ui::dim("omitido."));
            report.followup(format!("[el usuario rechazó borrar {d}]"));
        }
    }
    report
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

/// Qué se decidió sobre un `dpx:run` propuesto, para informar al modelo con
/// precisión (no es lo mismo "el usuario no quiso" que "dpx lo prohibió").
enum RunDecision {
    Run,
    Refused,
    Blocked(&'static str),
}

/// Muestra el comando propuesto y pide confirmación antes de ejecutarlo,
/// según su nivel de riesgo (`fs::safety`):
/// - prohibido → se bloquea sin preguntar;
/// - peligroso → panel rojo + hay que reescribir la primera palabra del
///   comando (la allowlist NO aplica y no se ofrece `a=siempre`);
/// - seguro → flujo normal: `a`/`siempre` lo ejecuta Y lo guarda en la
///   allowlist del proyecto (`.dpx/allowed_commands`).
fn confirm_run(
    ask: &mut dyn FnMut(&str) -> Option<String>,
    store: &ProjectStore,
    cwd: &Path,
    cmd: &str,
    auto: crate::cli::AutoMode,
) -> RunDecision {
    use crate::fs::safety::{self, CommandRisk};

    match safety::assess_command(cmd) {
        CommandRisk::Forbidden { reason } => {
            ui::danger_panel(
                "✗ comando bloqueado por dpx",
                &format!("{cmd}\n\n{reason}. dpx nunca ejecuta esto; si de verdad hace falta, hazlo tú a mano fuera de dpx."),
            );
            return RunDecision::Blocked(reason);
        }
        CommandRisk::Dangerous { reason } => {
            // El peligro manda: ni la allowlist ni `a=siempre` aplican aquí.
            let keyword = cmd.split_whitespace().next().unwrap_or("si").to_string();
            ui::danger_panel(
                "⚠ comando peligroso",
                &format!("{cmd}\n\n{reason}.\nPara ejecutarlo, escribe: {keyword}"),
            );
            warn_outside_paths(cwd, cmd);
            return match ask("¿confirmar? ") {
                Some(ans) if ans.trim().eq_ignore_ascii_case(&keyword) => RunDecision::Run,
                _ => RunDecision::Refused,
            };
        }
        CommandRisk::Safe => {}
    }

    if store.is_command_allowed(cmd) {
        println!(
            "\n{} {}  {}",
            ui::grad("▸▸ ejecutar"),
            ui::accent(cmd),
            ui::dim("(permitido siempre)")
        );
        return RunDecision::Run;
    }
    // Modo auto: un comando clasificado como SEGURO corre sin preguntar (los
    // peligrosos/prohibidos ya retornaron arriba con sus puertas intactas).
    if auto.commands() {
        println!("\n{} {}  {}", ui::grad("▸▸ ejecutar"), ui::accent(cmd), ui::dim("(auto)"));
        warn_outside_paths(cwd, cmd);
        return RunDecision::Run;
    }
    println!("\n{} {}", ui::grad("▸▸ ejecutar"), ui::accent(cmd));
    warn_outside_paths(cwd, cmd);
    match ask("¿ejecutar? [s/N/a=siempre] ") {
        Some(ans) => {
            let a = ans.trim().to_lowercase();
            if a == "a" || a == "siempre" {
                match store.allow_command(cmd) {
                    Ok(()) => println!(
                        "{}",
                        ui::dim("guardado en .dpx/allowed_commands · no volveré a preguntar por este comando")
                    ),
                    Err(e) => eprintln!("{} {e}", ui::dim("no pude guardar el permiso:")),
                }
                RunDecision::Run
            } else if is_yes(&a) {
                RunDecision::Run
            } else {
                RunDecision::Refused
            }
        }
        None => RunDecision::Refused,
    }
}

/// Aviso (no bloqueo) cuando el comando menciona rutas absolutas fuera del
/// proyecto: que se confirme sabiendo que sale del territorio.
fn warn_outside_paths(cwd: &Path, cmd: &str) {
    let outside = crate::fs::safety::outside_project_paths(cmd, cwd);
    if !outside.is_empty() {
        println!(
            "  {}",
            ui::dim(&format!("⚠ toca rutas fuera del proyecto: {}", outside.join(", ")))
        );
    }
}

fn is_yes(answer: &str) -> bool {
    matches!(
        answer.trim().to_lowercase().as_str(),
        "s" | "si" | "sí" | "y" | "yes"
    )
}

/// Mapea cualquier alias de comando (español o inglés) a su clave canónica
/// interna. Los nombres OFICIALES son en español; el inglés queda como alias
/// oculto (no rompe la memoria muscular de quien ya usaba `/help`, `/focus`…).
fn canonical_cmd(name: &str) -> &str {
    match name {
        "ayuda" => "help",
        "estado" => "status",
        "costo" | "coste" => "cost",
        "presupuesto" => "budget",
        "modelos" => "models",
        "cambios" => "diff",
        "deshacer" => "undo",
        "limpiar" => "clear",
        "compactar" => "compact",
        "contexto" => "context",
        "enfoque" => "focus",
        "modo" => "mode",
        "examen" => "quiz",
        "habilidades" => "skills",
        "cerebro" => "brain",
        "comité" => "comite",
        "actualizar" => "update",
        other => other,
    }
}

/// ¿Está disponible el comando `name` (clave canónica) en el modo activo? Los
/// comandos no listados aquí son GLOBALES (los tres modos los tienen). El
/// reparto: `auto` solo en code/hack (learn no escribe), `comite`/`brainstorm`
/// solo en hack (lluvia de ideas), `quiz`/`progreso`/`temario` solo en learn.
fn command_in_mode(name: &str, mode: Mode) -> bool {
    match name {
        "auto" => matches!(mode, Mode::Code | Mode::Hack),
        "comite" | "brainstorm" => mode == Mode::Hack,
        "quiz" | "progreso" | "temario" => mode == Mode::Learn,
        _ => true,
    }
}

/// Avisa (con una pista) que un comando no pertenece al modo activo.
fn reject_command(name: &str, mode: Mode) {
    let needed = match name {
        "auto" => "code o hack",
        "comite" | "brainstorm" => "hack",
        "quiz" | "progreso" | "temario" => "learn",
        _ => "otro",
    };
    println!(
        "{}",
        ui::dim(&format!(
            "/{name} no está en modo {} · es de modo {needed} — cambia con /modo {}",
            mode.name(),
            needed.split(' ').next().unwrap_or(needed)
        ))
    );
}

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Code => "code (agente autónomo)",
        Mode::Hack => "hack (construir rápido con criterio)",
        Mode::Learn => "learn (tutor socrático)",
    }
}

/// Versión compacta para la barra de estado (sin paréntesis largos).
fn mode_label_compact(mode: Mode) -> &'static str {
    match mode {
        Mode::Code => "code",
        Mode::Hack => "hack",
        Mode::Learn => "learn",
    }
}

/// Acorta el path del proyecto usando `~` para el home (solo estético).
fn short_path(cwd: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir()
        && let Ok(rest) = cwd.strip_prefix(&home) {
            // Normalizamos a '/' para que se vea consistente en Windows.
            return format!("~/{}", rest.display().to_string().replace('\\', "/"));
        }
    cwd.display().to_string().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::fs::{FileEdit, FileWrite};

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dpx-chat-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn auto_delegacion_solo_en_investigacion() {
        // Investigación → delega a researcher (ahorra: lo hace el flash barato).
        assert_eq!(classify_delegation("¿dónde se valida el token de sesión?"), Some("researcher"));
        assert_eq!(classify_delegation("explica cómo funciona el editor de entrada"), Some("researcher"));
        assert_eq!(classify_delegation("busca todos los usos de run_turn en el proyecto"), Some("researcher"));
        // Cambios → NO delega (lo hace el agente principal, que escribe).
        assert_eq!(classify_delegation("crea un endpoint nuevo para usuarios"), None);
        assert_eq!(classify_delegation("arregla el bug del muro de reglas"), None);
        // Trivial/corto → no compensa el overhead.
        assert_eq!(classify_delegation("hola"), None);
        assert_eq!(classify_delegation("gracias crack"), None);
    }

    #[test]
    fn comandos_filtrados_por_modo() {
        // /auto en code/hack, no en learn.
        assert!(command_in_mode("auto", Mode::Code));
        assert!(command_in_mode("auto", Mode::Hack));
        assert!(!command_in_mode("auto", Mode::Learn));
        // comité solo en hack; quiz/progreso/temario solo en learn.
        assert!(command_in_mode("comite", Mode::Hack));
        assert!(!command_in_mode("comite", Mode::Code));
        assert!(command_in_mode("quiz", Mode::Learn));
        assert!(!command_in_mode("quiz", Mode::Code));
        // Globales: disponibles en los tres.
        for m in [Mode::Code, Mode::Hack, Mode::Learn] {
            assert!(command_in_mode("status", m));
            assert!(command_in_mode("focus", m));
        }
    }

    #[test]
    fn parse_token_count_acepta_sufijos() {
        assert_eq!(parse_token_count("50000"), Some(50_000));
        assert_eq!(parse_token_count("100k"), Some(100_000));
        assert_eq!(parse_token_count("1.5k"), Some(1_500));
        assert_eq!(parse_token_count("2m"), Some(2_000_000));
        assert_eq!(parse_token_count("  80K "), Some(80_000));
        assert_eq!(parse_token_count("abc"), None);
        assert_eq!(parse_token_count(""), None);
    }

    #[test]
    fn write_confirmado_se_aplica_y_reporta() {
        let dir = tmp("w-ok");
        let writes = vec![FileWrite { path: "a.txt".into(), content: "hola\n".into() }];
        let report = process_writes(&dir, &writes, &mut |_| Some("s".into()), AutoMode::Off);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "hola\n");
        assert!(!report.needs_followup);
        assert!(report.notes[0].contains("escrito"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_rechazado_pide_followup() {
        let dir = tmp("w-no");
        let writes = vec![FileWrite { path: "a.txt".into(), content: "hola\n".into() }];
        let report = process_writes(&dir, &writes, &mut |_| Some("n".into()), AutoMode::Off);
        assert!(!dir.join("a.txt").exists());
        assert!(report.needs_followup);
        assert!(report.notes[0].contains("rechazó"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_all_pregunta_una_sola_vez() {
        let dir = tmp("w-all");
        let writes = vec![
            FileWrite { path: "a.txt".into(), content: "A\n".into() },
            FileWrite { path: "b.txt".into(), content: "B\n".into() },
        ];
        let mut asked = 0;
        let report = process_writes(
            &dir,
            &writes,
            &mut |_| {
                asked += 1;
                Some("a".into())
            },
            AutoMode::Off,
        );
        assert_eq!(asked, 1);
        assert!(dir.join("a.txt").exists() && dir.join("b.txt").exists());
        assert!(!report.needs_followup);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn edit_confirmado_modifica_el_archivo() {
        let dir = tmp("e-ok");
        std::fs::write(dir.join("x.txt"), "uno\ndos\ntres\n").unwrap();
        let edits = vec![FileEdit { path: "x.txt".into(), search: "dos".into(), replace: "DOS".into() }];
        let report = process_edits(&dir, &edits, &mut |_| Some("s".into()), AutoMode::Off);
        assert_eq!(std::fs::read_to_string(dir.join("x.txt")).unwrap(), "uno\nDOS\ntres\n");
        assert!(!report.needs_followup);
        assert!(report.notes[0].contains("aplicada"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn edit_sin_match_reporta_error_sin_preguntar() {
        let dir = tmp("e-bad");
        std::fs::write(dir.join("x.txt"), "contenido\n").unwrap();
        let edits = vec![FileEdit { path: "x.txt".into(), search: "no-existe".into(), replace: "y".into() }];
        let mut asked = 0;
        let report = process_edits(
            &dir,
            &edits,
            &mut |_| {
                asked += 1;
                Some("s".into())
            },
            AutoMode::Off,
        );
        assert_eq!(asked, 0);
        assert!(report.needs_followup);
        assert!(report.notes[0].contains("ERROR"));
        assert_eq!(std::fs::read_to_string(dir.join("x.txt")).unwrap(), "contenido\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn edit_sobre_archivo_inexistente_reporta_error() {
        let dir = tmp("e-nofile");
        let edits = vec![FileEdit { path: "nada.txt".into(), search: "x".into(), replace: "y".into() }];
        let report = process_edits(&dir, &edits, &mut |_| Some("s".into()), AutoMode::Off);
        assert!(report.needs_followup);
        assert!(report.notes[0].contains("dpx:write"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn delete_rechazado_no_borra_y_reporta() {
        let dir = tmp("d-no");
        std::fs::write(dir.join("x.txt"), "x").unwrap();
        let report = process_deletes(&dir, &["x.txt".to_string()], &mut |_| Some("n".into()));
        assert!(dir.join("x.txt").exists());
        assert!(report.needs_followup);
        assert!(report.notes[0].contains("rechazó"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn delete_confirmado_borra() {
        let dir = tmp("d-ok");
        std::fs::write(dir.join("x.txt"), "x").unwrap();
        let report = process_deletes(&dir, &["x.txt".to_string()], &mut |_| Some("s".into()));
        assert!(!dir.join("x.txt").exists());
        assert!(!report.needs_followup);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn report_absorb_acumula_y_propaga_followup() {
        let mut a = ActionReport::default();
        a.ok("[escrito: a]".into());
        let mut b = ActionReport::default();
        b.followup("[ERROR en b]".into());
        a.absorb(b);
        assert_eq!(a.notes.len(), 2);
        assert!(a.needs_followup);
    }

    #[test]
    fn rebuild_history_resume_y_conserva_recientes() {
        let mut history: Vec<Message> =
            (0..10).map(|i| Message::user(format!("mensaje {i}"))).collect();
        rebuild_history(&mut history, "resumen de prueba");
        assert_eq!(history.len(), 2 + KEEP_RECENT_MESSAGES);
        let first = serde_json::to_string(&history[0]).unwrap();
        assert!(first.contains("CONTEXTO COMPACTADO"));
        assert!(first.contains("resumen de prueba"));
        let last = serde_json::to_string(history.last().unwrap()).unwrap();
        assert!(last.contains("mensaje 9"));
    }

    #[test]
    fn rebuild_history_con_pocos_mensajes_no_pierde_nada() {
        let mut history: Vec<Message> = vec![Message::user("único".to_string())];
        rebuild_history(&mut history, "r");
        assert_eq!(history.len(), 3);
        let last = serde_json::to_string(history.last().unwrap()).unwrap();
        assert!(last.contains("único"));
    }

    // ----- el loop agéntico `run_turn`, con un Mentor fake (sin red) -----

    use std::cell::RefCell;
    use std::collections::VecDeque;

    /// Mentor guionado: entrega sus respuestas en orden y registra cada input
    /// que recibe, para asertar sobre el flujo de rondas del loop.
    struct FakeMentor {
        replies: RefCell<VecDeque<Result<ChatReply>>>,
        inputs: RefCell<Vec<String>>,
    }

    impl FakeMentor {
        fn new(replies: Vec<Result<ChatReply>>) -> Self {
            Self { replies: RefCell::new(replies.into()), inputs: RefCell::new(Vec::new()) }
        }

        fn ok(reply: &str) -> Result<ChatReply> {
            Ok(ChatReply { text: reply.to_string(), calls: Vec::new(), usage: None })
        }

        fn ok_with_calls(
            text: &str,
            calls: Vec<rig_core::message::ToolCall>,
        ) -> Result<ChatReply> {
            Ok(ChatReply { text: text.to_string(), calls, usage: None })
        }

        fn fail(error: &str) -> Result<ChatReply> {
            Err(anyhow::anyhow!(error.to_string()))
        }
    }

    /// Construye una tool call como la emitiría el modelo.
    fn test_call(id: &str, name: &str, args: serde_json::Value) -> rig_core::message::ToolCall {
        rig_core::message::ToolCall {
            id: id.to_string(),
            call_id: None,
            function: rig_core::message::ToolFunction {
                name: name.to_string(),
                arguments: args,
            },
            additional_params: None,
            signature: None,
        }
    }

    impl TurnBrain for FakeMentor {
        async fn chat_stream(
            &self,
            input: &str,
            _history: &mut Vec<Message>,
            _on_delta: &mut dyn FnMut(&str),
        ) -> Result<ChatReply> {
            self.inputs.borrow_mut().push(input.to_string());
            self.replies
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(anyhow::anyhow!("el fake se quedó sin respuestas")))
        }
    }

    /// Corre un turno contra el fake en `dir`, contestando TODAS las
    /// confirmaciones con `answer`. Devuelve también el historial, donde
    /// quedan los tool results que el loop empujó.
    async fn fake_turn(fake: &FakeMentor, dir: &Path, answer: &str) -> (TurnOutcome, Vec<Message>) {
        let mut history = Vec::new();
        let skin = ui::skin();
        let store = ProjectStore::init(dir).unwrap();
        let mut ask = |_: &str| Some(answer.to_string());
        let out = run_turn(fake, &mut history, dir, &skin, &mut ask, &store, "hola", AutoMode::Off).await;
        (out, history)
    }

    /// Como `fake_turn` pero en modo AUTO y con `ask` que PROHÍBE preguntar:
    /// si algo pide confirmación en auto cuando no debe, el test explota.
    async fn fake_turn_auto(fake: &FakeMentor, dir: &Path) -> (TurnOutcome, Vec<Message>) {
        let mut history = Vec::new();
        let skin = ui::skin();
        let store = ProjectStore::init(dir).unwrap();
        let mut ask = |p: &str| -> Option<String> {
            panic!("en modo auto no debió preguntar nada, pero preguntó: {p}")
        };
        let out = run_turn(fake, &mut history, dir, &skin, &mut ask, &store, "hola", AutoMode::All).await;
        (out, history)
    }

    #[tokio::test]
    async fn turno_simple_es_una_sola_ronda() {
        let dir = tmp("turn-simple");
        let fake = FakeMentor::new(vec![FakeMentor::ok("hola, soy tu mentor")]);
        match fake_turn(&fake, &dir, "s").await.0 {
            TurnOutcome::Reply(r) => assert!(r.contains("soy tu mentor")),
            _ => panic!("esperaba Reply"),
        }
        assert_eq!(fake.inputs.borrow().len(), 1);
    }

    #[tokio::test]
    async fn dpx_read_realimenta_el_archivo_y_continua() {
        let dir = tmp("turn-read");
        std::fs::write(dir.join("datos.txt"), "SECRETO42").unwrap();
        let fake = FakeMentor::new(vec![
            FakeMentor::ok("voy a mirar\n```dpx:read path=datos.txt\n```\n"),
            FakeMentor::ok("listo, ya lo vi"),
        ]);
        let (out, _) = fake_turn(&fake, &dir, "s").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        let inputs = fake.inputs.borrow();
        assert_eq!(inputs.len(), 2);
        assert!(inputs[1].contains("SECRETO42"), "la ronda 2 debe llevar el contenido leído");
    }

    #[tokio::test]
    async fn modelo_caido_en_ronda_1_es_model_failed() {
        let dir = tmp("turn-fail1");
        let fake = FakeMentor::new(vec![FakeMentor::fail("403 forbidden")]);
        match fake_turn(&fake, &dir, "s").await.0 {
            TurnOutcome::ModelFailed(e) => assert!(e.contains("403")),
            _ => panic!("esperaba ModelFailed (candidato a fallback de cerebro)"),
        }
    }

    #[tokio::test]
    async fn error_en_ronda_2_conserva_lo_ya_dicho() {
        let dir = tmp("turn-fail2");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let fake = FakeMentor::new(vec![
            FakeMentor::ok("primera parte\n```dpx:read path=a.txt\n```\n"),
            FakeMentor::fail("403 forbidden"),
        ]);
        match fake_turn(&fake, &dir, "s").await.0 {
            TurnOutcome::Reply(r) => assert!(r.contains("primera parte")),
            _ => panic!("esperaba Reply con el texto de la ronda 1, no perderlo"),
        }
    }

    #[tokio::test]
    async fn write_rechazado_se_informa_al_modelo_sin_escribir() {
        let dir = tmp("turn-write-no");
        let fake = FakeMentor::new(vec![
            FakeMentor::ok("te propongo\n```dpx:write path=nuevo.txt\nhola\n```\n"),
            FakeMentor::ok("entendido, no lo escribo"),
        ]);
        let (out, _) = fake_turn(&fake, &dir, "n").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        let inputs = fake.inputs.borrow();
        assert_eq!(inputs.len(), 2, "el rechazo debe disparar una ronda de followup");
        assert!(inputs[1].contains("rechazó escribir"));
        assert!(!dir.join("nuevo.txt").exists());
    }

    // ----- sandbox de dpx:run -----

    #[test]
    fn comando_peligroso_exige_reescribir_la_primera_palabra() {
        let dir = tmp("safe-word");
        let store = ProjectStore::init(&dir).unwrap();
        // Responder "s" (el reflejo del piloto automático) NO basta.
        let mut ask_s = |_: &str| Some("s".to_string());
        assert!(matches!(
            confirm_run(&mut ask_s, &store, &dir, "git reset --hard", AutoMode::Off),
            RunDecision::Refused
        ));
        // Reescribir la primera palabra sí confirma.
        let mut ask_git = |_: &str| Some("git".to_string());
        assert!(matches!(
            confirm_run(&mut ask_git, &store, &dir, "git reset --hard", AutoMode::Off),
            RunDecision::Run
        ));
        // Y el modo auto NO exime a un comando peligroso de su puerta.
        let mut ask_no = |_: &str| Some("n".to_string());
        assert!(matches!(
            confirm_run(&mut ask_no, &store, &dir, "git reset --hard", AutoMode::All),
            RunDecision::Refused
        ));
    }

    #[test]
    fn el_peligro_manda_sobre_la_allowlist() {
        let dir = tmp("safe-allow");
        let store = ProjectStore::init(&dir).unwrap();
        store.allow_command("rm -rf target").unwrap();
        // Aunque esté en la allowlist, un comando peligroso vuelve a preguntar.
        let mut pregunto = false;
        let mut ask = |_: &str| {
            pregunto = true;
            Some("n".to_string())
        };
        assert!(matches!(
            confirm_run(&mut ask, &store, &dir, "rm -rf target", AutoMode::Off),
            RunDecision::Refused
        ));
        assert!(pregunto, "debió pedir confirmación reforzada pese a la allowlist");
    }

    #[test]
    fn comando_prohibido_se_bloquea_sin_preguntar() {
        let dir = tmp("safe-block");
        let store = ProjectStore::init(&dir).unwrap();
        let mut ask = |_: &str| -> Option<String> {
            panic!("un comando prohibido jamás debe llegar a preguntar")
        };
        assert!(matches!(
            confirm_run(&mut ask, &store, &dir, "shutdown /s /t 0", AutoMode::All),
            RunDecision::Blocked(_)
        ));
    }

    #[tokio::test]
    async fn run_prohibido_avisa_al_modelo_que_no_insista() {
        let dir = tmp("turn-run-block");
        let fake = FakeMentor::new(vec![
            FakeMentor::ok("limpio el disco\n```dpx:run\nformat c: /q\n```\n"),
            FakeMentor::ok("entendido, no lo propongo más"),
        ]);
        let (out, _) = fake_turn(&fake, &dir, "s").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        let inputs = fake.inputs.borrow();
        assert_eq!(inputs.len(), 2);
        assert!(inputs[1].contains("BLOQUEÓ"), "el modelo debe saber que fue dpx quien bloqueó");
    }

    #[tokio::test]
    async fn run_rechazado_se_informa_al_modelo() {
        let dir = tmp("turn-run-no");
        let fake = FakeMentor::new(vec![
            FakeMentor::ok("ejecuto\n```dpx:run\necho hola\n```\n"),
            FakeMentor::ok("vale, no lo ejecuto"),
        ]);
        let (out, _) = fake_turn(&fake, &dir, "n").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        let inputs = fake.inputs.borrow();
        assert_eq!(inputs.len(), 2);
        assert!(inputs[1].contains("rechazó ejecutar"));
    }

    // ----- guard anti-truncado en writes -----

    #[test]
    fn shrink_warning_solo_salta_en_encogimientos_sospechosos() {
        let grande = "x\n".repeat(100);
        // Archivo nuevo: sin aviso.
        assert_eq!(shrink_warning(None, "hola"), None);
        // Archivo chico que encoge: creíble, sin aviso.
        assert_eq!(shrink_warning(Some("a\nb\nc\n"), "a\n"), None);
        // Archivo grande a menos del 60%: aviso con las cifras.
        assert_eq!(shrink_warning(Some(&grande), &"y\n".repeat(10)), Some((100, 10)));
        // Archivo grande que apenas cambia: sin aviso.
        assert_eq!(shrink_warning(Some(&grande), &"y\n".repeat(90)), None);
    }

    #[test]
    fn write_truncado_rechazado_ensena_al_modelo_a_usar_edit() {
        let dir = tmp("guard-trunc");
        std::fs::write(dir.join("grande.rs"), "linea\n".repeat(200)).unwrap();
        let writes = vec![crate::fs::FileWrite {
            path: "grande.rs".into(),
            content: "linea\n".repeat(20), // 200 → 20 líneas: truncado casi seguro
        }];
        let report = process_writes(&dir, &writes, &mut |_| Some("n".to_string()), AutoMode::Off);
        assert!(report.needs_followup);
        assert!(report.notes[0].contains("TRUNCADA"));
        assert!(report.notes[0].contains("edit_file"));
        // El archivo no se tocó.
        let contenido = std::fs::read_to_string(dir.join("grande.rs")).unwrap();
        assert_eq!(contenido.lines().count(), 200);
    }

    #[test]
    fn write_all_no_salta_el_guard_anti_truncado() {
        let dir = tmp("guard-todos");
        std::fs::write(dir.join("grande.rs"), "linea\n".repeat(200)).unwrap();
        std::fs::write(dir.join("otro.rs"), "x\n").unwrap();
        let writes = vec![
            crate::fs::FileWrite { path: "otro.rs".into(), content: "y\n".into() },
            crate::fs::FileWrite {
                path: "grande.rs".into(),
                content: "linea\n".repeat(20),
            },
        ];
        // "a" en la primera activa write_all; el write sospechoso DEBE volver
        // a preguntar igualmente (contestamos "n" la segunda vez).
        let mut respuestas = vec!["a", "n"].into_iter();
        let report = process_writes(
            &dir,
            &writes,
            &mut |_| respuestas.next().map(str::to_string),
            AutoMode::Off,
        );
        assert!(respuestas.next().is_none(), "debió preguntar DOS veces (write_all no exime al guard)");
        assert!(report.notes.iter().any(|n| n.contains("TRUNCADA")));
        // El grande sigue intacto; el chico sí se escribió.
        assert_eq!(std::fs::read_to_string(dir.join("grande.rs")).unwrap().lines().count(), 200);
        assert_eq!(std::fs::read_to_string(dir.join("otro.rs")).unwrap(), "y\n");
    }

    #[test]
    fn reescritura_grande_sin_encoger_tambien_avisa() {
        let dir = tmp("guard-bigrw");
        std::fs::write(dir.join("grande.rs"), "linea\n".repeat(250)).unwrap();
        let writes = vec![crate::fs::FileWrite {
            path: "grande.rs".into(),
            content: "linea\n".repeat(240), // no encoge >40%, pero es rewrite completo
        }];
        let report = process_writes(&dir, &writes, &mut |_| Some("n".to_string()), AutoMode::Off);
        assert!(report.needs_followup);
        assert!(report.notes[0].contains("edits quirúrgicos"));
        assert_eq!(
            std::fs::read_to_string(dir.join("grande.rs")).unwrap().lines().count(),
            250
        );
        // Y `a=todos` tampoco lo salta: con write_all activo debe preguntar igual.
        std::fs::write(dir.join("chico.rs"), "x\n").unwrap();
        let writes = vec![
            crate::fs::FileWrite { path: "chico.rs".into(), content: "y\n".into() },
            crate::fs::FileWrite { path: "grande.rs".into(), content: "linea\n".repeat(240) },
        ];
        let mut respuestas = vec!["a", "n"].into_iter();
        process_writes(&dir, &writes, &mut |_| respuestas.next().map(str::to_string), AutoMode::Off);
        assert!(respuestas.next().is_none(), "debió preguntar dos veces");
    }

    #[tokio::test]
    async fn cuarentena_un_bloque_roto_anula_todos_los_bloques_de_texto() {
        let dir = tmp("turn-quarantine");
        // Marcador dpx:edit suelto (fuera de bloque) = malformado; el dpx:write
        // está BIEN formado, pero la cuarentena también lo anula.
        let reply = "voy a editar\ndpx:edit path=a.rs\n```dpx:write path=ok.txt\nhola\n```\n";
        let fake = FakeMentor::new(vec![
            FakeMentor::ok(reply),
            FakeMentor::ok("re-emito como tool calls"),
        ]);
        let (out, _) = fake_turn(&fake, &dir, "s").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        // El write bien formado NO se aplicó (ni se preguntó).
        assert!(!dir.join("ok.txt").exists());
        // El modelo recibe la cuarentena y una ronda para corregir.
        let inputs = fake.inputs.borrow();
        assert_eq!(inputs.len(), 2);
        assert!(inputs[1].contains("CUARENTENA"));
    }

    // ----- ciclo del plan persistente (.dpx/plan.md) -----

    fn turn_with_plan(marks: &str) -> Vec<Turn> {
        // marks: una letra por tarea, 'x' hecha / 'o' pendiente.
        let mut body = String::from("va el plan:\n```dpx:plan\n");
        for (i, m) in marks.chars().enumerate() {
            let mark = if m == 'x' { "[x]" } else { "[ ]" };
            body.push_str(&format!("{mark} tarea {i}\n"));
        }
        body.push_str("```\n");
        vec![Turn { role: "assistant", text: body }]
    }

    #[test]
    fn plan_con_pendientes_se_guarda_al_cerrar() {
        let dir = tmp("plan-save");
        let store = ProjectStore::init(&dir).unwrap();
        persist_plan(&store, &turn_with_plan("xo"));
        let saved = store.read_plan().expect("debió guardar .dpx/plan.md");
        assert!(saved.contains("[x] tarea 0"));
        assert!(saved.contains("[ ] tarea 1"));
    }

    #[test]
    fn plan_completo_se_limpia_y_sin_plan_se_conserva() {
        let dir = tmp("plan-clear");
        let store = ProjectStore::init(&dir).unwrap();
        store.write_plan("# Plan pendiente\n\n```dpx:plan\n[ ] vieja\n```\n").unwrap();
        // Sesión sin plan: el archivo anterior se conserva.
        persist_plan(&store, &[Turn { role: "assistant", text: "sin plan".into() }]);
        assert!(store.read_plan().is_some());
        // Sesión con el plan completado: se limpia.
        persist_plan(&store, &turn_with_plan("xx"));
        assert!(store.read_plan().is_none());
    }

    #[test]
    fn resume_plan_inyecta_y_respeta_el_rechazo_de_memoria() {
        let dir = tmp("plan-resume");
        let store = ProjectStore::init(&dir).unwrap();
        store.write_plan("# Plan pendiente\n\n```dpx:plan\n[ ] seguir\n```\n").unwrap();
        // Con memoria retomada: el plan viaja en el contexto inyectado.
        let prior = resume_plan(&store, Some("contexto previo".into())).unwrap();
        assert!(prior.contains("contexto previo"));
        assert!(prior.contains("[ ] seguir"));
        assert!(prior.contains("re-emítelo"));
        // Memoria rechazada (None): el plan NO se inyecta.
        assert!(resume_plan(&store, None).is_none());
        // Sin plan guardado: el contexto pasa intacto.
        store.remove_plan().unwrap();
        assert_eq!(resume_plan(&store, Some("solo".into())).unwrap(), "solo");
    }

    // ----- continuación de rondas, resiliencia y modo auto -----

    #[tokio::test]
    async fn el_turno_continua_mas_alla_de_8_rondas_si_el_usuario_acepta() {
        let dir = tmp("turn-extend");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let pide_leer = "sigo\n```dpx:read path=a.txt\n```\n";
        let mut replies: Vec<Result<ChatReply>> =
            (0..11).map(|_| FakeMentor::ok(pide_leer)).collect();
        replies.push(FakeMentor::ok("terminé"));
        let fake = FakeMentor::new(replies);
        // "s" responde tanto al checkpoint de rondas como a cualquier confirm.
        let (out, _) = fake_turn(&fake, &dir, "s").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        assert_eq!(
            fake.inputs.borrow().len(),
            12,
            "con el checkpoint aceptado, el turno debe pasar de las 8 rondas"
        );
    }

    #[tokio::test]
    async fn el_usuario_puede_frenar_el_turno_en_el_checkpoint() {
        let dir = tmp("turn-stop");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let pide_leer = "sigo\n```dpx:read path=a.txt\n```\n";
        let fake =
            FakeMentor::new((0..12).map(|_| FakeMentor::ok(pide_leer)).collect());
        let (out, _) = fake_turn(&fake, &dir, "n").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        assert_eq!(fake.inputs.borrow().len(), 8, "con 'n' el turno para en el presupuesto");
    }

    #[tokio::test]
    async fn corte_transitorio_a_mitad_de_turno_no_lo_mata() {
        let dir = tmp("turn-cut");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let fake = FakeMentor::new(vec![
            FakeMentor::ok("primera parte\n```dpx:read path=a.txt\n```\n"),
            FakeMentor::fail("error sending request: connection reset"),
            FakeMentor::ok("segunda parte, terminé"),
        ]);
        let (out, _) = fake_turn(&fake, &dir, "s").await;
        match out {
            TurnOutcome::Reply(full) => {
                assert!(full.contains("primera parte"));
                assert!(full.contains("segunda parte"), "el turno debe sobrevivir al corte");
            }
            _ => panic!("esperaba Reply"),
        }
        let inputs = fake.inputs.borrow();
        assert_eq!(inputs.len(), 3);
        assert!(inputs[2].contains("se cortó"), "el modelo debe saber que su ronda se perdió");
    }

    #[tokio::test]
    async fn error_no_transitorio_a_mitad_si_termina_el_turno() {
        let dir = tmp("turn-cut-perm");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let fake = FakeMentor::new(vec![
            FakeMentor::ok("avancé\n```dpx:read path=a.txt\n```\n"),
            FakeMentor::fail("402 Insufficient Balance"),
        ]);
        let (out, _) = fake_turn(&fake, &dir, "s").await;
        // Sin saldo no hay reintento que valga: conserva lo dicho y cierra.
        assert!(matches!(out, TurnOutcome::Reply(f) if f.contains("avancé")));
        assert_eq!(fake.inputs.borrow().len(), 2);
    }

    #[tokio::test]
    async fn modo_auto_aplica_write_y_run_seguro_sin_preguntar() {
        let dir = tmp("turn-auto");
        let fake = FakeMentor::new(vec![
            FakeMentor::ok_with_calls(
                "voy",
                vec![
                    test_call(
                        "c1",
                        "write_file",
                        serde_json::json!({ "path": "nuevo.txt", "content": "hola auto" }),
                    ),
                    test_call("c2", "run_command", serde_json::json!({ "command": "echo auto" })),
                ],
            ),
            FakeMentor::ok("listo"),
        ]);
        // fake_turn_auto PANICA si algo pregunta: éxito = nadie preguntó.
        let (out, history) = fake_turn_auto(&fake, &dir).await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        assert_eq!(std::fs::read_to_string(dir.join("nuevo.txt")).unwrap(), "hola auto");
        let serial = serde_json::to_string(&history).unwrap();
        assert!(serial.contains("auto"), "la salida del echo viaja como tool result");
    }

    #[tokio::test]
    async fn modo_auto_no_exime_al_guard_anti_truncado() {
        let dir = tmp("turn-auto-guard");
        std::fs::write(dir.join("grande.rs"), "linea\n".repeat(200)).unwrap();
        let fake = FakeMentor::new(vec![
            FakeMentor::ok_with_calls(
                "reescribo",
                vec![test_call(
                    "c1",
                    "write_file",
                    serde_json::json!({ "path": "grande.rs", "content": "linea\n".repeat(20) }),
                )],
            ),
            FakeMentor::ok("ok"),
        ]);
        // Aquí ask SÍ debe ser llamado (el guard pregunta incluso en auto).
        let mut history = Vec::new();
        let skin = ui::skin();
        let store = ProjectStore::init(&dir).unwrap();
        let mut pregunto = false;
        let mut ask = |_: &str| {
            pregunto = true;
            Some("n".to_string())
        };
        let _ =
            run_turn(&fake, &mut history, &dir, &skin, &mut ask, &store, "hola", AutoMode::All).await;
        assert!(pregunto, "el guard anti-truncado debe preguntar incluso en modo auto");
        assert_eq!(
            std::fs::read_to_string(dir.join("grande.rs")).unwrap().lines().count(),
            200,
            "el archivo no debe tocarse"
        );
    }

    #[test]
    fn extend_rounds_en_auto_respeta_el_tope_duro() {
        let mut budget = MAX_TURN_ROUNDS;
        let mut ask = |_: &str| -> Option<String> { panic!("en auto no se pregunta") };
        // Por debajo del tope: amplía solo.
        assert!(extend_rounds(&mut ask, 8, AutoMode::All, &mut budget));
        assert_eq!(budget, 16);
        // En el tope duro: frena.
        assert!(!extend_rounds(&mut ask, AUTO_MAX_ROUNDS, AutoMode::All, &mut budget));
    }

    #[test]
    fn truncate_log_recorta_sin_partir_utf8() {
        assert_eq!(truncate_log("corto", 10), "corto");
        let largo = "ñ".repeat(50);
        let out = truncate_log(&largo, 10);
        assert!(out.starts_with(&"ñ".repeat(10)));
        assert!(out.ends_with("[recortado]"));
    }

    // ----- herramientas git nativas -----

    /// Inicializa un repo git de prueba con un commit inicial. `None` si no
    /// hay git instalado (el test se salta solo).
    fn git_repo(name: &str) -> Option<PathBuf> {
        let dir = tmp(name);
        let ok = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["init", "-q"])
            .status()
            .ok()?
            .success();
        if !ok {
            return None;
        }
        // Identidad local para que el commit no falle en CI sin config global.
        for args in [["config", "user.email", "t@t.t"], ["config", "user.name", "t"]] {
            let _ = std::process::Command::new("git").current_dir(&dir).args(args).status();
        }
        std::fs::write(dir.join("a.txt"), "uno\n").unwrap();
        let _ = std::process::Command::new("git").current_dir(&dir).args(["add", "-A"]).status();
        let _ = std::process::Command::new("git")
            .current_dir(&dir)
            .args(["commit", "-q", "-m", "init"])
            .status();
        Some(dir)
    }

    #[test]
    fn git_status_y_diff_son_solo_lectura() {
        let Some(dir) = git_repo("git-ro") else { return };
        std::fs::write(dir.join("a.txt"), "uno\ndos\n").unwrap();
        assert!(run_git(&dir, &["status", "--short"]).contains("a.txt"));
        assert!(run_git(&dir, &["diff"]).contains("dos"));
    }

    #[test]
    fn git_commit_con_mensaje_de_varias_palabras_funciona() {
        // El bug original: split_whitespace partía el mensaje. Aquí el mensaje
        // tiene espacios y dos puntos y DEBE quedar íntegro en el log.
        let Some(dir) = git_repo("git-commit") else { return };
        std::fs::write(dir.join("b.txt"), "nuevo\n").unwrap();
        run_git(&dir, &["add", "-A"]);
        run_git(&dir, &["commit", "-m", "feat: mensaje con varias palabras"]);
        let log = run_git(&dir, &["log", "--oneline", "-1"]);
        assert!(log.contains("feat: mensaje con varias palabras"), "log fue: {log}");
    }

    #[tokio::test]
    async fn git_commit_rechazado_no_commitea() {
        let Some(dir) = git_repo("git-no-commit") else { return };
        std::fs::write(dir.join("c.txt"), "x\n").unwrap();
        let fake = FakeMentor::new(vec![
            FakeMentor::ok_with_calls(
                "commiteo",
                vec![test_call("c1", "git_commit", serde_json::json!({ "message": "no debería" }))],
            ),
            FakeMentor::ok("ok, no commiteo"),
        ]);
        // ask responde "n": el commit se rechaza.
        let mut history = Vec::new();
        let skin = ui::skin();
        let store = ProjectStore::init(&dir).unwrap();
        let mut ask = |_: &str| Some("n".to_string());
        let _ = run_turn(&fake, &mut history, &dir, &skin, &mut ask, &store, "hola", AutoMode::Off).await;
        // El log NO debe tener el commit rechazado.
        assert!(!run_git(&dir, &["log", "--oneline"]).contains("no debería"));
        let serial = serde_json::to_string(&history).unwrap();
        assert!(serial.contains("rechazó crear el commit"));
    }

    // ----- tool calls nativas (function calling) -----

    #[tokio::test]
    async fn tool_call_read_deja_el_contenido_como_tool_result() {
        let dir = tmp("tc-read");
        std::fs::write(dir.join("datos.txt"), "SECRETO42").unwrap();
        let fake = FakeMentor::new(vec![
            FakeMentor::ok_with_calls(
                "voy a leerlo",
                vec![test_call("c1", "read_file", serde_json::json!({ "path": "datos.txt" }))],
            ),
            FakeMentor::ok("ya lo vi"),
        ]);
        let (out, history) = fake_turn(&fake, &dir, "s").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        assert_eq!(fake.inputs.borrow().len(), 2, "una call debe disparar otra ronda");
        // El resultado viaja como tool result en el historial, con su id.
        let serial = serde_json::to_string(&history).unwrap();
        assert!(serial.contains("SECRETO42"));
        assert!(serial.contains("c1"));
    }

    #[tokio::test]
    async fn tool_call_write_rechazado_no_escribe_y_lo_reporta() {
        let dir = tmp("tc-write-no");
        let fake = FakeMentor::new(vec![
            FakeMentor::ok_with_calls(
                "te lo escribo",
                vec![test_call(
                    "c1",
                    "write_file",
                    serde_json::json!({ "path": "nuevo.txt", "content": "hola" }),
                )],
            ),
            FakeMentor::ok("entendido"),
        ]);
        let (_, history) = fake_turn(&fake, &dir, "n").await;
        assert!(!dir.join("nuevo.txt").exists());
        let serial = serde_json::to_string(&history).unwrap();
        assert!(serial.contains("rechazó escribir"));
    }

    #[tokio::test]
    async fn tool_call_run_prohibido_queda_bloqueado_en_el_tool_result() {
        let dir = tmp("tc-run-block");
        let fake = FakeMentor::new(vec![
            FakeMentor::ok_with_calls(
                "",
                vec![test_call(
                    "c1",
                    "run_command",
                    serde_json::json!({ "command": "shutdown /s /t 0" }),
                )],
            ),
            FakeMentor::ok("no insisto"),
        ]);
        let (_, history) = fake_turn(&fake, &dir, "s").await;
        let serial = serde_json::to_string(&history).unwrap();
        assert!(serial.contains("BLOQUEÓ"));
    }

    #[tokio::test]
    async fn tool_call_desconocida_devuelve_error_explicable() {
        let dir = tmp("tc-unknown");
        let fake = FakeMentor::new(vec![
            FakeMentor::ok_with_calls(
                "",
                vec![test_call("c1", "fetch_url", serde_json::json!({ "url": "x" }))],
            ),
            FakeMentor::ok("ok, uso las que existen"),
        ]);
        let (_, history) = fake_turn(&fake, &dir, "s").await;
        let serial = serde_json::to_string(&history).unwrap();
        assert!(serial.contains("desconocida"));
    }

    // ----- subagentes (spawn_agent) -----

    #[tokio::test]
    async fn subagente_lee_archivos() {
        let dir = tmp("sub-read");
        std::fs::write(dir.join("notas.txt"), "DATO_SUB").unwrap();
        let call = test_call("s1", "read_file", serde_json::json!({ "path": "notas.txt" }));
        let out = subagent_tool(&dir, &call).await;
        assert!(out.contains("DATO_SUB"), "el subagente debe poder leer: {out}");
    }

    #[tokio::test]
    async fn subagente_es_solo_lectura() {
        let dir = tmp("sub-ro");
        // Un write a través del subagente NO escribe y devuelve la negativa.
        let w = test_call(
            "s1",
            "write_file",
            serde_json::json!({ "path": "no.txt", "content": "x" }),
        );
        let out = subagent_tool(&dir, &w).await;
        assert!(!dir.join("no.txt").exists(), "el subagente no debe escribir");
        assert!(out.contains("SOLO LECTURA"), "debe rechazar con su naturaleza: {out}");

        // Tampoco puede ejecutar comandos…
        let r = test_call("s2", "run_command", serde_json::json!({ "command": "echo hola" }));
        assert!(subagent_tool(&dir, &r).await.contains("SOLO LECTURA"));

        // …ni anidar subagentes (sin recursión).
        let n = test_call("s3", "spawn_agent", serde_json::json!({ "task": "otra cosa" }));
        assert!(subagent_tool(&dir, &n).await.contains("SOLO LECTURA"));
    }

    #[test]
    fn subagent_preamble_aplica_la_identidad_del_rol() {
        use crate::agent::roles::AgentRole;
        let dir = tmp("sub-role");
        // El rol elegido inyecta su identidad; el default es investigador.
        let rev = subagent_preamble(&dir, "revisa fs/mod.rs", AgentRole::Reviewer);
        assert!(rev.contains("REVISOR"), "debe llevar la identidad del revisor: {rev}");
        let def = subagent_preamble(&dir, "x", AgentRole::parse(None));
        assert!(def.contains("INVESTIGADOR"), "el default es investigador");
        // En cualquier rol, las reglas de solo-lectura siguen presentes.
        assert!(rev.contains("SOLO LECTURA") && def.contains("SOLO LECTURA"));
    }

    // ----- compactación de tool-outputs viejos -----

    #[test]
    fn elide_tool_outputs_viejos_conserva_pairing_y_recientes() {
        let big = "X".repeat(5000);
        let mut history = vec![
            Message::user("hola"),
            Message::assistant("ok"),
            Message::tool_result("c1", big.clone()), // viejo + gordo → elidir
            Message::tool_result("c2", "corto"),     // viejo pero pequeño → intacto
        ];
        // 6 mensajes recientes que empujan lo anterior a la zona "vieja".
        for i in 0..6 {
            history.push(Message::user(format!("reciente {i}")));
        }
        // Un tool result reciente y gordo: NO debe elidirse (está en la cola).
        let recent_big = "Y".repeat(5000);
        history.push(Message::tool_result("c9", recent_big.clone()));

        let n = prune_tool_outputs(&mut history);
        assert_eq!(n, 1, "solo el tool result viejo y gordo se elide");

        let serial = serde_json::to_string(&history).unwrap();
        assert!(!serial.contains(&big), "el viejo gordo debe quedar elidido");
        assert!(serial.contains("c1"), "el id se conserva → emparejamiento tool intacto");
        assert!(serial.contains("salida elidida"));
        assert!(serial.contains("corto"), "el viejo pequeño queda intacto");
        assert!(serial.contains(&recent_big), "el tool result reciente NO se toca");

        // Idempotente: una segunda pasada no re-elide (prefijo de historial estable).
        assert_eq!(prune_tool_outputs(&mut history), 0);
        // Nada se borró: el número de mensajes no cambia (sin huérfanos → sin 400).
        assert_eq!(history.len(), 11);
    }

    // ── subagente: solo-lectura ────────────────────────────────────────

    /// Verifica que `subagent_tool` rechaza cualquier tool call que no sea
    /// read_file, search_project o web_search (el subagente es solo-lectura).
    #[tokio::test]
    async fn subagent_rechaza_escritura() {
        use rig_core::message::ToolCall;
        use serde_json::json;
        let call = ToolCall {
            id: "t1".into(),
            call_id: None,
            signature: None,
            additional_params: Default::default(),
            function: rig_core::message::ToolFunction {
                name: "write_file".into(),
                arguments: json!({"path": "x.txt", "content": "evil"}),
            },
        };
        let cwd = std::env::current_dir().unwrap();
        let out = subagent_tool(&cwd, &call).await;
        assert!(
            out.contains("SOLO LECTURA"),
            "el subagente debe rechazar write_file: {out}"
        );
    }

    /// Verifica que `subagent_tool` también rechaza comandos `run_command`.
    #[tokio::test]
    async fn subagent_rechaza_run() {
        use rig_core::message::ToolCall;
        use serde_json::json;
        let call = ToolCall {
            id: "t2".into(),
            call_id: None,
            signature: None,
            additional_params: Default::default(),
            function: rig_core::message::ToolFunction {
                name: "run_command".into(),
                arguments: json!({"command": "rm -rf /"}),
            },
        };
        let cwd = std::env::current_dir().unwrap();
        let out = subagent_tool(&cwd, &call).await;
        assert!(
            out.contains("SOLO LECTURA"),
            "el subagente debe rechazar run_command: {out}"
        );
    }

    /// Verifica que `subagent_tool` SÍ acepta read_file.
    #[tokio::test]
    async fn subagent_acepta_read() {
        use rig_core::message::ToolCall;
        use serde_json::json;
        let call = ToolCall {
            id: "t3".into(),
            call_id: None,
            signature: None,
            additional_params: Default::default(),
            function: rig_core::message::ToolFunction {
                name: "read_file".into(),
                arguments: json!({"path": "Cargo.toml"}),
            },
        };
        let cwd = std::env::current_dir().unwrap();
        let out = subagent_tool(&cwd, &call).await;
        assert!(
            out.contains("[package]") || out.contains("name"),
            "el subagente debe leer Cargo.toml: {out}"
        );
    }

    /// Verifica que el consumo de un subagente se refleja en el ledger de /cost.
    #[test]
    fn subagent_consumo_se_suma_al_ledger() {
        crate::token::reset();
        let before = crate::token::totals();

        crate::token::record(&Some(rig_core::completion::Usage {
            input_tokens: 500,
            output_tokens: 100,
            total_tokens: 600,
            cached_input_tokens: 200,
            cache_creation_input_tokens: 0,
            reasoning_tokens: 0,
        }));

        let (i, c, o) = crate::token::totals();
        assert_eq!(i, 500);
        assert_eq!(c, 200);
        assert_eq!(o, 100);

        let line = crate::token::turn_line(before).unwrap();
        assert!(line.contains("500 in") && line.contains("100 out"), "turn_line: {line}");

        let summary = crate::token::session_summary().unwrap();
        assert!(summary.contains("500") && summary.contains("100"), "/cost summary: {summary}");
    }
}
