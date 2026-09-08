//! Ledger de tokens y costo de la sesión.
//!
//! Captura el consumo REAL que reporta el proveedor (no la estimación tosca por
//! caracteres de `estimate_tokens`), lo acumula por sesión con atomics globales
//! (patrón single-user, como `ui::CANCEL`), y distingue los tokens de entrada
//! servidos desde el **caché de contexto** del proveedor (DeepSeek los cobra
//! ~120x más baratos) de los que se pagan completos. Ese % de caché es la
//! métrica que de verdad mueve la factura.
//!
//! El ledger separa por **tier** ([`Tier::Pro`] / [`Tier::Flash`]) porque pro
//! cuesta 3.1x lo que flash: mezclarlos en una sola tarifa hacía que el medidor
//! mintiera en ambos sentidos según qué modelo dominara el turno.

use std::sync::atomic::{AtomicU64, Ordering};

use rig_core::completion::Usage;

/// Tier de DeepSeek que consumió los tokens. Se factura distinto cada uno.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// `deepseek-v4-pro`: el cerebro de cada turno.
    Pro,
    /// `deepseek-v4-flash`: subagentes, resúmenes y clasificación.
    Flash,
}

impl Tier {
    pub fn label(self) -> &'static str {
        match self {
            Tier::Pro => "pro",
            Tier::Flash => "flash",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "pro" => Some(Tier::Pro),
            "flash" => Some(Tier::Flash),
            _ => None,
        }
    }

    fn idx(self) -> usize {
        match self {
            Tier::Pro => 0,
            Tier::Flash => 1,
        }
    }
}

const TIERS: [Tier; 2] = [Tier::Pro, Tier::Flash];

// ── Tarifas (USD por 1M tokens) ─────────────────────────────────────
// Precios de lista de DeepSeek al 2026-08-04. El conteo de tokens es EXACTO
// (viene de la API); solo el costo en $ es estimado. AJÚSTALAS aquí si cambian.
// PENDIENTE: DeepSeek anunció pricing peak/off-peak (2x en 9:00-12:00 y
// 14:00-18:00 hora Beijing, UTC+8) sin fecha efectiva todavía. Cuando entre,
// esto necesita multiplicar por 2 según la hora del turno.
struct Rates {
    /// Entrada sin caché (cache miss).
    input: f64,
    /// Entrada servida del caché de contexto.
    cached: f64,
    /// Salida generada.
    output: f64,
}

const RATES: [Rates; 2] = [
    // Pro: el caché descuenta ~120x.
    Rates { input: 0.435, cached: 0.003625, output: 0.87 },
    // Flash: el caché descuenta ~50x.
    Rates { input: 0.14, cached: 0.0028, output: 0.28 },
];

// ── Totales acumulados de la sesión, por tier ───────────────────────
static IN_TOKENS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)]; // entrada total (incluye cacheados)
static CACHED_TOKENS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)]; // de la entrada, cuántos pegó el caché
static OUT_TOKENS: [AtomicU64; 2] = [AtomicU64::new(0), AtomicU64::new(0)]; // salida (tokens generados)

// Presupuesto de tokens de la sesión (total in+out, sumando tiers). `0` = sin
// tope. Cuando se supera, el modo auto deja de auto-extenderse y pregunta antes
// de seguir gastando, y se avisa tras cada turno. Es un guardrail de gasto, no
// un corte duro a mitad de respuesta (eso rompería el protocolo de tool calls).
static BUDGET: AtomicU64 = AtomicU64::new(0);

/// Consumo `(entrada, cacheados, salida)` de un tier.
type Counts = (u64, u64, u64);

/// Foto del ledger completo en un instante, para calcular el delta de un turno.
/// Se toma antes del turno y se pasa a [`turn_line`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Snapshot {
    pro: Counts,
    flash: Counts,
}

impl Snapshot {
    fn get(&self, tier: Tier) -> Counts {
        match tier {
            Tier::Pro => self.pro,
            Tier::Flash => self.flash,
        }
    }

    fn set(&mut self, tier: Tier, counts: Counts) {
        match tier {
            Tier::Pro => self.pro = counts,
            Tier::Flash => self.flash = counts,
        }
    }

    /// Suma de los dos tiers: `(entrada, cacheados, salida)`.
    fn combined(&self) -> Counts {
        (
            self.pro.0 + self.flash.0,
            self.pro.1 + self.flash.1,
            self.pro.2 + self.flash.2,
        )
    }

    /// Delta de este snapshot respecto a uno anterior (saturante, nunca negativo).
    fn since(&self, before: &Snapshot) -> Snapshot {
        let mut out = Snapshot::default();
        for tier in TIERS {
            let (i, c, o) = self.get(tier);
            let (bi, bc, bo) = before.get(tier);
            out.set(
                tier,
                (
                    i.saturating_sub(bi),
                    c.saturating_sub(bc),
                    o.saturating_sub(bo),
                ),
            );
        }
        out
    }

    /// Costo estimado en USD, cada tier con su propia tarifa.
    fn cost_usd(&self) -> f64 {
        TIERS.iter().map(|&t| cost_of(t, self.get(t))).sum()
    }
}

/// Costo estimado en USD de un consumo suelto, a la tarifa del tier dado. Para
/// medir fuera del ledger de sesión (p. ej. el banco de pruebas).
pub fn estimate_cost(tier: Tier, input: u64, cached: u64, output: u64) -> f64 {
    cost_of(tier, (input, cached, output))
}

/// Costo en USD de un consumo concreto a la tarifa de su tier.
fn cost_of(tier: Tier, (input, cached, output): Counts) -> f64 {
    let r = &RATES[tier.idx()];
    let uncached = input.saturating_sub(cached);
    (uncached as f64) * r.input / 1_000_000.0
        + (cached as f64) * r.cached / 1_000_000.0
        + (output as f64) * r.output / 1_000_000.0
}

/// Registra el consumo de una ronda del `tier` dado. `None` (el proveedor no lo
/// reportó en el stream) no suma nada, así el medidor nunca miente con ceros
/// inventados.
pub fn record(tier: Tier, usage: &Option<Usage>) {
    let Some(u) = usage else { return };
    let i = tier.idx();
    IN_TOKENS[i].fetch_add(u.input_tokens, Ordering::Relaxed);
    CACHED_TOKENS[i].fetch_add(u.cached_input_tokens, Ordering::Relaxed);
    OUT_TOKENS[i].fetch_add(u.output_tokens, Ordering::Relaxed);
}

/// Foto de los totales acumulados de los dos tiers.
pub fn snapshot() -> Snapshot {
    let mut snap = Snapshot::default();
    for tier in TIERS {
        let i = tier.idx();
        snap.set(
            tier,
            (
                IN_TOKENS[i].load(Ordering::Relaxed),
                CACHED_TOKENS[i].load(Ordering::Relaxed),
                OUT_TOKENS[i].load(Ordering::Relaxed),
            ),
        );
    }
    snap
}

/// Snapshot de los totales acumulados sumando tiers: `(entrada, cacheados, salida)`.
pub fn totals() -> Counts {
    snapshot().combined()
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
/// `before` tomado antes del turno. `None` si no hubo consumo real (p.ej. el
/// proveedor no reportó usage, o turno sin llamada).
pub fn turn_line(before: Snapshot) -> Option<String> {
    let delta = snapshot().since(&before);
    let (di, dc, do_out) = delta.combined();
    if di == 0 && do_out == 0 {
        return None;
    }
    Some(format!(
        "⎿ {} in · {} out · caché {}% · ~${:.4}",
        k(di),
        k(do_out),
        cache_hit_pct(di, dc),
        delta.cost_usd()
    ))
}

/// Resumen acumulado de la sesión, para el comando `/cost`. `None` si todavía no
/// se registró consumo real (evita mostrar un panel de puros ceros). Desglosa
/// pro vs flash: son 3.1x distintos y ahí se ve si los subagentes están
/// cargando trabajo de verdad o si todo cae en el cerebro caro.
pub fn session_summary() -> Option<String> {
    let snap = snapshot();
    let (i, c, o) = snap.combined();
    if i == 0 && o == 0 {
        return None;
    }
    Some(format!(
        "entrada {} (caché {} · {}%) · salida {} · total {} tok · ~${:.4} (pro ${:.4} · flash ${:.4})",
        k(i),
        k(c),
        cache_hit_pct(i, c),
        k(o),
        k(i + o),
        snap.cost_usd(),
        cost_of(Tier::Pro, snap.get(Tier::Pro)),
        cost_of(Tier::Flash, snap.get(Tier::Flash)),
    ))
}

/// Reinicia el ledger (al `/clear`, que arranca una conversación nueva). NO
/// toca el presupuesto: el tope que pusiste sigue vigente tras un `/clear`.
pub fn reset() {
    for i in 0..TIERS.len() {
        IN_TOKENS[i].store(0, Ordering::Relaxed);
        CACHED_TOKENS[i].store(0, Ordering::Relaxed);
        OUT_TOKENS[i].store(0, Ordering::Relaxed);
    }
}

/// Fija el presupuesto de tokens de la sesión (`0` lo desactiva).
pub fn set_budget(tokens: u64) {
    BUDGET.store(tokens, Ordering::Relaxed);
}

/// Presupuesto actual (`0` = sin tope).
pub fn budget() -> u64 {
    BUDGET.load(Ordering::Relaxed)
}

/// Lógica pura del tope (testeable sin tocar el estado global): `0` = sin tope.
fn is_over(used: u64, budget: u64) -> bool {
    budget != 0 && used > budget
}

/// ¿Se superó el presupuesto? Siempre `false` si no hay tope (`0`).
pub fn over_budget() -> bool {
    let (i, _, o) = totals();
    is_over(i + o, budget())
}

/// Estado del presupuesto para `/budget` sin argumento. `None` si no hay tope.
pub fn budget_status() -> Option<String> {
    let b = budget();
    if b == 0 {
        return None;
    }
    let (i, _, o) = totals();
    let used = i + o;
    let pct = (used as f64 / b as f64 * 100.0).round() as u64;
    Some(format!(
        "{} / {} tok usados ({}%){}",
        k(used),
        k(b),
        pct,
        if used > b { " · SUPERADO" } else { "" }
    ))
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
    fn cache_hit_y_costo_por_tier() {
        assert_eq!(cache_hit_pct(1000, 250), 25);
        assert_eq!(cache_hit_pct(0, 0), 0);

        // 1M de entrada sin caché cuesta exactamente la tarifa de lista del tier.
        let pro = cost_of(Tier::Pro, (1_000_000, 0, 0));
        let flash = cost_of(Tier::Flash, (1_000_000, 0, 0));
        assert!((pro - 0.435).abs() < 1e-9, "pro: {pro}");
        assert!((flash - 0.14).abs() < 1e-9, "flash: {flash}");

        // Pro cuesta ~3.1x lo que flash, en entrada y en salida.
        let ratio_in = pro / flash;
        let ratio_out =
            cost_of(Tier::Pro, (0, 0, 1_000_000)) / cost_of(Tier::Flash, (0, 0, 1_000_000));
        assert!((ratio_in - 3.107).abs() < 0.01, "ratio entrada: {ratio_in}");
        assert!((ratio_out - 3.107).abs() < 0.01, "ratio salida: {ratio_out}");

        // El caché de pro descuenta ~120x (antes modelábamos 10x).
        let cacheado = cost_of(Tier::Pro, (1_000_000, 1_000_000, 0));
        assert!(pro / cacheado > 100.0, "descuento de caché muy chico: {cacheado}");
    }

    #[test]
    fn k_formatea_compacto() {
        assert_eq!(k(0), "0");
        assert_eq!(k(999), "999");
        assert_eq!(k(1500), "1.5k");
    }

    #[test]
    fn is_over_respeta_el_tope() {
        // 0 = sin tope: nunca se supera.
        assert!(!is_over(1_000_000, 0));
        // Estrictamente mayor cuenta como superado.
        assert!(!is_over(100, 100));
        assert!(is_over(101, 100));
    }

    #[test]
    fn snapshot_delta_no_se_va_en_negativo() {
        let mut antes = Snapshot::default();
        antes.set(Tier::Pro, (500, 100, 50));
        let despues = Snapshot::default(); // ledger reseteado a mitad
        assert_eq!(despues.since(&antes), Snapshot::default());
    }

    // El ledger usa atomics GLOBALES; un solo test secuencial evita carreras con
    // la ejecución en paralelo de cargo (ningún otro test toca estos globals).
    #[test]
    fn ledger_global_acumula_por_tier_y_resetea() {
        reset();
        assert!(turn_line(Snapshot::default()).is_none(), "sin consumo no hay línea");
        assert!(session_summary().is_none());

        record(Tier::Pro, &None); // None no suma
        assert_eq!(totals(), (0, 0, 0));

        record(Tier::Pro, &usage(2000, 1000, 500));
        let mitad = snapshot();
        record(Tier::Flash, &usage(500, 0, 100));

        // Los totales suman los dos tiers…
        assert_eq!(totals(), (2500, 1000, 600));
        // …pero cada tier guarda lo suyo.
        let snap = snapshot();
        assert_eq!(snap.get(Tier::Pro), (2000, 1000, 500));
        assert_eq!(snap.get(Tier::Flash), (500, 0, 100));

        // El delta del turno flash se cobra a tarifa flash, no a la de pro.
        let line = turn_line(mitad).unwrap();
        assert!(line.contains("500 in") && line.contains("100 out") && line.contains("caché 0%"));
        let delta = snapshot().since(&mitad);
        assert!(
            (delta.cost_usd() - cost_of(Tier::Flash, (500, 0, 100))).abs() < 1e-12,
            "el delta no usó la tarifa flash: {line}"
        );

        let resumen = session_summary().unwrap();
        assert!(resumen.contains("caché") && resumen.contains("pro $") && resumen.contains("flash $"));

        reset();
        assert_eq!(totals(), (0, 0, 0));
    }
}
