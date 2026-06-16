---
name: escribir tests en el repo de dpx
focus: dpx
cuando: "USAR cuando: escribir o arreglar un test de dpx, testear el loop del chat, mockear el modelo, o probar estado global (atómicos)."
---
Convenciones de testabilidad de dpx que DEBES respetar (romperlas hace tests
frágiles o imposibles):

1. **Costura del loop**: `run_turn` toma `&impl TurnBrain` (no `&Mentor`) y
   `ask: &mut dyn FnMut(&str) -> Option<String>` (no el editor directo). En los
   tests usa `FakeMentor` para guionar las `ChatReply` y un `ask` que da respuestas
   fijas. Si cambias la firma del loop, MANTÉN esta costura.
2. **Estado global = atómicos** (`ui::CANCEL`, `BUDGET`, el ledger de `token.rs`, el
   `State` de `checkpoint.rs`): NUNCA los toques desde un test (carreras entre
   tests). Prueba la lógica en una INSTANCIA LOCAL — copia el patrón de los tests de
   `token.rs`/`checkpoint.rs`.
3. **Cada lógica nueva lleva test** en su mismo módulo: camino feliz + un borde.
   Funciones de parsing/formato son las más fáciles y valiosas de cubrir.
4. **Verifica de verdad**: `cargo clippy --all-targets -- -D warnings` Y
   `cargo test`. clippy/test NO cazan bugs visuales — razona la salida real.
5. Tests temporales que crean archivos: usa rutas en `std::env::temp_dir()` con el
   pid, y límpialas al final.
