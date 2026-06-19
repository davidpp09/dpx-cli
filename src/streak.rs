//! Racha de aprendizaje: sesiones consecutivas en modo `learn`.
//!
//! Persiste en `.dpx/streak.md`. Regla simple: sesión diaria seguida → sube;
//! día roto → vuelve a 1. El mismo día no cuenta doble.

use chrono::NaiveDate;

pub struct Streak {
    pub days: u32,
    pub last_date: String,
}

/// Actualiza la racha dado el día actual y la racha anterior (si la hay).
pub fn update(today: &str, prior: Option<Streak>) -> Streak {
    match prior {
        None => Streak { days: 1, last_date: today.to_string() },
        Some(s) if s.last_date == today => s,
        Some(s) => {
            let consecutive = day_before(today).is_some_and(|y| y == s.last_date);
            if consecutive {
                Streak { days: s.days + 1, last_date: today.to_string() }
            } else {
                Streak { days: 1, last_date: today.to_string() }
            }
        }
    }
}

/// Serializa para `.dpx/streak.md`.
pub fn to_markdown(s: &Streak) -> String {
    format!("racha: {}\nultima: {}\n", s.days, s.last_date)
}

/// Lee desde `.dpx/streak.md`. `None` si el formato no encaja.
pub fn from_markdown(md: &str) -> Option<Streak> {
    let mut days: Option<u32> = None;
    let mut last: Option<String> = None;
    for line in md.lines() {
        if let Some(v) = line.strip_prefix("racha: ") {
            days = v.trim().parse().ok();
        }
        if let Some(v) = line.strip_prefix("ultima: ") {
            last = Some(v.trim().to_string());
        }
    }
    Some(Streak { days: days?, last_date: last? })
}

/// Mensaje de racha para mostrar al usuario. `None` si es trivial (1 día).
pub fn message(days: u32) -> Option<String> {
    match days {
        0 | 1 => None,
        2 => Some("2 sesiones seguidas".to_string()),
        n => Some(format!("{n} sesiones seguidas")),
    }
}

fn day_before(date: &str) -> Option<String> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .ok()
        .map(|d| (d - chrono::Duration::days(1)).format("%Y-%m-%d").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primera_sesion_arranca_en_1() {
        let s = update("2026-06-19", None);
        assert_eq!(s.days, 1);
        assert_eq!(s.last_date, "2026-06-19");
    }

    #[test]
    fn mismo_dia_no_cuenta_doble() {
        let prior = update("2026-06-19", None);
        let again = update("2026-06-19", Some(prior));
        assert_eq!(again.days, 1);
    }

    #[test]
    fn dia_siguiente_incrementa_la_racha() {
        let prior = update("2026-06-18", None);
        let next = update("2026-06-19", Some(prior));
        assert_eq!(next.days, 2);
        assert_eq!(next.last_date, "2026-06-19");
    }

    #[test]
    fn brecha_de_dias_resetea_a_1() {
        let prior = Streak { days: 5, last_date: "2026-06-15".to_string() };
        let after_gap = update("2026-06-19", Some(prior));
        assert_eq!(after_gap.days, 1);
    }

    #[test]
    fn markdown_ida_y_vuelta() {
        let s = Streak { days: 7, last_date: "2026-06-19".to_string() };
        let md = to_markdown(&s);
        let back = from_markdown(&md).unwrap();
        assert_eq!(back.days, 7);
        assert_eq!(back.last_date, "2026-06-19");
    }

    #[test]
    fn message_omite_racha_trivial() {
        assert!(message(0).is_none());
        assert!(message(1).is_none());
        assert_eq!(message(2).as_deref(), Some("2 sesiones seguidas"));
        assert!(message(10).is_some());
    }
}
