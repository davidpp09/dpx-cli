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
/// o `PreCommit` falló (exit ≠ 0), indicando que la acción debería cancelarse.
///
/// `PreToolUse` y `PreCommit` pueden vetar: si el comando devuelve un código
/// distinto de cero, `run_hooks` retorna `false`. El resto de hooks son
/// best-effort (si fallan, se avisa pero no bloquean).
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
                if hook.event == HookEvent::PreToolUse || hook.event == HookEvent::PreCommit {
                    ok = false;
                }
            }
            Err(e) => {
                eprintln!(
                    "  ⚠ hook {}/{} no se pudo ejecutar: {e}",
                    hook.event.as_str(),
                    hook.command,
                );
                if hook.event == HookEvent::PreToolUse || hook.event == HookEvent::PreCommit {
                    ok = false;
                }
            }
        }
    }
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_hook_events_roundtrip() {
        for (input, expected) in [
            ("PreToolUse", HookEvent::PreToolUse),
            ("PostToolUse", HookEvent::PostToolUse),
            ("OnSessionStart", HookEvent::OnSessionStart),
            ("OnSessionEnd", HookEvent::OnSessionEnd),
            ("PreCommit", HookEvent::PreCommit),
        ] {
            let parsed = HookEvent::parse(input).expect("debe parsear");
            assert_eq!(parsed, expected);
            assert_eq!(parsed.as_str(), input);
        }
        assert!(HookEvent::parse("RocketLaunch").is_none());
        assert!(HookEvent::parse("").is_none());
    }

    #[test]
    fn load_hooks_vacio_si_no_existe() {
        let tmp = std::env::temp_dir().join("dpx_test_hooks_empty");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(load_hooks(&tmp).is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn load_hooks_parsea_toml_valido() {
        let tmp = std::env::temp_dir().join("dpx_test_hooks_valid");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(
            tmp.join("hooks.toml"),
            r#"[[hooks]]
event = "PreCommit"
command = "cargo test"

[[hooks]]
event = "PostToolUse"
tools = ["write_file", "edit_file"]
command = "cargo fmt"
"#,
        )
        .unwrap();

        let hooks = load_hooks(&tmp);
        assert_eq!(hooks.len(), 2);

        let pre = &hooks[0];
        assert_eq!(pre.event, HookEvent::PreCommit);
        assert_eq!(pre.command, "cargo test");
        assert!(pre.tools.is_none());

        let post = &hooks[1];
        assert_eq!(post.event, HookEvent::PostToolUse);
        assert_eq!(post.command, "cargo fmt");
        assert_eq!(post.tools.as_deref(), Some(&["write_file".to_string(), "edit_file".to_string()][..]));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn run_hooks_precommit_falla_veta() {
        let hooks = vec![Hook {
            event: HookEvent::PreCommit,
            tools: None,
            command: if cfg!(windows) { "cmd /C exit 1".into() } else { "false".into() },
        }];
        let tmp = std::env::temp_dir().join("dpx_test_hooks_run");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // PreCommit con exit ≠ 0 debe vetar.
        assert!(!run_hooks(&hooks, &HookEvent::PreCommit, None, &tmp));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn run_hooks_posttool_best_effort_no_veta() {
        let hooks = vec![Hook {
            event: HookEvent::PostToolUse,
            tools: None,
            command: if cfg!(windows) { "cmd /C exit 1".into() } else { "false".into() },
        }];
        let tmp = std::env::temp_dir().join("dpx_test_hooks_post");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // PostToolUse con fallo NO veta (best-effort).
        assert!(run_hooks(&hooks, &HookEvent::PostToolUse, None, &tmp));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn run_hooks_filtra_por_tool() {
        let hooks = vec![Hook {
            event: HookEvent::PostToolUse,
            tools: Some(vec!["write_file".into()]),
            command: "echo ok".into(),
        }];
        let tmp = std::env::temp_dir().join("dpx_test_hooks_filter");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Coincide tool → se ejecuta.
        assert!(run_hooks(&hooks, &HookEvent::PostToolUse, Some("write_file"), &tmp));
        // No coincide tool → se salta (no hay nada que ejecutar → ok).
        assert!(run_hooks(&hooks, &HookEvent::PostToolUse, Some("edit_file"), &tmp));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
