---
name: agregar un comando al REPL
focus: dpx
cuando: "agregar, quitar o renombrar un comando slash (/algo) del REPL de dpx"
---
Para añadir un comando `/x` al REPL de dpx, toca estos puntos EN ESTE ORDEN (y
nada más — no explores el resto del repo):

1. **Tabla de comandos** — en `src/cli/commands.rs`, AÑADE la entrada nueva a la
   lista `COMMANDS` (no sustituyas ninguna existente). Ahí van nombre, ayuda y si
   pide confirmación.
2. **Alias español / canónico** — si el comando tiene nombre en español, mapéalo
   en `canonical_cmd` (`src/cli/chat.rs`).
3. **Filtro por modo** — si solo aplica a ciertos modos, regístralo en
   `command_in_mode` (`src/cli/chat.rs`); si no, queda disponible en todos.
4. **Manejo** — añade el brazo del `match` que ejecuta el comando en el
   dispatcher de comandos de `src/cli/chat.rs` (donde se resuelven `/panel`,
   `/progreso`, etc.).
5. **Ayuda** — añádelo a `print_help` en `src/ui.rs`, respetando el filtro por
   modo (que aparezca en `/ayuda`).
6. **Test** — un test del camino feliz en el módulo que tocaste.
7. **Verifica** — `cargo clippy --all-targets -- -D warnings` y `cargo test`.

Empieza a editar en el paso 1 de inmediato; con leer `commands.rs` y el
dispatcher de `chat.rs` te basta para arrancar.
