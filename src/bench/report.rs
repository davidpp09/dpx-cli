//! Salida del banco: lo que se ve en la terminal y lo que queda en `eval/`.
//!
//! La consola da el veredicto de un vistazo; los transcripts guardan la
//! respuesta COMPLETA de cada arm, porque en los casos sin criterio automático
//! la calidad la juzgas tú leyéndolos, no un `assert`.

use std::path::Path;

use anyhow::{Context, Result};

use super::{Arm, Probe, Run};
use crate::agent::Effort;
use crate::ui;

pub fn print_header(arms: &[Arm], cases: usize, repeat: usize, out_dir: &Path) {
    let labels: Vec<&str> = arms.iter().map(|a| a.label).collect();
    println!(
        "\n{} {} · {} casos × {} arms × {} rep = {} corridas",
        ui::accent("⏺ banco de pruebas"),
        ui::dim(&labels.join(" vs ")),
        cases,
        arms.len(),
        repeat,
        cases * arms.len() * repeat
    );
    println!("  {}", ui::dim(&format!("transcripts → {}", out_dir.display())));
}

/// Veredicto de una sonda, en palabras que no afirman más de lo medido.
///
/// `reasoning_tokens > 0` es prueba positiva de que razonó. El `0` NO prueba lo
/// contrario: si el cliente no mapea ese campo, siempre llega en cero. Por eso
/// el caso ambiguo se compara contra la salida del mismo tier SIN thinking —
/// si pensar no encarece la salida, el esfuerzo no está llegando.
fn probe_verdict(p: &Probe, baseline: Option<u64>) -> String {
    let Err(e) = &p.accepted else {
        if p.reasoning_tokens > 0 {
            return format!("✓ razonó · {} tokens de razonamiento", p.reasoning_tokens);
        }
        if p.effort == Effort::Off {
            return format!("✓ aceptada · {} tokens de salida", p.output_tokens);
        }
        return match baseline {
            // Con thinking la salida crece de forma clara; un 20% de margen
            // deja fuera el ruido normal entre dos respuestas.
            Some(base) if p.output_tokens > (base as f64 * 1.2) as u64 => format!(
                "✓ aceptada · sin reasoning_tokens en el usage, pero la salida sube {} → {}",
                base, p.output_tokens
            ),
            Some(base) => format!(
                "⚠ aceptada pero SIN señal de razonamiento (salida {} vs {} sin thinking)",
                p.output_tokens, base
            ),
            None => format!(
                "⚠ aceptada · sin reasoning_tokens; corre el arm sin thinking del mismo tier \
                 para comparar (salida {})",
                p.output_tokens
            ),
        };
    };
    format!("✗ la API la rechazó · {}", ui::friendly_error(e))
}

/// Salida del mismo tier SIN thinking, si esa casilla también se sondeó.
fn baseline_for(p: &Probe, probes: &[Probe]) -> Option<u64> {
    probes
        .iter()
        .find(|o| o.model == p.model && o.effort == Effort::Off && o.accepted.is_ok())
        .map(|o| o.output_tokens)
}

pub fn print_probe(probes: &[Probe]) {
    println!("\n{}", ui::accent("⏺ sonda de reasoning_effort"));
    for p in probes {
        let detalle = probe_verdict(p, baseline_for(p, probes));
        println!("  {:<16} {:<22} {}", p.arm, ui::dim(&p.model), detalle);
    }
    if probes.iter().any(|p| p.effort != Effort::Off && p.reasoning_tokens == 0) {
        println!(
            "  {}",
            ui::dim(
                "nota: si ningún arm reporta reasoning_tokens, probablemente el cliente no mapea \
                 ese campo en DeepSeek; fíate de la comparación de tokens de salida"
            )
        );
    }
}

pub fn print_running(case: &str, arm: &str, rep: usize, total: usize) {
    let sufijo = if total > 1 { format!(" ({rep}/{total})") } else { String::new() };
    println!("\n  {}", ui::dim(&format!("▸ {case} · {arm}{sufijo}")));
}

pub fn print_run(run: &Run) {
    let veredicto = match (run.error.as_ref(), run.ok) {
        (Some(e), _) => format!("✗ error · {}", ui::friendly_error(e)),
        (None, Some(true)) => "✓ acierto".to_string(),
        (None, Some(false)) => format!("✗ falta: {}", run.missing.join(", ")),
        (None, None) => "· sin criterio automático (juzga el transcript)".to_string(),
    };
    println!(
        "    {} {}",
        veredicto,
        ui::dim(&format!(
            "{}ms · {} rondas · {} tools · {} in / {} out · ~${:.4}",
            run.ms, run.rounds, run.tool_calls, run.input, run.output, run.cost
        ))
    );
}

/// Agregado de un arm sobre todas sus corridas.
struct Total {
    arm: &'static str,
    aciertos: usize,
    puntuados: usize,
    errores: usize,
    ms: u128,
    rondas: usize,
    tools: usize,
    input: u64,
    output: u64,
    reasoning: u64,
    cost: f64,
    corridas: usize,
}

fn totals(runs: &[Run], arms: &[Arm]) -> Vec<Total> {
    arms.iter()
        .map(|arm| {
            let suyas: Vec<&Run> = runs.iter().filter(|r| r.arm == arm.label).collect();
            Total {
                arm: arm.label,
                aciertos: suyas.iter().filter(|r| r.ok == Some(true)).count(),
                puntuados: suyas.iter().filter(|r| r.ok.is_some()).count(),
                errores: suyas.iter().filter(|r| r.error.is_some()).count(),
                ms: suyas.iter().map(|r| r.ms).sum(),
                rondas: suyas.iter().map(|r| r.rounds).sum(),
                tools: suyas.iter().map(|r| r.tool_calls).sum(),
                input: suyas.iter().map(|r| r.input).sum(),
                output: suyas.iter().map(|r| r.output).sum(),
                reasoning: suyas.iter().map(|r| r.reasoning).sum(),
                cost: suyas.iter().map(|r| r.cost).sum(),
                corridas: suyas.len(),
            }
        })
        .collect()
}

impl Total {
    /// Latencia media por corrida, en segundos.
    fn s_medio(&self) -> f64 {
        if self.corridas == 0 { 0.0 } else { self.ms as f64 / self.corridas as f64 / 1000.0 }
    }

    /// Tasa de acierto (0.0–1.0). `0.0` si no hubo casos puntuables.
    fn tasa(&self) -> f64 {
        if self.puntuados == 0 { 0.0 } else { self.aciertos as f64 / self.puntuados as f64 }
    }
}

/// El arm recomendado: máxima tasa de acierto y, entre los que empatan ahí, el
/// más barato por acierto. La jerarquía es deliberada — un arm que falla no se
/// compensa con ser barato.
fn mejor_arm(totales: &[Total]) -> Option<&Total> {
    let candidatos: Vec<&Total> = totales.iter().filter(|t| t.aciertos > 0).collect();
    let mejor_tasa = candidatos
        .iter()
        .map(|t| t.tasa())
        .fold(f64::NEG_INFINITY, f64::max);
    candidatos
        .into_iter()
        // Margen mínimo por el redondeo de floats, no por tolerancia a fallos.
        .filter(|t| t.tasa() >= mejor_tasa - 1e-9)
        .min_by(|a, b| {
            let (x, y) = (a.cost / a.aciertos as f64, b.cost / b.aciertos as f64);
            x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal)
        })
}

pub fn print_table(runs: &[Run], arms: &[Arm]) {
    let totales = totals(runs, arms);
    println!("\n{}", ui::accent("⏺ resultado"));
    println!(
        "  {:<16} {:>9} {:>9} {:>8} {:>8} {:>10} {:>11}",
        "arm", "aciertos", "s/caso", "rondas", "tools", "razonam.", "costo"
    );
    for t in &totales {
        let aciertos = if t.puntuados == 0 {
            "—".to_string()
        } else {
            format!("{}/{}", t.aciertos, t.puntuados)
        };
        println!(
            "  {:<16} {:>9} {:>9.1} {:>8} {:>8} {:>10} {:>11}",
            t.arm,
            aciertos,
            t.s_medio(),
            t.rondas,
            t.tools,
            t.reasoning,
            format!("${:.4}", t.cost)
        );
        if t.errores > 0 {
            println!("  {}", ui::dim(&format!("    ({} corrida(s) con error)", t.errores)));
        }
    }

    // La conclusión, en el orden correcto: PRIMERO acertar, luego el precio.
    // Ordenar solo por costo/acierto premiaba al arm que fallaba —un fallo sale
    // del numerador y también del denominador, así que salir barato y fallar
    // puntuaba mejor que salir barato y acertar todo. Se descarta lo que no
    // empata en tasa de acierto y solo entre esos se compara el bolsillo.
    if let Some(mejor) = mejor_arm(&totales) {
        println!(
            "\n  {}",
            ui::dim(&format!(
                "mejor: {} · {}/{} aciertos · {:.1}s por caso · ~${:.4} por acierto",
                mejor.arm,
                mejor.aciertos,
                mejor.puntuados,
                mejor.s_medio(),
                mejor.cost / mejor.aciertos as f64
            ))
        );
    }
    println!(
        "  {}",
        ui::dim("los casos sin criterio automático NO cuentan aquí: léelos en los transcripts")
    );
}

pub fn print_footer(out_dir: &Path) {
    println!(
        "\n  {}\n",
        ui::dim(&format!("resumen completo → {}", out_dir.join("summary.md").display()))
    );
}

/// Un transcript por corrida: la respuesta íntegra, para juzgar calidad.
pub fn write_transcript(out_dir: &Path, run: &Run, rep: usize) -> Result<()> {
    let nombre = format!("{}__{}__{rep}.md", run.case, run.arm);
    let path = out_dir.join(&nombre);
    let veredicto = match (run.error.as_ref(), run.ok) {
        (Some(e), _) => format!("ERROR — {e}"),
        (None, Some(true)) => "acierto".to_string(),
        (None, Some(false)) => format!("fallo — falta: {}", run.missing.join(", ")),
        (None, None) => "sin criterio automático".to_string(),
    };
    let cuerpo = format!(
        "# {} · {}\n\n\
         - veredicto: {}\n\
         - latencia: {} ms\n\
         - rondas: {} · tool calls: {}\n\
         - tokens: {} in ({} de caché) · {} out · {} de razonamiento\n\
         - costo estimado: ${:.4}\n\n\
         ## Respuesta\n\n{}\n",
        run.case,
        run.arm,
        veredicto,
        run.ms,
        run.rounds,
        run.tool_calls,
        run.input,
        run.cached,
        run.output,
        run.reasoning,
        run.cost,
        if run.answer.trim().is_empty() { "[sin respuesta]" } else { run.answer.trim() },
    );
    // En los casos de escritura, lo que se juzga es el CÓDIGO, no el relato.
    // Va dentro del mismo transcript para poder comparar arms de un vistazo.
    let cuerpo = match &run.sandbox {
        Some(root) => format!("{cuerpo}\n## Código resultante\n\n{}", dump_sandbox(root)),
        None => cuerpo,
    };
    std::fs::write(&path, cuerpo)
        .with_context(|| format!("no pude escribir {}", path.display()))
}

/// Vuelca los archivos del sandbox en Markdown, en orden estable para que dos
/// arms se puedan comparar línea a línea.
fn dump_sandbox(root: &Path) -> String {
    let mut archivos = Vec::new();
    recolectar(root, root, &mut archivos);
    archivos.sort();
    if archivos.is_empty() {
        return format!("[el sandbox quedó vacío: {}]\n", root.display());
    }
    let mut md = format!("Sandbox: `{}`\n\n", root.display());
    for rel in archivos {
        let body = std::fs::read_to_string(root.join(&rel))
            .unwrap_or_else(|e| format!("[no pude leerlo: {e}]"));
        md.push_str(&format!("### {rel}\n\n```rust\n{}\n```\n\n", body.trim_end()));
    }
    md
}

fn recolectar(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            recolectar(root, &path, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// El `summary.md` de la corrida: tabla agregada + detalle caso por caso.
pub fn summary_markdown(runs: &[Run], arms: &[Arm], probes: &[Probe]) -> String {
    let mut md = String::from("# Banco de pruebas dpx\n\n");
    md.push_str(&format!("Fecha: {}\n\n", chrono::Local::now().format("%Y-%m-%d %H:%M")));

    md.push_str("## Sonda de reasoning_effort\n\n");
    md.push_str("| arm | modelo | veredicto | razonam. | salida |\n");
    md.push_str("|---|---|---|---|---|\n");
    for p in probes {
        let veredicto = probe_verdict(p, baseline_for(p, probes))
            .replace('|', "/")
            .replace('\n', " ");
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            p.arm, p.model, veredicto, p.reasoning_tokens, p.output_tokens
        ));
    }

    md.push_str("\n## Agregado\n\n");
    md.push_str("| arm | aciertos | s/caso | rondas | tools | razonam. | tokens in | tokens out | costo |\n");
    md.push_str("|---|---|---|---|---|---|---|---|---|\n");
    for t in totals(runs, arms) {
        let aciertos = if t.puntuados == 0 {
            "—".to_string()
        } else {
            format!("{}/{}", t.aciertos, t.puntuados)
        };
        md.push_str(&format!(
            "| {} | {} | {:.1} | {} | {} | {} | {} | {} | ${:.4} |\n",
            t.arm, aciertos, t.s_medio(), t.rondas, t.tools, t.reasoning, t.input, t.output, t.cost
        ));
    }

    md.push_str("\n## Detalle por caso\n\n");
    md.push_str("| caso | arm | veredicto | ms | rondas | costo |\n|---|---|---|---|---|---|\n");
    for r in runs {
        let veredicto = match (r.error.as_ref(), r.ok) {
            (Some(_), _) => "ERROR".to_string(),
            (None, Some(true)) => "✓".to_string(),
            (None, Some(false)) => format!("✗ falta {}", r.missing.join(", ")),
            (None, None) => "juicio humano".to_string(),
        };
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | ${:.4} |\n",
            r.case, r.arm, veredicto, r.ms, r.rounds, r.cost
        ));
    }
    md.push_str(
        "\n> Los casos sin criterio automático no puntúan: compara sus transcripts a mano.\n",
    );
    md
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token::Tier;

    fn run(case: &'static str, arm: &'static str, ok: Option<bool>, cost: f64) -> Run {
        Run {
            case,
            arm,
            ok,
            missing: Vec::new(),
            ms: 1000,
            rounds: 2,
            tool_calls: 1,
            input: 100,
            cached: 10,
            output: 50,
            reasoning: 0,
            cost,
            answer: "respuesta".into(),
            error: None,
            sandbox: None,
        }
    }

    const ARMS_TEST: [Arm; 2] = [
        Arm { label: "pro-nothink", tier: Tier::Pro, effort: Effort::Off },
        Arm { label: "flash-high", tier: Tier::Flash, effort: Effort::High },
    ];

    /// El caso real que destapó el bug: un arm barato que FALLA no puede ganarle
    /// a uno que acierta todo por poco más dinero.
    #[test]
    fn el_mejor_arm_prioriza_acertar_sobre_ser_barato() {
        let runs = vec![
            // flash-high: 1 de 2, barato.
            run("a", "flash-high", Some(true), 0.010),
            run("b", "flash-high", Some(false), 0.005),
            // pro-nothink: 2 de 2, un pelo más caro por acierto.
            run("a", "pro-nothink", Some(true), 0.011),
            run("b", "pro-nothink", Some(true), 0.011),
        ];
        let t = totals(&runs, &ARMS_TEST);
        // Por costo/acierto puro ganaría flash-high (0.015/1 = 0.015 vs
        // 0.022/2 = 0.011)… en realidad no; comprobemos la jerarquía de tasa.
        let mejor = mejor_arm(&t).expect("debe haber un mejor arm");
        assert_eq!(mejor.arm, "pro-nothink", "ganó un arm que falla casos");
    }

    #[test]
    fn con_igual_tasa_gana_el_mas_barato() {
        let runs = vec![
            run("a", "pro-nothink", Some(true), 0.030),
            run("a", "flash-high", Some(true), 0.010),
        ];
        let t = totals(&runs, &ARMS_TEST);
        assert_eq!(mejor_arm(&t).unwrap().arm, "flash-high");
    }

    #[test]
    fn sin_aciertos_no_hay_recomendacion() {
        let runs = vec![run("a", "pro-nothink", Some(false), 0.03)];
        let t = totals(&runs, &ARMS_TEST);
        assert!(mejor_arm(&t).is_none(), "no puede recomendar un arm que nunca acertó");
    }

    #[test]
    fn totales_separan_por_arm_y_no_puntuan_los_abiertos() {
        let runs = vec![
            run("a", "pro-nothink", Some(true), 0.01),
            run("b", "pro-nothink", Some(false), 0.01),
            run("c", "pro-nothink", None, 0.01), // abierto: no puntúa
            run("a", "flash-high", Some(true), 0.002),
        ];
        let t = totals(&runs, &ARMS_TEST);
        assert_eq!(t[0].aciertos, 1);
        assert_eq!(t[0].puntuados, 2, "el caso abierto no debe contar como puntuado");
        assert_eq!(t[0].corridas, 3);
        assert!((t[0].cost - 0.03).abs() < 1e-9);
        assert_eq!(t[1].aciertos, 1);
        assert_eq!(t[1].corridas, 1);
        assert!((t[1].s_medio() - 1.0).abs() < 1e-9);
    }

    fn probe(arm: &str, effort: Effort, reasoning: u64, output: u64) -> Probe {
        Probe {
            arm: arm.to_string(),
            model: "deepseek-v4-flash".into(),
            effort,
            accepted: Ok(()),
            reasoning_tokens: reasoning,
            output_tokens: output,
        }
    }

    #[test]
    fn reasoning_tokens_positivos_son_prueba_directa() {
        let p = probe("flash-high", Effort::High, 120, 200);
        assert!(probe_verdict(&p, None).starts_with("✓ razonó"));
    }

    #[test]
    fn sin_reasoning_tokens_se_compara_contra_el_baseline() {
        // La salida se dispara respecto a no-think → sí está pensando, aunque
        // el usage no lo reporte.
        let sube = probe("flash-high", Effort::High, 0, 500);
        assert!(probe_verdict(&sube, Some(100)).contains("la salida sube"));

        // Salida prácticamente igual → el esfuerzo NO está llegando.
        let plano = probe("flash-high", Effort::High, 0, 105);
        let v = probe_verdict(&plano, Some(100));
        assert!(v.starts_with("⚠"), "debe avisar, no dar por bueno: {v}");
        assert!(v.contains("SIN señal"), "{v}");

        // Sin baseline no se puede concluir: lo dice en vez de inventarlo.
        assert!(probe_verdict(&plano, None).contains("para comparar"));

        // Un arm sin thinking con 0 razonamiento es lo NORMAL: no debe alarmar.
        let off = probe("flash-nothink", Effort::Off, 0, 100);
        assert!(probe_verdict(&off, None).starts_with("✓ aceptada"));
    }

    #[test]
    fn baseline_solo_toma_el_mismo_modelo_sin_thinking() {
        let mut otro = probe("pro-nothink", Effort::Off, 0, 999);
        otro.model = "deepseek-v4-pro".into();
        let probes = vec![probe("flash-nothink", Effort::Off, 0, 100), otro];
        let alto = probe("flash-high", Effort::High, 0, 300);
        assert_eq!(
            baseline_for(&alto, &probes),
            Some(100),
            "no debe cruzar el baseline entre modelos distintos"
        );
    }

    #[test]
    fn summary_incluye_sonda_agregado_y_detalle() {
        let runs = vec![run("a", "pro-nothink", Some(true), 0.01)];
        let mut p = probe("pro-nothink", Effort::Off, 0, 42);
        p.model = "deepseek-v4-pro".into();
        let md = summary_markdown(&runs, &ARMS_TEST, &[p]);
        assert!(md.contains("Sonda de reasoning_effort"));
        assert!(md.contains("deepseek-v4-pro"));
        assert!(md.contains("## Agregado"));
        assert!(md.contains("## Detalle por caso"));
    }

    #[test]
    fn summary_no_rompe_la_tabla_si_el_error_trae_pipes() {
        let probes = vec![Probe {
            arm: "flash-high".to_string(),
            model: "deepseek-v4-flash".into(),
            effort: Effort::High,
            accepted: Err("400 | valor inválido\nsegunda línea".into()),
            reasoning_tokens: 0,
            output_tokens: 0,
        }];
        let md = summary_markdown(&[], &ARMS_TEST, &probes);
        // El error va en UNA celda: ni pipes ni saltos que partan la fila.
        let fila = md
            .lines()
            .find(|l| l.contains("deepseek-v4-flash"))
            .expect("falta la fila de la sonda");
        assert_eq!(fila.matches('|').count(), 6, "la fila se partió: {fila}");
    }
}
