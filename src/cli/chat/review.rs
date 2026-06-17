//! Gate de AUTORREVISIÓN: antes de cerrar un turno AUTÓNOMO que cambió código,
//! un subagente REVISOR compara los cambios contra lo que pidió el usuario y
//! emite un veredicto. Si hay problemas P0/P1 (no hace lo pedido, o lo hace con
//! un bug real), el turno NO cierra: se fuerza una ronda de corrección. Es el
//! green-gate pero de COMPORTAMIENTO, no de compilación.
//!
//! Solo corre en modo auto (`/auto all`): es la situación "dejarlo solo", sin un
//! humano que revise el diff. En interactivo el humano ES la revisión.

use std::collections::HashSet;
use std::path::Path;

use super::{run_git, run_subagent};
use crate::ui;

/// Tope de líneas del diff que se le pasa al revisor: un diff enorme ahoga el
/// contexto y dispara el coste; lo grande lo revisa leyendo los archivos.
const MAX_DIFF_LINES: usize = 400;

/// Corre la autorrevisión. Devuelve `Some(crítica)` si el revisor encontró
/// problemas P0/P1 que el agente debe corregir antes de cerrar; `None` si los
/// cambios cumplen lo pedido (o no hay nada revisable / el veredicto fue OK).
pub(crate) async fn run_self_review(
    cwd: &Path,
    user_request: &str,
    changed_paths: &HashSet<String>,
) -> Option<String> {
    if changed_paths.is_empty() {
        return None;
    }
    let mut files: Vec<String> = changed_paths.iter().cloned().collect();
    files.sort();
    let diff = build_diff(cwd, &files);
    println!(
        "{}",
        ui::dim("⎿ autorrevisión: un revisor compara tus cambios con lo que se pidió…")
    );
    let task = review_task(user_request, &files, &diff);
    let conclusion = run_subagent(cwd, &task, Some("reviewer")).await;
    parse_verdict(&conclusion)
}

/// Construye el diff de los archivos cambiados (acotado). Vacío si no es git, no
/// hay HEAD, o git falla: el revisor igual puede leer los archivos él mismo.
fn build_diff(cwd: &Path, files: &[String]) -> String {
    if !cwd.join(".git").exists() {
        return String::new();
    }
    let mut args: Vec<&str> = vec!["diff", "HEAD", "--"];
    for f in files {
        args.push(f.as_str());
    }
    let out = run_git(cwd, &args);
    if out.contains("[git salió con") || out.contains("[error al ejecutar git") {
        return String::new(); // repo sin commits o git falló → sin diff
    }
    truncate_lines(&out, MAX_DIFF_LINES)
}

/// Prompt para el subagente revisor: comparar cambios vs intención del usuario y
/// emitir un veredicto estructurado (`VEREDICTO: OK` o `VEREDICTO: CORREGIR`).
fn review_task(user_request: &str, files: &[String], diff: &str) -> String {
    let diff_block = if diff.trim().is_empty() {
        "(sin diff disponible; LEE los archivos cambiados con read_file para evaluarlos)".to_string()
    } else {
        format!("```diff\n{diff}\n```")
    };
    format!(
        "El usuario pidió EXACTAMENTE esto:\n\"{user_request}\"\n\n\
         El agente cambió estos archivos: {lista}\n\n\
         Cambios realizados:\n{diff_block}\n\n\
         Tu trabajo: evalúa si los cambios CUMPLEN lo que el usuario pidió y si están BIEN hechos. \
         Clasifica cada problema que encuentres:\n\
         - P0 = no hace lo que el usuario pidió, o lo rompe.\n\
         - P1 = lo hace, pero con un bug real, un caso borde sin cubrir o una omisión importante.\n\
         - P2/P3 = menor o de estilo (NO bloquean).\n\n\
         Si necesitas más contexto, usa read_file/search_project sobre los archivos cambiados.\n\n\
         RESPONDE así, con la PRIMERA línea siendo el veredicto:\n\
         - `VEREDICTO: OK` si NO hay ningún P0 ni P1 (cumple lo pedido y está bien).\n\
         - `VEREDICTO: CORREGIR` si hay al menos un P0 o P1; debajo lista SOLO los P0/P1 \
         concretos (archivo:línea + qué está mal + qué falta), sin relleno.\n\
         Sé estricto pero JUSTO: no inventes problemas ni bloquees por gustos de estilo.",
        lista = files.join(", ")
    )
}

/// Interpreta el veredicto del revisor. `Some(issues)` = hay que corregir (P0/P1);
/// `None` = OK o veredicto ambiguo (ante la duda NO se bloquea: mejor cerrar que
/// ciclar contra un revisor confundido).
fn parse_verdict(conclusion: &str) -> Option<String> {
    let lower = conclusion.to_lowercase();
    if lower.contains("veredicto: corregir") || lower.contains("veredicto:corregir") {
        Some(conclusion.trim().to_string())
    } else {
        None
    }
}

/// Conserva las primeras `max` líneas (el grueso del cambio suele ir arriba).
fn truncate_lines(s: &str, max: usize) -> String {
    let lines: Vec<&str> = s.lines().collect();
    if lines.len() > max {
        format!("{}\n… [diff recortado a {max} líneas]", lines[..max].join("\n"))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_verdict_distingue_ok_corregir_y_ambiguo() {
        assert!(parse_verdict("VEREDICTO: OK\ntodo bien").is_none());
        let corregir = parse_verdict("VEREDICTO: CORREGIR\nP0 main.rs:5 falta el handler");
        assert!(corregir.is_some());
        assert!(corregir.unwrap().contains("P0"));
        // Insensible a mayúsculas/espacios.
        assert!(parse_verdict("veredicto:corregir\nP1 ...").is_some());
        // Ambiguo / sin veredicto → NO bloquea (mejor cerrar que ciclar).
        assert!(parse_verdict("creo que está más o menos bien").is_none());
        assert!(parse_verdict("").is_none());
    }

    #[test]
    fn truncate_lines_recorta_diffs_grandes() {
        let small = "a\nb\nc";
        assert_eq!(truncate_lines(small, 10), small);
        let big: String = (0..50).map(|i| format!("l{i}\n")).collect();
        let out = truncate_lines(&big, 10);
        assert!(out.contains("recortado a 10"));
        assert!(out.contains("l0") && !out.contains("l40"));
    }
}
