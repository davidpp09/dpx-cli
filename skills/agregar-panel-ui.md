---
name: agregar o cambiar un panel de la UI
focus: dpx
cuando: "agregar o modificar un panel, vista o caja de la UI (dashboard /panel, modo hack, /progreso, bienvenida)"
---
Para añadir o cambiar un panel/caja en la terminal, todo vive en `src/ui.rs`.
Haz esto y nada más (no reinventes el dibujo de la caja):

1. **Usa el helper `draw_box`** — `draw_box(&lines, color, max_width)` dibuja la
   caja redondeada con degradado. NUNCA dibujes los bordes a mano (`╭─╮`/`│`/`╰─╯`):
   ya está centralizado, repetirlo es un bug esperando.
2. **Arma `lines: Vec<String>`** con los helpers de color: `grad()` (degradado del
   modo), `accent()`, `dim()`. El color de cada modo (code azul, hack ámbar, learn
   verde) sale del tema activo; para colores FIJOS por modo, ANSI truecolor directo.
3. **TRUNCA los datos variables** del proyecto o del usuario (texto de `context.md`,
   `plan.md`, resúmenes) a ~64 chars con elipsis ANTES de meterlos a la caja: son
   párrafos enteros y desbordan el recuadro.
4. **No recalcules el ancho a mano**: `visible_width()` ya ignora las secuencias
   ANSI, úsalo (si cuentas los bytes del color, la caja se descuadra).
5. **SIN EMOJIS** — solo se permiten `⏺`, `✻`, `⎿`.
6. Si el panel se invoca por comando (`/algo`), cabléalo siguiendo el playbook
   "agregar un comando".
7. **Verifica de verdad**: clippy estricto + tests, y RAZONA la salida — un bug de
   layout (caja desbordada, padding mal) compila y pasa tests igual. Si puedes,
   míralo corriendo.
