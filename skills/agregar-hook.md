---
name: agregar un evento de hook del ciclo de vida
focus: dpx
cuando: "USAR cuando: agregar un hook o evento del ciclo de vida (OnSessionStart, PreToolUse y similares) que dispare comandos del usuario. NO usar para tools del agente ni comandos del REPL."
---
Los hooks viven en `src/cli/hooks.rs` y se configuran en `.dpx/hooks.toml`. Para
añadir un evento nuevo:

1. **Variante del enum**: añade el caso a `enum HookEvent` en `cli/hooks.rs`.
2. **Mapeo string ↔ enum**: añádelo en `HookEvent::parse` (string → variante) y en
   la conversión inversa (variante → string). Deben coincidir EXACTO con el valor
   que el usuario escribe en `hooks.toml`.
3. **Disparo**: llama a `run_hooks(&hooks, &HookEvent::TuEvento, ...)` en el punto
   del ciclo de vida donde debe dispararse (mira cómo se dispara `OnSessionStart`
   al arrancar la sesión en `src/cli/chat.rs`).
4. **Doc**: actualiza el comentario de cabecera de `hooks.rs` (lista de eventos) y
   el README si menciona los hooks.
5. **Test** de `parse`/round-trip del nuevo evento + clippy estricto + tests.
