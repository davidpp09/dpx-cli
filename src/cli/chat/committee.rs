//! Comité de hack (lluvia de ideas: roles + síntesis). Extraído de `chat`.

use std::path::Path;

use super::run_subagent;
use crate::session::{ProjectStore, Turn};
use crate::ui;

/// Lanza el comité de hack desde el REPL: banner, 4 roles + síntesis,
/// y muestra el resultado (que incluye el bloque dpx:plan).
pub(crate) async fn run_comite_command(
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
pub(crate) async fn run_committee(cwd: &Path, idea: &str) -> String {
    let roles = crate::focus::committee::roles();
    let mut contributions: Vec<(String, String)> = Vec::new();

    for role in &roles {
        println!("{} · {}", ui::accent("  comité"), ui::dim(role.label));
        let task = crate::focus::committee::role_task(role, idea);
        let contrib = run_subagent(cwd, &task).await;
        contributions.push((role.label.to_string(), contrib));
    }

    println!(
        "{comite} · {synth}",
        comite = ui::accent("  comité"),
        synth = ui::dim("sintetizando aportes…")
    );
    let synthesis_task = crate::focus::committee::synthesis_prompt(&contributions, idea);
    run_subagent(cwd, &synthesis_task).await
}
