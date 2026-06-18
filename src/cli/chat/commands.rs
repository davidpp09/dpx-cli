//! Despacho de comandos del REPL (`/...`) y sus helpers de prompt.
//! Extraído de `chat` sin cambio de comportamiento.

use std::path::Path;

use rig_core::completion::Message;

use super::{
    brain_rows, build_mentor, canonical_cmd, command_in_mode, ensure_embedder, estimate_tokens,
    mode_label, parse_token_count, reject_command, self_update, short_path,
};
use crate::agent::{Brain, Mentor, ModelRouter};
use crate::focus::{self, Mode};
use crate::session::ProjectStore;
use crate::ui;

/// Construye el prompt sintético de `/quiz`: instruye al tutor a hacer un repaso
/// de recuperación (retrieval practice) sobre lo que el usuario ya vio. Si se da
/// un tema, se centra en él; si no, usa las habilidades registradas; y si no hay
/// ninguna, repasa lo básico del temario del stack.
pub(crate) fn build_quiz_prompt(store: &ProjectStore, focus_id: Option<&str>, topic: &str) -> String {
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
pub(crate) fn remember(
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

/// Despacha un comando del REPL. Muta el estado de la sesión cuando aplica.
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_command(
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

        "hooks" => {
            let hooks = store.load_hooks();
            ui::hooks_panel(&hooks);
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
