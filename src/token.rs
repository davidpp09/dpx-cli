//! Ledger de tokens y costo de la sesión.
//!
//! Captura el consumo REAL que reporta el proveedor (no la estimación tosca por
//! caracteres de `estimate_tokens`), lo acumula por sesión con atomics globales
//! (patrón single-user, como `ui::CANCEL`), y distingue los tokens de entrada
//! servidos desde el **caché de contexto** del proveedor (DeepSeek los cobra
//! ~10x más baratos) de los que se pagan completos. Ese % de caché es la métrica
//! que de verdad mueve la factura.

use std::sync::atomic::{AtomicU64, Ordering};

use rig_core::completion::Usage;

// ── Totales acumulados de la sesión ─────────────────────────────────
static IN_TOKENS: AtomicU64 = AtomicU64::new(0); // entrada total (incluye cacheados)
static CACHED_TOKENS: AtomicU64 = AtomicU64::new(0); // de la entrada, cuántos pegó el caché
static OUT_TOKENS: AtomicU64 = AtomicU64::new(0); // salida (tokens generados)

// ── Tarifas (USD por 1M tokens) ─────────────────────────────────────
// Aproximadas para DeepSeek; AJÚSTALAS aquí si cambian o usas otro cerebro. El
// conteo de tokens es EXACTO (viene de la API); solo el costo en $ es estimado.
const USD_PER_M_INPUT: f64 = 0.28; // entrada sin caché (cache miss)
const USD_PER_M_INPUT_CACHED: f64 = 0.028; // entrada servida del caché (~10x menos)
const USD_PER_M_OUTPUT: f64 = 0.42; // salida generada

/// Registra el consumo de una ronda. `None` (el proveedor no lo reportó en el
/// stream) no suma nada, así el medidor nunca miente con ceros inventados.
pub fn record(usage: &Option<Usage>) {
    let Some(u) = usage else { return };
    IN_TOKENS.fetch_add(u.input_tokens, Ordering::Relaxed);
    CACHED_TOKENS.fetch_add(u.cached_input_tokens, Ordering::Relaxed);
    OUT_TOKENS.fetch_add(u.output_tokens, Ordering::Relaxed);
}

/// Snapshot de los totales acumulados: `(entrada, cacheados, salida)`.
pub fn totals() -> (u64, u64, u64) {
    (
        IN_TOKENS.load(Ordering::Relaxed),
        CACHED_TOKENS.load(Ordering::Relaxed),
        OUT_TOKENS.load(Ordering::Relaxed),
    )
}

/// Costo estimado en USD de unos tokens `(entrada, cacheados, salida)`.
fn cost_usd(input: u64, cached: u64, output: u64) -> f64 {
    let uncached = input.saturating_sub(cached);
    (uncached as f64) * USD_PER_M_INPUT / 1_000_000.0
        + (cached as f64) * USD_PER_M_INPUT_CACHED / 1_000_000.0
        + (output as f64) * USD_PER_M_OUTPUT / 1_000_000.0
}

/// Porcentaje de la entrada servido desde el caché (0 si no hubo entrada).
fn cache_hit_pct(input: u64, cached: u64) -> u64 {
    if input == 0 {
        0
    } else {
        (cached as f64 / input as f64 * 100.0).round() as u64
    }
}

/// Formatea un conteo de tokens compacto: `1234` → `"1.2k"`.
fn k(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// Línea de cierre de un turno: el DELTA consumido respecto al snapshot
/// `before` = `(in, cached, out)` tomado antes del turno. `None` si no hubo
/// consumo real (p.ej. el proveedor no reportó usage, o turno sin llamada).
pub fn turn_line(before: (u64, u64, u64)) -> Option<String> {
    let (i, c, o) = totals();
    let (di, dc, do_out) = (
        i.saturating_sub(before.0),
        c.saturating_sub(before.1),
        o.saturating_sub(before.2),
    );
    if di == 0 && do_out == 0 {
        return None;
    }
    Some(format!(
        "⎿ {} in · {} out · caché {}% · ~${:.4}",
        k(di),
        k(do_out),
        cache_hit_pct(di, dc),
        cost_usd(di, dc, do_out)
    ))
}

/// Resumen acumulado de la sesión, para el comando `/cost`. `None` si todavía no
/// se registró consumo real (evita mostrar un panel de puros ceros).
pub fn session_summary() -> Option<String> {
    let (i, c, o) = totals();
    if i == 0 && o == 0 {
        return None;
    }
    Some(format!(
        "entrada {} (caché {} · {}%) · salida {} · total {} tok · ~${:.4}",
        k(i),
        k(c),
        cache_hit_pct(i, c),
        k(o),
        k(i + o),
        cost_usd(i, c, o)
    ))
}

/// Reinicia el ledger (al `/clear`, que arranca una conversación nueva).
pub fn reset() {
    IN_TOKENS.store(0, Ordering::Relaxed);
    CACHED_TOKENS.store(0, Ordering::Relaxed);
    OUT_TOKENS.store(0, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, cached: u64, output: u64) -> Option<Usage> {
        Some(Usage {
            input_tokens: input,
            output_tokens: output,
            total_tokens: input + output,
            cached_input_tokens: cached,
            cache_creation_input_tokens: 0,
            reasoning_tokens: 0,
        })
    }

    // Funciones PURAS (sin estado global): seguras de correr en paralelo.
    #[test]
    fn cache_hit_y_costo() {
        // 100% caché cuesta 10x menos que 0% caché, a igualdad de entrada.
        assert_eq!(cache_hit_pct(1000, 250), 25);
        assert_eq!(cache_hit_pct(0, 0), 0);
        let caro = cost_usd(1_000_000, 0, 0);
        let barato = cost_usd(1_000_000, 1_000_000, 0);
        assert!((caro - USD_PER_M_INPUT).abs() < 1e-9);
        assert!((barato - USD_PER_M_INPUT_CACHED).abs() < 1e-9);
        assert!(caro > barato * 5.0);
    }

    #[test]
    fn k_formatea_compacto() {
        assert_eq!(k(0), "0");
        assert_eq!(k(999), "999");
        assert_eq!(k(1500), "1.5k");
    }

    // El ledger usa atomics GLOBALES; un solo test secuencial evita carreras con
    // la ejecución en paralelo de cargo (ningún otro test toca estos globals).
    #[test]
    fn ledger_global_acumula_delta_y_resetea() {
        reset();
        assert!(turn_line((0, 0, 0)).is_none(), "sin consumo no hay línea");
        assert!(session_summary().is_none());

        record(&None); // None no suma
        assert_eq!(totals(), (0, 0, 0));

        record(&usage(2000, 1000, 500));
        record(&usage(500, 0, 100));
        assert_eq!(totals(), (2500, 1000, 600));

        // turn_line usa el DELTA respecto al snapshot dado.
        let line = turn_line((2000, 1000, 500)).unwrap(); // delta = (500, 0, 100)
        assert!(line.contains("500 in") && line.contains("100 out") && line.contains("caché 0%"));

        let resumen = session_summary().unwrap();
        assert!(resumen.contains("caché") && resumen.contains("total"));

        reset();
        assert_eq!(totals(), (0, 0, 0));
    }
}
