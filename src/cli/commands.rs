//! Comandos slash personalizados desde `.dpx/commands.toml`.
//!
//! Cada comando define una `description`, un `prompt` que se inyecta como
//! tarea al modelo, y opcionalmente `confirm` (si requiere confirmación).
//!
//! Formato de `.dpx/commands.toml`:
//! ```toml
//! [commands.test]
//! description = "Ejecuta los tests y diagnostica fallos"
//! prompt = "Ejecuta los tests del proyecto, analiza los fallos y corrígelos uno por uno"
//! confirm = false
//! ```

use crate::session::CustomCommand;

/// Busca `name` entre los comandos personalizados. Si lo encuentra y
/// (si requiere confirmación) el usuario acepta, inyecta el prompt como
/// si fuera un mensaje del usuario y lanza un turno completo.
pub async fn dispatch_custom_command(
    cmds: &[CustomCommand],
    name: &str,
) -> Option<String> {
    let cmd = cmds.iter().find(|c| c.name == name)?;
    if cmd.confirm {
        print!("  ¿ejecutar /{}? [s/N] ", cmd.name);
        let _ = std::io::Write::flush(&mut std::io::stdout());
        let mut answer = String::new();
        if std::io::stdin().read_line(&mut answer).is_err() {
            return None;
        }
        if !matches!(answer.trim().to_lowercase().as_str(), "s" | "si" | "sí" | "y" | "yes") {
            println!("  cancelado.");
            return None;
        }
    }
    println!("  ⏺ {} · {}", cmd.name, cmd.description);
    Some(cmd.prompt.clone())
}
