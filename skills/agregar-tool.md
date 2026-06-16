---
name: agregar una tool/acción del agente
focus: dpx
cuando: "USAR cuando: agregar una acción o tool nueva que dpx pueda emitir (un bloque dpx:algo que lee o muta el repo). NO usar para comandos del REPL (/x) ni paneles."
---
Las acciones de dpx son bloques con marcador (`dpx:write path=`, `dpx:read path=`,
`dpx:run`, `dpx:edit`, `dpx:delete`, `dpx:search`). Para añadir una:

1. **Parser** en `src/fs/mod.rs`: una función `parse_<x>_marker`/`is_<x>_fence`
   estilo las existentes (`parse_path_marker`, `is_run_fence`).
2. **Guard de stripping**: añade tu marcador a la lista de `on_fence`/`on_next` en
   `fs/mod.rs` (la que limpia los bloques de acción del texto visible) — si no, tu
   bloque se imprime crudo.
3. **Frontera lectura/mutación** (CRÍTICO): si la tool LEE, va libre. Si MUTA
   (write/edit/delete/run), cabléala por la puerta de confirmación en
   `src/cli/chat.rs` (`process_writes`/`process_edits`/`process_deletes`/`confirm_run`)
   y respeta los guards (shrink, big-rewrite, sandbox). NUNCA un atajo que mute en silencio.
4. **Doctrina**: documenta el marcador en el prompt de herramientas (`SHARED_TOOLS`
   en `src/focus/mod.rs`) para que dpx sepa que existe.
5. **Test** del parser (camino feliz + borde) y verifica con clippy estricto + tests.
