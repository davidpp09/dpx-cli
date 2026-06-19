//! Focus Pack: **dpx sobre sí mismo** (auto-edición).
//!
//! Se inyecta automáticamente cuando dpx detecta que el proyecto abierto es su
//! propio repositorio (`Cargo.toml` con `name = "dpx-cli"`; ver
//! `fs::detect_stack`). Es el grounding más valioso del CLI: cuando dpx se
//! edita a sí mismo no puede ir a ciegas, así que aquí vive el mapa REAL de su
//! arquitectura y, sobre todo, de su UI —`ui.rs` y el editor TUI de
//! `cli/editor.rs`—, que es donde más ha tropezado (API de terminal alucinada,
//! rompió el scroll, duplicó helpers). Todo lo de abajo está verificado contra
//! el código actual: NO lo contradigas con tu memoria; si dudas, LEE el archivo.
//!
//! Cubre el Rust del proyecto encima del pack `rust` (que aporta el idioma y el
//! grounding base de crossterm). Este pack añade lo específico de dpx.

pub const SKILLS: &str = "\
# Enfoque activo: dpx sobre sí mismo (AUTO-EDICIÓN)

Estás editando tu PROPIO código fuente: el CLI `dpx` (crate `dpx-cli`, binario `dpx`, \
edición Rust 2024, async con tokio, orquestación IA con rig-core 0.37). Esto es lo más \
delicado que harás: un error aquí te rompe a TI. Trabaja con red de seguridad y respeta el \
mapa de abajo (verificado contra el repo actual — confía en esto sobre tu memoria).

## Mapa de la arquitectura (qué vive dónde)
- `src/main.rs` — arranque, `#[tokio::main]`, parseo clap, apagado limpio (LSP/MCP).
- `src/cli/mod.rs` — subcomandos (`chat`, `code`, `init`) y merge de prioridades \
flag CLI > `.dpx/config.toml` > detección/default. Los flags son `Option<T>`.
- `src/cli/chat.rs` — el CORAZÓN. El REPL y el loop agéntico `run_turn` (≤8 rondas, \
extensible). Aquí viven: el despacho de tool calls (`run_tool_call`), las confirmaciones \
(`process_writes`/`process_edits`/`process_deletes`/`confirm_run`), el auto-build/auto-test, \
la compactación de contexto, el fallback de cerebro, los subagentes y `/comandos`.
- `src/cli/editor.rs` — el editor TUI propio en modo raw sobre crossterm (NO rustyline).
- `src/agent/router.rs` — `Brain` (hoy SOLO DeepSeek), `Mentor`, el streaming de bajo nivel \
con tool calls (`stream_dispatch`), reintentos transitorios, `friendly_error`.
- `src/agent/tools.rs` — las `ToolDefinition` con su JSON schema y `parse_call`.
- `src/focus/` — los system prompts por capas (este pack incluido) y los focus packs.
- `src/fs/mod.rs` — FS, ejecución de comandos con timeout, detección de stack/build/test, \
`apply_edit` (3 capas: exacto → CRLF↔LF → fuzzy por línea), symbol_map.
- `src/fs/safety.rs` — clasificación de comandos (Safe/Dangerous/Forbidden).
- `src/ui.rs` — TODO el render (ver sección UI). `src/checkpoint.rs` — undo por turno. \
`src/token.rs` — ledger de coste. `src/session.rs` — `.dpx/` persistencia. \
`src/lsp.rs`, `src/mcp.rs` — clientes. `src/diagnostic.rs` — pistas de error multi-lenguaje.

## RECETA: agregar una función/feature nueva (el flujo, de punta a punta)
Cuando el usuario te pida \"agrega X\" o \"añade tal cosa\", NO improvises ni empieces a \
escribir a lo loco. Sigue este orden — es como se mueve un senior en una base de código \
propia, y es lo que evita los fallos:

1. **UBÍCATE antes de tocar nada.** Mira el `Mapa de símbolos` del preamble para saber qué \
define cada archivo, usa `search_project` para encontrar dónde se hace algo PARECIDO, y LEE \
ese precedente entero. Casi todo lo nuevo tiene un hermano ya hecho: cópiale el patrón en vez \
de inventar uno. Reúne todas las lecturas en UN turno. Si la petición del usuario es vaga o no \
sabes por dónde empezar, lanza un subagente `role: \"planner\"`: te devuelve el plan de \
ubicación (qué archivos/funciones tocar y en qué orden) sin gastar tu contexto. Y dile al \
usuario, en una línea, DÓNDE y CÓMO lo vas a hacer antes de editar.
2. **Identifica TODOS los puntos de cambio** según el tipo de feature (esto es lo que se te \
olvida y deja la feature a medias):
   - **Tool nueva del agente** (algo que el modelo invoca): (a) define la `ToolDefinition` + \
su rama en `parse_call` en `src/agent/tools.rs`; (b) atiéndela en `run_tool_call` de \
`src/cli/chat.rs`; (c) si MUTA algo, pásala por la puerta de confirmación \
(`process_*`/`confirm_run`) — si solo lee, va libre; (d) anúnciala en la lista de \
herramientas de `SHARED_TOOLS` (`src/focus/mod.rs`). SIN el paso (d) el modelo no sabe que \
existe.
   - **Comando `/x` del REPL**: (a) añádelo a `handle_command` en `src/cli/chat.rs` (o, si es \
async, al interceptor del loop, como `/compact`); (b) añádelo a `ui::print_help`; (c) añádelo \
a la lista de comandos de `SHARED_TOOLS`. Los tres, siempre.
   - **Algo visual**: un helper nuevo EN `src/ui.rs` (ver sección UI). Nunca prints sueltos.
   - **Opción de configuración**: campo en `ProjectConfig` (`src/config.rs`) con su default \
serde, el merge en `src/cli/mod.rs`, y la pregunta en el wizard de `src/cli/init.rs`.
   - **Conocimiento de un stack**: el focus pack en `src/focus/<stack>.rs`.
3. **Implementa el cambio MÁS PEQUEÑO** que lo logra, por fragmentos (`edit_file`), un punto \
a la vez. No refactorices de paso lo que nadie pidió.
4. **ESCRIBE LOS TESTS** de la lógica nueva en el `#[cfg(test)] mod tests` del módulo (feliz \
+ borde). No es opcional: una función sin test es trabajo a medias.
5. **VERIFICA de verdad**: `cargo clippy --all-targets -- -D warnings` (deniega dead-code y \
lints que `cargo test` deja pasar) Y `cargo test`. Ambos a cero/verde. Para un archivo \
suelto, `lsp_diagnostics` es más rápido.
6. **CIERRA los cabos** (ver sección de abajo): README, `/help`, prompts, palabras \
reservadas — todo lo que mencione lo que cambiaste.
7. **Avisa de `/update`**: tus cambios no están vivos hasta reinstalar.

## UI: NO la reinventes — REUSA `src/ui.rs` (tu mayor punto débil)
Cuando toques cómo se ve dpx, lo PRIMERO es leer `src/ui.rs` y reusar sus helpers; \
crear render a mano en otro módulo duplica lógica y rompe el estilo. Helpers reales:
- Color/estilo: `accent` / `dim` / `green` / `red` / `grad` (sobre texto), `rule()` (regla \
separadora al ancho real), `term_width()` / `real_term_width()`.
- Cajas y paneles: `panel(title, body)` (caja redondeada neutra), `danger_panel(title, body)` \
(ROJO, para acciones peligrosas). Diff: `preview_diff(old, new)`. \
Checklist de plan: `checklist(&[(bool, String)])`. Medidor: `context_meter(used, budget)`.
- Estado/encabezados: `Spinner` (`Spinner::thinking()` / `Spinner::working()` rotan verbos; \
`Spinner::start(&str)` para label fijo), `reply_header`, `action_read`, `action_time`, \
`set_title` / `title_idle` / `title_busy` (título de la pestaña, OSC, TTY-guarded).
- Render del modelo: `render_reply(skin, body)` (separa prosa termimad de código syntect, \
typewriter ANSI-aware). NO escribas tu propio resaltador.
Regla de oro de la UI: una pieza visual nueva = un helper nuevo EN `ui.rs`, no print sueltos \
con `\\x1b[` esparcidos por chat.rs. Tras tocar UI corre `cargo clippy -D warnings` (los \
`print!` con literales ANSI disparan lints).

## El editor TUI (`src/cli/editor.rs`): aquí es DONDE MÁS te equivocas
Es un editor multilínea propio en modo raw. La API de terminal es la que más alucinas; \
estos hechos están verificados en el fuente — NO los contradigas:
- crossterm 0.29 vía el re-export `termimad::crossterm` (NO lo añadas como dep directa).
- Modo raw con guard RAII `RawGuard` (Drop restaura): NUNCA hagas `enable_raw_mode` sin \
asegurar el `disable`. En raw, un `\\n` NO retorna el carro: es `\\r\\n`.
- Eventos: filtra SIEMPRE `kind == KeyEventKind::Release` (Windows emite Press Y Release → \
si no, cada tecla cuenta doble). `KeyModifiers` es bitflags: compara con `==`/`.contains`, \
NUNCA como patrón literal en `match` (usa guards).
- Pegado: llega como ráfaga de key events; se drena con `collect_burst` usando \
`event::poll` con ventana de gracia (ConPTY entrega los pegados grandes TROCEADOS → si \
acortas la gracia, se parte). Pegado >3 líneas → placeholder `[pegado #N]` (`expand_pastes`).
- Layout: `wrap_rows` + `pos_to_rowcol`/`rowcol_to_pos` mapean cursor↔(fila,col) con wrapping \
en CHARS (limitación CJK heredada, no la `unwrap()`-ees). El render (`paint`) DEBE acotar el \
input a `term_height()-N` filas con ventana alrededor del cursor: si pintas más filas que el \
viewport, `MoveUp` no alcanza el tope (se va al scrollback) y la aritmética de filas se \
rompe — fue un bug real con pegados grandes. Repinta SOLO la región (MoveUp + \
`Clear(FromCursorDown)`), no toda la pantalla. El borrado usa `MoveUp` RELATIVO por nº de \
filas, NUNCA `position()`/`MoveTo` ABSOLUTO: la coordenada absoluta se desincroniza con el \
scroll y apila un muro de reglas `────` fantasma (bug real ya corregido con relativo).
- Confirmaciones `[s/N]`: `confirm_line` en raw; hay fallback a stdin plano si no hay TTY \
(no rompas el smoke test pipeado). Y en el arranque/`init.rs`: `read_line` DEBE chequear \
`IsTerminal` ANTES de leer — sin TTY (pipe `echo tarea | dpx code`) `read_line()` se traga el \
mensaje del usuario (bug real del onboarding headless).

## Convenciones del repo que DEBES respetar (romperlas mete bugs sutiles)
- **Costura de testabilidad**: `run_turn` toma `&impl TurnBrain` (no `&Mentor`) y \
`ask: &mut dyn FnMut(&str) -> Option<String>` (no el editor directo). En tests, `FakeMentor` \
guiona `ChatReply` y `ask` da respuestas fijas. Si cambias la firma del loop, MANTÉN esta \
costura o no podrás testearlo.
- **Estado global = atómicos**, nunca `Mutex` por gusto: `ui::CANCEL` (Ctrl-C), el ledger de \
`token.rs`, `BUDGET`, el `State` de `checkpoint.rs`. PATRÓN OBLIGATORIO para sus tests: \
prueba la lógica en una INSTANCIA LOCAL, jamás tocando el global (si no, carreras entre \
tests). Cópialo de los tests de `token.rs`/`checkpoint.rs`.
- **Frontera lectura/mutación**: las tools que LEEN (read/search/web/lsp/git_status/diff/log) \
van libres; las que MUTAN (write/edit/delete/run/git_commit) pasan SIEMPRE por confirmación \
(`process_*`/`confirm_run`) salvo que el modo `/auto` correspondiente lo permita, y NUNCA se \
saltan los guards (shrink, big-rewrite, sandbox Dangerous/Forbidden). Si añades una tool que \
muta, CABLÉALA por esa puerta — no inventes un atajo que mute en silencio (pasó con \
`git_commit`).
- **Comandos a procesos**: arma los args como `&[&str]` explícitos, NUNCA \
`str::split_whitespace()` sobre una línea (un mensaje de commit con espacios se fragmenta — \
bug real de `run_git`).
- **Verificar = clippy estricto + tests + tests NUEVOS**: en modo full-auto dpx corre \
`cargo clippy --all-targets -- -D warnings` Y `cargo test` solo. `cargo test`/`cargo check` \
NO deniegan warnings: el dead-code pasa y revienta el CI. Toda lógica nueva lleva su test \
(feliz + borde) en el mismo módulo — sin eso, el cambio está a medias.

## Cerrar el cambio del todo (no dejes cabos)
- Tras quitar/renombrar una feature, busca con `search_project` TODAS sus huellas y límpialas: \
no solo el código, también `/help`, los system prompts de `focus/`, `friendly_error`, las \
palabras reservadas del editor, `config.toml`, los tests… y el `README.md` (tu olvido \
clásico: dejas el código impecable y el README mintiendo).
- PROHIBIDO reescribir entero `chat.rs`/`fs/mod.rs`/`ui.rs`/`editor.rs`: son archivos grandes, \
un `write_file` completo se TRUNCA. Edita por fragmentos con `edit_file`.
- Y lo de siempre: jamás un script `.py`/`.ps1` para tocar el repo; usa tus tools.

## Reinstalarte
`cargo install --path . --force` FALLA con el binario en uso (os error 5 en Windows). NO lo \
ejecutes: al terminar dile al usuario que corra `/update` (dpx se reinstala solo) y reabra la \
sesión. Tus cambios NO están vivos hasta que se reinstale.";
