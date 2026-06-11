//! El loop conversacional (REPL) del mentor, con UI estilo Claude Code,
//! respuestas en streaming y comandos (`/help`, `/clear`, `/focus`, …).
//!
//! La entrada (multilínea, autocompletado, pegados, historial) vive en el
//! editor propio sobre crossterm: ver `super::editor`.

use std::env;
use std::path::Path;

use anyhow::Result;
use rig_core::completion::Message;

use super::editor::{InputEditor, ReadResult};
use crate::agent::tools::{self, DpxCall};
use crate::agent::{Brain, ChatReply, Mentor, ModelRouter};
use crate::focus::{self, Mode, Persona};
use crate::session::{self, ProjectStore, Turn};
use crate::ui;

pub async fn run(focus: Option<String>, mode: Mode, brain: Brain, persona: Persona) -> Result<()> {
    let cwd = env::current_dir()?;
    let store = ProjectStore::init(&cwd)?;
    let skin = ui::skin();

    ui::install_ctrl_c_handler();
    ui::logo();

    // Con `--focus` explícito se respeta tal cual; sin él, el arranque inteligente
    // resuelve el enfoque (detección de stack) y si se retoma el contexto previo.
    let (focus_id, prior) = match focus {
        Some(f) => (Some(f), store.prior_context()),
        None => startup_flow(&cwd, &store),
    };

    // Estado mutable de la sesión (los comandos pueden cambiarlo en caliente).
    let mut focus_id = focus_id;
    let mut mode = mode;
    let mut brain = brain;
    let mut persona = persona;
    let mut router = ModelRouter::new(brain);
    let mut mentor = build_mentor(&router, focus_id.as_deref(), mode, persona, prior.as_deref())?;

    ui::welcome(
        focus::display_name(focus_id.as_deref()),
        mode_label(mode),
        router.brain_label(),
        &short_path(&cwd),
    );
    println!("  {}", ui::dim(&format!("persona   {}", persona_label(persona))));
    if prior.is_some() {
        println!("  {}", ui::dim("memoria · retomando contexto de sesiones anteriores"));
    }
    println!(
        "\n{}",
        ui::dim("escribe tu mensaje · @archivo lee código · Shift+Enter salto de línea · Ctrl-C cancela · /salir")
    );

    let mut history: Vec<Message> = Vec::new();
    let mut turns: Vec<Turn> = Vec::new();

    // Editor de entrada propio (crossterm): multilínea, Tab, pegados, historial.
    let mut ed = InputEditor::new(cwd.clone());

    loop {
        let bar = ui::format_input_status(
            focus::display_name(focus_id.as_deref()),
            mode_label(mode),
            router.brain_label(),
            persona_label(persona),
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
                if single_line && input == "/compact" {
                    compact_now(&router, &mut history, &turns).await;
                    continue;
                }

                if single_line && let Some(cmd) = input.strip_prefix('/') {
                    if matches!(cmd, "salir" | "exit" | "q") {
                        break;
                    }
                    handle_command(
                        cmd, &skin, &store, &prior, &mut router, &mut mentor, &mut focus_id,
                        &mut mode, &mut brain, &mut persona, &mut history, &cwd, turns.len(),
                    );
                    continue;
                }

                store.checkpoint("user", input)?;
                turns.push(Turn { role: "user", text: input.to_string() });

                // Turno (puede ser agéntico: el mentor pide archivos con dpx:read,
                // se los leemos y continúa, hasta dar su respuesta final).
                // Si el cerebro no llega a responder nada (sin saldo, cuota, caído),
                // se degrada al siguiente con API key y se reintenta UNA vez.
                ui::clear_cancel();
                let mut outcome = run_turn(
                    &mentor, &mut history, &cwd, &skin, &mut |p| ed.confirm_line(p), &store, input,
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
                            &new_router, focus_id.as_deref(), mode, persona, prior.as_deref(),
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
                                    input,
                                )
                                .await;
                            }
                            Err(e) => println!("{} {e}", ui::dim("no pude cambiar de cerebro:")),
                        }
                    }
                }
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

                // Compactación automática al acercarse al límite de contexto.
                if estimate_tokens(&history) > COMPACT_THRESHOLD_TOKENS {
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

    close_session(&router, &store, &turns, prior.as_deref()).await;
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
    print!("  ¿continuar? [S/n] ");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() {
        return true;
    }
    !matches!(answer.trim().to_lowercase().as_str(), "n" | "no")
}

/// Máximo de rondas agénticas por turno (lee/escribe/ejecuta → itera). Evita bucles.
const MAX_TURN_ROUNDS: usize = 8;

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

/// Umbral de compactación automática: al superar este estimado de tokens, el
/// historial se resume con el modelo barato para no chocar con el límite.
const COMPACT_THRESHOLD_TOKENS: usize = ui::CONTEXT_BUDGET * 3 / 4;

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
    let recent = history.split_off(keep_from);
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
async fn run_turn(
    mentor: &impl TurnBrain,
    history: &mut Vec<Message>,
    cwd: &Path,
    skin: &termimad::MadSkin,
    ask: &mut dyn FnMut(&str) -> Option<String>,
    store: &ProjectStore,
    user_input: &str,
) -> TurnOutcome {
    let mut to_send = attach_files(cwd, user_input);
    let mut full = String::new();
    let mut round = 0usize;

    loop {
        round += 1;
        let label = if round == 1 { "Pensando…" } else { "Continuando…" };
        let spinner = ui::Spinner::start(label);
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

        let ChatReply { text: reply, calls } = match result {
            Ok(r) => r,
            Err(e) => {
                if full.trim().is_empty() {
                    return TurnOutcome::ModelFailed(e.to_string());
                }
                ui::panel("⚠ error del modelo", &ui::friendly_error(&e.to_string()));
                return TurnOutcome::Reply(full);
            }
        };
        full.push_str(&reply);
        full.push('\n');

        // 1. Narración de esta ronda (sin bloques de acción) → Markdown progresivo.
        let body = crate::fs::strip_action_blocks(&reply);
        if !body.trim().is_empty() {
            ui::render_reply(skin, &body);
        }

        // Plan/checklist: si el modelo emitió un `dpx:plan`, lo pintamos vivo (☐/☑).
        if let Some(plan) = crate::fs::parse_plan(&reply) {
            ui::checklist(&plan);
        }

        // 2. Cambios propuestos en esta ronda (escrituras, ediciones quirúrgicas y
        //    borrados): confirmar y aplicar YA, guardando el resultado de cada uno
        //    para informárselo al modelo (que no asuma que algo se aplicó si el
        //    usuario lo rechazó o falló).
        let writes = crate::fs::parse_writes(&reply);
        let mut report = process_writes(cwd, &writes, &mut *ask);

        let edits = crate::fs::parse_edits(&reply);
        report.absorb(process_edits(cwd, &edits, &mut *ask));

        let deletes = crate::fs::parse_deletes(&reply);
        report.absorb(process_deletes(cwd, &deletes, &mut *ask));

        // Intentos de acción malformados (marcador fuera de bloque, dpx:edit sin
        // SEARCH/REPLACE completo): en vez de ignorarlos en silencio, se le avisa
        // al modelo para que los re-emita bien.
        let malformed = crate::fs::detect_malformed_actions(&reply);
        if !malformed.is_empty() {
            println!(
                "\n{}",
                ui::dim("⚠ detecté bloques dpx malformados · le pido al modelo que los corrija")
            );
            for warning in malformed {
                report.followup(warning);
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
                run_tool_call(cwd, store, &mut *ask, call, &mut s_writes, &mut s_edits);
            let (text, cancelled) = match outcome {
                ToolOutcome::Done(t) => (t, false),
                ToolOutcome::Cancelled(t) => (t, true),
            };
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

        // 3. Lecturas, búsquedas y ejecuciones
        let mut reads = Vec::new();
        for r in crate::fs::parse_reads(&reply) {
            if !reads.contains(&r) {
                reads.push(r);
            }
        }
        
        let searches = crate::fs::parse_searches(&reply);
        let mut runs = crate::fs::parse_runs(&reply);

        // Verificación de build automática: si escribimos código fuente y hay un
        // proyecto Maven/Gradle/Cargo, dpx propone compilar y realimenta los
        // errores al modelo para que itere (escribe → compila → corrige), sin que
        // tenga que pedirlo. Se omite si el modelo ya pidió el mismo build tool.
        let mut auto_built = false;
        if crate::fs::touches_build(&writes)
            || crate::fs::edits_touch_build(&edits)
            || crate::fs::touches_build(&s_writes)
            || crate::fs::edits_touch_build(&s_edits)
        {
            if let Some(build_cmd) = crate::fs::detect_build(cwd) {
                let already = runs
                    .iter()
                    .any(|r| r.contains("mvn") || r.contains("gradle") || r.contains("cargo"));
                if !already {
                    runs.push(build_cmd);
                    auto_built = true;
                }
            }
        }

        let has_requests =
            !reads.is_empty() || !searches.is_empty() || !runs.is_empty() || !calls.is_empty();
        if (has_requests || report.needs_followup) && round < MAX_TURN_ROUNDS {
            let mut ctx = String::new();
            if !report.notes.is_empty() {
                ctx.push_str("\n--- resultado de los cambios propuestos ---\n");
                for note in &report.notes {
                    ctx.push_str(note);
                    ctx.push('\n');
                }
                ctx.push_str("--- fin ---\n");
            }
            for r in &reads {
                ui::action_read(r);
                match crate::fs::read_file(cwd, r) {
                    Ok(c) => ctx.push_str(&format!("\n--- `{r}` ---\n{c}\n--- fin `{r}` ---\n")),
                    Err(e) => ctx.push_str(&format!("\n[no pude leer `{r}`: {e}]\n")),
                }
            }
            for s in &searches {
                println!("{}", ui::accent(&format!("  ⎁ buscando: {}", s)));
                let out = crate::fs::search_in_project(cwd, s);
                ctx.push_str(&format!("\n--- resultados de búsqueda para `{s}` ---\n{out}\n--- fin ---\n"));
            }
            if auto_built {
                println!("\n{}", ui::dim("dpx verifica que el proyecto compile…"));
            }
            for cmd in &runs {
                match confirm_run(&mut *ask, store, cwd, cmd) {
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

/// Resultado de atender una tool call nativa: el texto que va al historial
/// como tool result, y si la ejecución se canceló (aborta el turno).
enum ToolOutcome {
    Done(String),
    Cancelled(String),
}

/// Atiende UNA tool call: lecturas/búsquedas libres, escrituras/ediciones/
/// borrados con diff + confirmación (la misma maquinaria que los bloques de
/// texto) y comandos con el sandbox de `confirm_run`. Las escrituras y
/// ediciones se acumulan en `writes`/`edits` para el auto-build.
fn run_tool_call(
    cwd: &Path,
    store: &ProjectStore,
    ask: &mut dyn FnMut(&str) -> Option<String>,
    call: &rig_core::message::ToolCall,
    writes: &mut Vec<crate::fs::FileWrite>,
    edits: &mut Vec<crate::fs::FileEdit>,
) -> ToolOutcome {
    match tools::parse_call(&call.function.name, &call.function.arguments) {
        Err(e) => {
            println!("\n{}", ui::dim(&format!("⚠ tool call inválida: {e}")));
            ToolOutcome::Done(format!("[ERROR: {e}]"))
        }
        Ok(DpxCall::Read { path }) => {
            ui::action_read(&path);
            ToolOutcome::Done(match crate::fs::read_file(cwd, &path) {
                Ok(c) => c,
                Err(e) => format!("[no pude leer `{path}`: {e}]"),
            })
        }
        Ok(DpxCall::Search { pattern }) => {
            println!("{}", ui::accent(&format!("  ⎁ buscando: {pattern}")));
            ToolOutcome::Done(crate::fs::search_in_project(cwd, &pattern))
        }
        Ok(DpxCall::Write { path, content }) => {
            let w = crate::fs::FileWrite { path, content };
            let report = process_writes(cwd, std::slice::from_ref(&w), ask);
            writes.push(w);
            ToolOutcome::Done(report.notes.join("\n"))
        }
        Ok(DpxCall::Edit { path, search, replace }) => {
            let e = crate::fs::FileEdit { path, search, replace };
            let report = process_edits(cwd, std::slice::from_ref(&e), ask);
            edits.push(e);
            ToolOutcome::Done(report.notes.join("\n"))
        }
        Ok(DpxCall::Delete { path }) => {
            let report = process_deletes(cwd, &[path], ask);
            ToolOutcome::Done(report.notes.join("\n"))
        }
        Ok(DpxCall::Run { command }) => match confirm_run(ask, store, cwd, &command) {
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
    }
}

/// Ejecuta un comando YA confirmado: salida en vivo (acotada en pantalla; el
/// modelo recibe todo), timeout, cancelación por Ctrl-C y diagnóstico
/// automático de fallos. Devuelve (texto para el modelo, ¿se canceló?).
fn execute_run(cwd: &Path, cmd: &str) -> (String, bool) {
    let t = std::time::Instant::now();
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
    persona: Persona,
    prior: Option<&str>,
) -> Result<Mentor> {
    let mut preamble = focus::system_prompt(focus_id, mode, persona, prior)?;
    // Le damos el árbol del proyecto para que sepa qué archivos puede pedir leer.
    let cwd = env::current_dir().unwrap_or_default();
    preamble.push_str(
        "\n\n# Herramientas del Agente\n\
         Tienes herramientas NATIVAS (function calling): `read_file`, `search_project`, \
         `write_file`, `edit_file`, `delete_file` y `run_command`. PREFIÉRELAS SIEMPRE: \
         emite tool calls, no describas las acciones en prosa. Solo si no puedes usarlas, \
         existe el formato alternativo de bloques de texto:\n\
         - Leer: ```dpx:read path=ruta/al/archivo```\n\
         - Buscar en todo el proyecto: ```dpx:search pattern=\"término a buscar\"```\n\
         - Escribir/Sobrescribir: ```dpx:write path=ruta/al/archivo```\n\
         - Editar un fragmento (cambios puntuales): ```dpx:edit path=ruta/al/archivo``` con SEARCH/REPLACE\n\
         - Borrar un archivo: ```dpx:delete path=ruta/al/archivo```\n\
         - Ejecutar comando: ```dpx:run\nmvn compile\n``` (que termine solo: nada de servidores ni watch, hay timeout)\n\n\
         Seguridad de dpx:run: los comandos destructivos (borrados recursivos/forzados, \
         `git reset --hard`, `git push --force`, matar procesos, publicar a registros) exigen \
         una confirmación reforzada del usuario, y los que tocan el sistema (formatear, registro \
         de Windows, apagar) están PROHIBIDOS y dpx los bloquea: no los propongas; prefiere \
         siempre la alternativa más segura y acotada al proyecto.\n\n\
         # Árbol del proyecto actual\n\
         Estos son los archivos que existen AHORA en el proyecto. Léelos con `dpx:read` \
         o bórralos con `dpx:delete` cuando los necesites (no le pidas al usuario que lo haga):\n\n```\n",
    );
    preamble.push_str(&crate::fs::project_tree(&cwd));
    preamble.push_str("```\n");

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
    persona: &mut Persona,
    history: &mut Vec<Message>,
    cwd: &Path,
    turn_count: usize,
) {
    let mut parts = cmd.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("");
    let arg = parts.next().map(str::trim).filter(|s| !s.is_empty());

    match name {
        "help" | "?" | "" => ui::print_help(),

        "status" => ui::status_panel(
            env!("CARGO_PKG_VERSION"),
            &short_path(cwd),
            focus::display_name(focus_id.as_deref()),
            mode_label(*mode),
            persona_label(*persona),
            router.brain_label(),
            &brain_rows(*brain),
            turn_count,
            prior.is_some(),
            estimate_tokens(history),
        ),

        "models" | "model" => ui::models_list(&brain_rows(*brain)),

        "clear" => {
            history.clear();
            println!(
                "{}",
                ui::dim("conversación reiniciada · olvida el contexto de esta sesión")
            );
        }

        "context" => match store.prior_context() {
            Some(c) => ui::print_markdown(skin, "⏺ memoria del proyecto", &c),
            None => println!("{}", ui::dim("aún no hay memoria guardada para este proyecto")),
        },

        "focus" => match arg {
            None => {
                println!("{} {}", ui::dim("enfoque actual:"), focus::display_name(focus_id.as_deref()));
                focus::print_catalog();
            }
            Some(f) => match build_mentor(router, Some(f), *mode, *persona, prior.as_deref()) {
                Ok(m) => {
                    *mentor = m;
                    *focus_id = Some(f.to_string());
                    println!("{} {}", ui::accent("⏺ enfoque →"), focus::display_name(Some(f)));
                }
                Err(e) => println!("{} {e}", ui::dim("no pude cambiar de enfoque:")),
            },
        },

        "mode" => {
            let new = match arg {
                Some("pro") => Some(Mode::Pro),
                Some("hack") => Some(Mode::Hack),
                _ => None,
            };
            match new {
                Some(m) => match build_mentor(router, focus_id.as_deref(), m, *persona, prior.as_deref()) {
                    Ok(agent) => {
                        *mentor = agent;
                        *mode = m;
                        println!("{} {}", ui::accent("⏺ modo →"), mode_label(m));
                    }
                    Err(e) => println!("{} {e}", ui::dim("error:")),
                },
                None => println!(
                    "{} (actual: {})",
                    ui::dim("uso: /mode pro|hack"),
                    mode_label(*mode)
                ),
            }
        }

        "brain" => match arg.and_then(Brain::parse) {
            Some(b) => {
                let new_router = ModelRouter::new(b);
                match build_mentor(&new_router, focus_id.as_deref(), *mode, *persona, prior.as_deref()) {
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
                ui::dim("uso: /brain deepseek|gemini|groq|mistral"),
                router.brain_label()
            ),
        },

        // Cambia de persona en caliente (enseña ↔ hace).
        "mentor" | "code" => {
            let new_persona = if name == "code" { Persona::Code } else { Persona::Mentor };
            if *persona == new_persona {
                println!("{} {}", ui::dim("ya estás en persona:"), persona_label(new_persona));
            } else {
                match build_mentor(router, focus_id.as_deref(), *mode, new_persona, prior.as_deref()) {
                    Ok(agent) => {
                        *mentor = agent;
                        *persona = new_persona;
                        println!("{} {}", ui::accent("⏺ persona →"), persona_label(new_persona));
                    }
                    Err(e) => println!("{} {e}", ui::dim("error:")),
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

fn persona_label(persona: Persona) -> &'static str {
    match persona {
        Persona::Mentor => "mentor (enseña)",
        Persona::Code => "code (agente autónomo)",
    }
}

/// Cierre limpio: genera y persiste el contexto del proyecto.
async fn close_session(
    router: &ModelRouter,
    store: &ProjectStore,
    turns: &[Turn],
    prior: Option<&str>,
) {
    if turns.is_empty() {
        println!("\n{}", ui::dim("sesión vacía: no hay nada que recordar. Hasta luego."));
        return;
    }

    let spinner = ui::Spinner::start("Guardando memoria…");
    let summary = session::summarize(router, turns, prior).await;
    spinner.stop();

    match summary {
        Ok(md) => match store.write_context(&md) {
            Ok(()) => println!(
                "{} {}",
                ui::accent("⏺"),
                "contexto guardado en .dpx/context.md · la próxima vez retomo desde aquí."
            ),
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

        let accepted = write_all || {
            let current = crate::fs::current_content(cwd, &w.path);
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
        } else {
            println!("{}", ui::dim("omitido."));
            report.followup(format!("[el usuario rechazó escribir {}]", w.path));
        }
    }
    report
}

/// Procesa las ediciones quirúrgicas (`dpx:edit`): localiza el bloque SEARCH de
/// forma literal, muestra el diff +/- contra el archivo actual y aplica SOLO si
/// el usuario confirma. Si el bloque no aparece, error claro y no se toca nada.
fn process_edits(
    cwd: &std::path::Path,
    edits: &[crate::fs::FileEdit],
    ask: &mut dyn FnMut(&str) -> Option<String>,
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
                if matches!(ask("¿aplicar? [s/N] "), Some(a) if is_yes(&a)) {
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
            p.trim_end_matches(|c: char| matches!(c, '.' | ',' | ';' | ':' | ')' | '?' | '!'))
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
            ui::accent("⏺ ejecutar:"),
            ui::accent(cmd),
            ui::dim("(permitido siempre)")
        );
        return RunDecision::Run;
    }
    println!("\n{} {}", ui::accent("⏺ ejecutar:"), ui::accent(cmd));
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

fn mode_label(mode: Mode) -> &'static str {
    match mode {
        Mode::Pro => "pro (metódico)",
        Mode::Hack => "hack (rápido)",
    }
}

/// Acorta el path del proyecto usando `~` para el home (solo estético).
fn short_path(cwd: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = cwd.strip_prefix(&home) {
            // Normalizamos a '/' para que se vea consistente en Windows.
            return format!("~/{}", rest.display().to_string().replace('\\', "/"));
        }
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
    fn write_confirmado_se_aplica_y_reporta() {
        let dir = tmp("w-ok");
        let writes = vec![FileWrite { path: "a.txt".into(), content: "hola\n".into() }];
        let report = process_writes(&dir, &writes, &mut |_| Some("s".into()));
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "hola\n");
        assert!(!report.needs_followup);
        assert!(report.notes[0].contains("escrito"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn write_rechazado_pide_followup() {
        let dir = tmp("w-no");
        let writes = vec![FileWrite { path: "a.txt".into(), content: "hola\n".into() }];
        let report = process_writes(&dir, &writes, &mut |_| Some("n".into()));
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
        let report = process_writes(&dir, &writes, &mut |_| {
            asked += 1;
            Some("a".into())
        });
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
        let report = process_edits(&dir, &edits, &mut |_| Some("s".into()));
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
        let report = process_edits(&dir, &edits, &mut |_| {
            asked += 1;
            Some("s".into())
        });
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
        let report = process_edits(&dir, &edits, &mut |_| Some("s".into()));
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
            Ok(ChatReply { text: reply.to_string(), calls: Vec::new() })
        }

        fn ok_with_calls(
            text: &str,
            calls: Vec<rig_core::message::ToolCall>,
        ) -> Result<ChatReply> {
            Ok(ChatReply { text: text.to_string(), calls })
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
        let out = run_turn(fake, &mut history, dir, &skin, &mut ask, &store, "hola").await;
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
    async fn el_loop_corta_en_el_tope_de_rondas() {
        let dir = tmp("turn-tope");
        std::fs::write(dir.join("a.txt"), "x").unwrap();
        let pide_leer = "sigo\n```dpx:read path=a.txt\n```\n";
        let fake = FakeMentor::new(
            (0..MAX_TURN_ROUNDS + 2).map(|_| FakeMentor::ok(pide_leer)).collect(),
        );
        let (out, _) = fake_turn(&fake, &dir, "s").await;
        assert!(matches!(out, TurnOutcome::Reply(_)));
        assert_eq!(
            fake.inputs.borrow().len(),
            MAX_TURN_ROUNDS,
            "debe parar exactamente en el tope, aunque el modelo siga pidiendo acciones"
        );
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
            confirm_run(&mut ask_s, &store, &dir, "git reset --hard"),
            RunDecision::Refused
        ));
        // Reescribir la primera palabra sí confirma.
        let mut ask_git = |_: &str| Some("git".to_string());
        assert!(matches!(
            confirm_run(&mut ask_git, &store, &dir, "git reset --hard"),
            RunDecision::Run
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
            confirm_run(&mut ask, &store, &dir, "rm -rf target"),
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
            confirm_run(&mut ask, &store, &dir, "shutdown /s /t 0"),
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
}
