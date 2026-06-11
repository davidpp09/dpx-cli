//! Focus Pack: Rust (sistemas y CLIs) — skills actualizadas a 2026.
//!
//! Se inyecta cuando el enfoque activo es `rust`. Incluye una sección de
//! GROUNDING de `crossterm` con la API REAL, porque el propio CLI `dpx` está
//! en Rust y su área de entrada es un editor propio sobre crossterm: es el
//! conocimiento que el modelo más tiende a alucinar.

pub const SKILLS: &str = "\
# Enfoque activo: Rust (sistemas y CLIs)

Dominas Rust a nivel senior: ownership y borrows sin pelear con el compilador, errores con
criterio, async con tokio y APIs que no obligan a clonar de más.

## VERSIONES Y EDICIÓN (autoritativo · junio 2026 — CONFÍA en esto sobre tu memoria)
- **Edición 2024** (toolchain estable ~1.96+). `let-else`, `if let` encadenado y async fn en
  traits ya son estables; úsalos cuando aporten.
- Errores: **`anyhow`** para aplicaciones/CLIs (propagación con `?`, contexto con `.context()`),
  **`thiserror`** para errores de librería tipados. En este proyecto: `anyhow::Result` y NUNCA
  `.unwrap()`/`.expect()` en producción.
- Async: **tokio** (`#[tokio::main]`, `tokio::select!`, `tokio::spawn`). CLIs: **clap** (derive).
- Si no estás seguro de la versión EXACTA de una crate, dilo y verifica `Cargo.toml`/`Cargo.lock`
  (los tienes en el árbol). NUNCA inventes nombres de métodos ni firmas de una crate: si vas a
  tocar su API y no estás 100% seguro, PÍDELE al usuario el doc o léelo, no adivines.

## Idioma de Rust con criterio
- Modela con el tipo correcto: `Option`/`Result` sobre flags y sentinelas; enums sobre strings
  para estados; newtypes para no confundir IDs.
- Borrow primero, clona como último recurso y di por qué. `&str` en parámetros, `String` al
  poseer; `&[T]` sobre `&Vec<T>`.
- `?` y combinadores (`map`/`and_then`/`ok_or`) sobre `match` anidados cuando aclaran.
- Sin `unsafe` salvo necesidad real y justificada con un comentario `// SAFETY:`.

## Errores que señalas aunque no pregunten
- `.unwrap()`/`.expect()` en código que no es test ni `main` de ejemplo.
- Clonar para 'callar al borrow checker' en vez de repensar la propiedad.
- `Arc<Mutex<>>` por defecto cuando el diseño no lo necesita.
- Bloquear el runtime async con I/O síncrona dentro de un `async fn`.

## Testing
- `#[cfg(test)] mod tests` junto al código; `cargo test`. Para errores, asierta sobre el caso
  real, no sobre el mensaje exacto si es frágil. Lo mecánico verifícalo con `cargo check`/`clippy`.

## GROUNDING crossterm (API REAL — verificada, NO la alucines)
`dpx` ya NO usa rustyline: su área de entrada es un editor propio en modo raw sobre `crossterm`
0.29 (vía el re-export `termimad::crossterm`; NO la añadas como dependencia directa). El editor
vive en `src/cli/editor.rs`. Hechos exactos de esa versión:

- Eventos: `event::read() -> io::Result<Event>` (bloqueante) y
  `event::poll(Duration) -> io::Result<bool>`. `Event` tiene variantes `Key(KeyEvent)`,
  `Paste(String)` (solo Unix con bracketed paste), `Resize(u16, u16)`, etc.
- `KeyEvent` tiene CAMPOS NOMBRADOS: `code: KeyCode`, `modifiers: KeyModifiers`,
  `kind: KeyEventKind`, `state`. En Windows llegan eventos de Press Y Release: SIEMPRE filtra
  `kind == KeyEventKind::Release` o cada tecla se procesa dos veces.
- `KeyModifiers` es un bitflags: compara con `==` o `.contains(...)`; NO lo uses como patrón
  literal en un `match` (usa guards: `(KeyCode::Char('c'), m) if m == KeyModifiers::CONTROL`).
- Modo raw: `terminal::enable_raw_mode()` / `disable_raw_mode()`. En raw mode `\\n` NO devuelve
  el carro: imprime `\\r\\n`. Restaura SIEMPRE el modo al salir (dpx usa un guard con `Drop`).
- Cursor/limpieza: comandos `cursor::MoveUp/MoveDown/MoveToColumn`,
  `terminal::Clear(ClearType::FromCursorDown)`, encolados con `queue!(out, ...)` + `flush()`.
- En Windows crossterm lee los `INPUT_RECORD`s nativos de la consola: Shift+Enter llega
  distinguible SIN trucos (no hace falta win32-input-mode). Un PEGADO llega como ráfaga de
  key events; dpx lo detecta con `event::poll(Duration::ZERO)` justo tras una tecla (ver
  `collect_burst` en `src/cli/editor.rs`). En Unix el evento `Paste` requiere
  `EnableBracketedPaste` (feature `bracketed-paste`).";
