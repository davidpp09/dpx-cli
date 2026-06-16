---
name: borrar o renombrar una feature sin dejar cabos
focus: dpx
cuando: "quitar, borrar, eliminar o renombrar una feature o un comando y limpiar todo lo que dejaba"
---
Borrar en dpx falla por DEJAR CABOS. Antes de declarar nada, haz `search_project`
del nombre/símbolo y barre TODOS estos puntos en el mismo turno:

1. **Código**: la función/struct + todos sus call sites (si cambias o quitas una
   firma, actualiza los usos YA, no en otra ronda — el build no se queda roto).
2. **Comando** (si es un `/x`): la entrada en la lista `COMMANDS` de
   `src/cli/commands.rs`, su mapeo en `command_in_mode` y `canonical_cmd`
   (`src/cli/chat.rs`), y el brazo del `match` en el dispatcher de `chat.rs`.
3. **Ayuda**: `print_help` en `src/ui.rs` (que desaparezca de `/ayuda`).
4. **Palabras reservadas del editor**: la lista de comandos en `src/cli/editor.rs`
   (CABO CLÁSICO que se olvida — déjalo y el editor sigue autocompletando algo muerto).
5. **System prompts**: los focus packs en `src/focus/` que mencionan el comando o
   la feature (otro CABO CLÁSICO — el prompt sigue anunciando algo que ya no existe).
6. **README.md** y los **tests** que lo referencian.
7. **Verifica**: `cargo clippy --all-targets -- -D warnings` (caza el dead-code que
   deja una eliminación a medias) + `cargo test`.

Regla de oro: el código impecable pero el `/ayuda`, el editor o el prompt mintiendo
= trabajo a medias.
