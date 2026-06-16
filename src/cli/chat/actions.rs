//! Aplicación de cambios con confirmación: la PUERTA por la que pasan TODAS
//! las mutaciones (write/edit/delete) y la ejecución de comandos del modelo.
//! Extraído de `chat` sin cambio de comportamiento.

use std::path::Path;

use super::short_path;
use crate::session::ProjectStore;
use crate::ui;

/// Ejecuta `git` con los args dados (cada uno SIN partir por espacios — clave
/// para que un mensaje de commit con espacios viaje como un solo argumento) en
/// `cwd` y devuelve su salida (stdout + stderr si falla). Para las
/// herramientas nativas git_*.
pub(crate) fn run_git(cwd: &Path, args: &[&str]) -> String {
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
/// automático de fallos. Devuelve (texto para el modelo, ¿se canceló?, código de
/// salida). El código (`Some(0)` = éxito) alimenta el green-gate de forma
/// determinista, sin depender de heurísticas sobre el texto de salida.
pub(crate) fn execute_run(cwd: &Path, cmd: &str) -> (String, bool, Option<i32>) {
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
    (text, out.cancelled, out.exit_code)
}

/// Resultado de aplicar los cambios propuestos por el modelo en una ronda
/// (escrituras, ediciones, borrados), para realimentárselo en la siguiente.
#[derive(Default)]
pub(crate) struct ActionReport {
    pub(crate) notes: Vec<String>,
    /// Hubo rechazos o fallos: el modelo debe enterarse y reaccionar.
    pub(crate) needs_followup: bool,
}

impl ActionReport {
    pub(crate) fn ok(&mut self, note: String) {
        self.notes.push(note);
    }

    pub(crate) fn followup(&mut self, note: String) {
        self.notes.push(note);
        self.needs_followup = true;
    }

    pub(crate) fn absorb(&mut self, other: ActionReport) {
        self.notes.extend(other.notes);
        self.needs_followup |= other.needs_followup;
    }
}

/// Procesa las propuestas de escritura: por cada archivo muestra un preview
/// compacto y lo escribe SOLO si el usuario confirma. Acepta `a`/`todos` para
/// escribir todos los restantes sin volver a preguntar. `ask` abstrae la
/// pregunta interactiva (rustyline en producción, respuestas fijas en tests).
pub(crate) fn process_writes(
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
pub(crate) fn shrink_warning(current: Option<&str>, new_content: &str) -> Option<(usize, usize)> {
    /// Por debajo de esto un encogimiento es creíble (refactor, limpieza).
    const MIN_LINES: usize = 80;
    let old = current?.lines().count();
    let new = new_content.lines().count();
    (old >= MIN_LINES && new * 10 < old * 6).then_some((old, new))
}

/// Detecta la reescritura completa de un archivo existente grande (la
/// doctrina dice `edit_file`): mismo riesgo de truncado, aunque no encoja.
pub(crate) fn big_rewrite_warning(current: Option<&str>) -> Option<usize> {
    /// El umbral de la doctrina de AGENTIC_SKILLS (~200 líneas).
    const MIN_LINES: usize = 200;
    let old = current?.lines().count();
    (old >= MIN_LINES).then_some(old)
}

/// Procesa las ediciones quirúrgicas (`dpx:edit`): localiza el bloque SEARCH de
/// forma literal, muestra el diff +/- contra el archivo actual y aplica SOLO si
/// el usuario confirma. Si el bloque no aparece, error claro y no se toca nada.
pub(crate) fn process_edits(
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
pub(crate) fn process_deletes(
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

/// Qué se decidió sobre un `dpx:run` propuesto, para informar al modelo con
/// precisión (no es lo mismo "el usuario no quiso" que "dpx lo prohibió").
pub(crate) enum RunDecision {
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
pub(crate) fn confirm_run(
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
pub(crate) fn warn_outside_paths(cwd: &Path, cmd: &str) {
    let outside = crate::fs::safety::outside_project_paths(cmd, cwd);
    if !outside.is_empty() {
        println!(
            "  {}",
            ui::dim(&format!("⚠ toca rutas fuera del proyecto: {}", outside.join(", ")))
        );
    }
}

pub(crate) fn is_yes(answer: &str) -> bool {
    matches!(
        answer.trim().to_lowercase().as_str(),
        "s" | "si" | "sí" | "y" | "yes"
    )
}
