//! Prompts de confirmación bonitos: caja centrada con degradado del modo activo,
//! sin emojis, con color. La acción va arriba en color de acento; las opciones
//! abajo en dim. No lee input — la lectura la sigue haciendo el editor.
//!
//! En sesiones sin TTY (tests, pipes) degrada a un prompt plano tradicional.

use std::io::{self, IsTerminal, Write};

use crate::ui;

/// Pinta una caja centrada con la acción y las opciones. El prompt se parsea
/// del formato `"¿acción? [opciones]"` que usan `process_writes`,
/// `process_edits`, `process_deletes` y `confirm_run`.
///
/// Tras pintar la caja, deja el cursor listo para leer la respuesta con
/// `confirm_line`/`confirm_line_raw`.
pub fn print_confirmation_box(prompt: &str) {
    if !io::stdout().is_terminal() {
        print!("{prompt}");
        let _ = io::stdout().flush();
        return;
    }

    let (action, hint) = parse_prompt(prompt);

    let w = ui::term_width();
    // Caja: 60% del ancho, acotada para que no quede ridícula.
    let box_width = (w * 3 / 5).clamp(36, 72);
    let pad = (w.saturating_sub(box_width)) / 2;
    let pad_str = " ".repeat(pad);
    let inner = box_width.saturating_sub(2); // ancho interior (sin bordes)

    // Centra `s` en un campo de ancho `width`, respetando ANSI.
    let center = |s: &str, width: usize| -> String {
        let vis = ui::visible_width(s);
        let left = (width.saturating_sub(vis)) / 2;
        let right = width.saturating_sub(vis + left);
        format!("{}{s}{}", " ".repeat(left), " ".repeat(right))
    };

    let top = ui::grad(&format!(
        "{pad_str}╭{}╮",
        "─".repeat(box_width.saturating_sub(2))
    ));
    let empty = ui::grad(&format!("{pad_str}│{}│", " ".repeat(inner)));
    let bot = ui::grad(&format!(
        "{pad_str}╰{}╯",
        "─".repeat(box_width.saturating_sub(2))
    ));

    println!();
    println!("{top}");
    println!("{empty}");
    println!(
        "{pad_str}{}{}{}",
        ui::grad("│"),
        center(&ui::accent(action), inner),
        ui::grad("│")
    );
    if !hint.is_empty() {
        println!(
            "{pad_str}{}{}{}",
            ui::grad("│"),
            center(&ui::dim(hint), inner),
            ui::grad("│")
        );
    }
    println!("{empty}");
    println!("{bot}");
    // El cursor queda justo debajo del borde, listo para que el editor lea.
    print!("{pad_str}  › ");
    let _ = io::stdout().flush();
}

/// Extrae acción y opciones de un prompt tipo `"¿escribir? [s/N/a=todos] "`.
fn parse_prompt(prompt: &str) -> (&str, &str) {
    let prompt = prompt.trim();
    if let Some(idx) = prompt.find('[') {
        let action = prompt[..idx].trim();
        let hint = prompt[idx..].trim();
        (action, hint)
    } else {
        (prompt, "")
    }
}

#[cfg(test)]
mod tests {
    use super::parse_prompt;

    #[test]
    fn separa_accion_y_opciones() {
        // Los prompts reales de confirm_run/process_* traen `[opciones]`.
        assert_eq!(parse_prompt("¿escribir? [s/N/a=todos] "), ("¿escribir?", "[s/N/a=todos]"));
        assert_eq!(parse_prompt("¿ejecutar? [s/N/a=siempre] "), ("¿ejecutar?", "[s/N/a=siempre]"));
        assert_eq!(parse_prompt("  ¿aplicar? [s/N]  "), ("¿aplicar?", "[s/N]"));
    }

    #[test]
    fn sin_corchetes_devuelve_hint_vacio() {
        // Borde: un prompt sin `[` no debe romper ni inventar opciones.
        assert_eq!(parse_prompt("¿continuar?"), ("¿continuar?", ""));
        assert_eq!(parse_prompt(""), ("", ""));
    }
}
