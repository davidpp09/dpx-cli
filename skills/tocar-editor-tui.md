---
name: tocar el editor de entrada (modo raw)
focus: dpx
cuando: "tocar el editor de entrada, el modo raw, el render del prompt, el cursor, el pegado o el muro de reglas fantasma"
---
`src/cli/editor.rs` es un editor multilínea en modo raw — es DONDE MÁS se rompe dpx.
Estos hechos están verificados en el fuente; no los contradigas:

1. **Modo raw con `RawGuard` (RAII)** — Drop restaura. Nunca `enable_raw_mode` sin
   asegurar el `disable`. En raw, un `\n` NO retorna el carro: es `\r\n`.
2. **Filtra SIEMPRE `KeyEventKind::Release`** — Windows emite Press Y Release; si no
   lo filtras, cada tecla cuenta doble.
3. **Repinta SOLO la región** con `MoveUp` RELATIVO (por nº de filas) +
   `Clear(FromCursorDown)`. NUNCA `position()`/`MoveTo` ABSOLUTO: la coordenada
   absoluta se desincroniza con el scroll y apila un MURO de reglas `────` fantasma
   (bug real ya corregido — no lo reintroduzcas).
4. **Acota el input** a `term_height()-N` filas con ventana alrededor del cursor: si
   pintas más filas que el viewport, `MoveUp` no alcanza el tope y la aritmética se
   rompe (pasó con pegados grandes).
5. **`KeyModifiers` es bitflags**: compara con `==`/`.contains`, nunca como patrón
   literal en un `match` (usa guards).
6. **Confirmaciones / `read_line`**: chequea `IsTerminal` ANTES de leer. Sin TTY
   (pipe `echo tarea | dpx code`) leer se traga el mensaje del usuario (bug real del
   onboarding headless).
7. **Verifica corriendo dpx de verdad**: un bug visual/de cursor compila y pasa los
   tests igual — clippy y `cargo test` no lo cazan.
