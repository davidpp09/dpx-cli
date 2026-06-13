//! Sistema de hooks del proyecto (`.dpx/hooks.toml`).
//!
//! Permite ejecutar comandos automáticamente ante eventos del ciclo de vida
//! de dpx: inicio/cierre de sesión, antes/después de usar herramientas, etc.
//!
//! Formato de `.dpx/hooks.toml`:
//! ```toml
//! [[hooks]]
//! event = "PostToolUse"
//! tools = ["write_file", "edit_file"]   # opcional: filtra por tool
//! command = "cargo fmt"
//!
//! [[hooks]]
//! event = "PreCommit"
//! command = "cargo test"
//!
//! [[hooks]]
//! event = "OnSessionStart"
//! command = "echo 'bienvenido'"
//! ```

use std::path::Path;
use std::process::Command;

/// Un hook definido por el proyecto.
#[derive(Debug, Clone)]
pub struct Hook {
    /// Evento que lo dispara.
    pub event: HookEvent,
    /// Si está presente, solo se dispara para estas tools (por nombre).
    pub tools: Option<Vec<String>>,
    /// Comando shell a ejecutar.
    pub command: String,
}

/// Eventos del ciclo de vida que pueden disparar hooks.
#[derive(Debug, Clone, PartialEq)]
pub enum HookEvent {
    /// Antes de ejecutar una tool (puede vetarla).
    PreToolUse,
    /// Después de ejecutar una tool (p.ej. auto-formateo).
    PostToolUse,
    /// Al iniciar la sesión.
    OnSessionStart,
    /// Al cerrar la sesión (antes del resumen).
    OnSessionEnd,
    /// Antes de crear un commit git.
    PreCommit,
}

impl HookEvent {
    /// Parsea desde el nombre que aparece en el TOML.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "PreToolUse" => Some(Self::PreToolUse),
            "PostToolUse" => Some(Self::PostToolUse),
            "OnSessionStart" => Some(Self::OnSessionStart),
            "OnSessionEnd" => Some(Self::OnSessionEnd),
            "PreCommit" => Some(Self::PreCommit),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::OnSessionStart => "OnSessionStart",
            Self::OnSessionEnd => "OnSessionEnd",
            Self::PreCommit => "PreCommit",
        }
    }
}

/// Carga los hooks desde `.dpx/hooks.toml`.
pub fn load_hooks(root: &Path) -> Vec<Hook> {
    let path = root.join("hooks.toml");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let parsed: toml::Value = match toml::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let Some(arr) = parsed.get("hooks").and_then(|h| h.as_array()) else {
        return Vec::new();
    };
    arr.iter()
        .filter_map(|entry| {
            let event = HookEvent::parse(entry.get("event")?.as_str()?)?;
            let command = entry.get("command")?.as_str()?.to_string();
            let tools = entry
                .get("tools")
                .and_then(|t| t.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                });
            Some(Hook {
                event,
                tools,
                command,
            })
        })
        .collect()
}

/// Ejecuta todos los hooks que coincidan con el evento y (si `tool_name` es
/// Some) con el nombre de la tool. Devuelve `false` si algún hook `PreToolUse`
/// falló (exit ≠ 0), indicando que la acción debería cancelarse.
///
/// Los hooks `PreToolUse` son los únicos que pueden vetar: si su comando
/// devuelve un código distinto de cero, `run_hooks` retorna `false`. El resto
/// de hooks son best-effort (si fallan, se avisa pero no bloquean).
pub fn run_hooks(
    hooks: &[Hook],
    event: &HookEvent,
    tool_name: Option<&str>,
    cwd: &Path,
) -> bool {
    let mut ok = true;
    for hook in hooks {
        if hook.event != *event {
            continue;
        }
        // Filtrar por tool si el hook lo especifica.
        if let Some(ref tools) = hook.tools {
            if let Some(name) = tool_name {
                if !tools.iter().any(|t| t == name) {
                    continue;
                }
            } else {
                // El hook pide filtrar por tool pero no hay tool_name → no aplica.
                continue;
            }
        }

        // Hook silencioso: no spam en la UI salvo si falla.
        let output = Command::new(if cfg!(windows) { "cmd" } else { "sh" })
            .arg(if cfg!(windows) { "/C" } else { "-c" })
            .arg(&hook.command)
            .current_dir(cwd)
            .output();

        match output {
            Ok(out) if out.status.success() => {
                // Éxito: nada que decir (el hook hizo su trabajo).
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                eprintln!(
                    "  ⚠ hook {}/{} falló (exit {}): {}",
                    hook.event.as_str(),
                    hook.command,
                    out.status,
                    stderr.trim()
                );
                if hook.event == HookEvent::PreToolUse {
                    ok = false;
                }
            }
            Err(e) => {
                eprintln!(
                    "  ⚠ hook {}/{} no se pudo ejecutar: {e}",
                    hook.event.as_str(),
                    hook.command,
                );
                if hook.event == HookEvent::PreToolUse {
                    ok = false;
                }
            }
        }
    }
    ok
}
