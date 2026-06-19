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

/// Playbooks EMPOTRADOS de Rust: pasos A→B de las tareas que más se repiten,
/// para que dpx no explore a ciegas y aplique la convención correcta.
/// Se cargan cuando el focus activo es `rust`. (nombre, cuándo, pasos).
#[allow(dead_code)]
pub const PLAYBOOKS: &[(&str, &str, &str)] = &[
    (
        "crear CLI con clap",
        "USAR cuando: crear/agregar un CLI, comando, subcomando, flag, argumento, --verbose, parseo de args. Palabras: clap, CLI, comando, argumento, flag. NO para tests ni config.",
        "1. Define los args con `#[derive(Parser)]` (clap derive) + `#[command(name, about)]`. Usa `#[arg(long, short)]` para flags.\n\
         2. Un subcomando = un `enum` con `#[derive(Subcommand)]`, cada variante con su struct de args.\n\
         3. En `main()`: `let cli = Cli::parse();` y despacha con `match` o `cli.command`.\n\
         4. NUNCA parsees args a mano con `std::env::args()` si ya tienes clap.\n\
         5. Verifica con `cargo run -- --help` y `cargo test`.",
    ),
    (
        "manejo de errores (anyhow / thiserror)",
        "USAR cuando: manejar errores, Result, anyhow, thiserror, propagar con ?, error custom, .context(). Palabras: anyhow, thiserror, Result, error, contexto, ?. NO para tests ni lógica de negocio pura.",
        "1. Para CLIs/aplicaciones: `anyhow::Result<T>` + `?` para propagar; `.context(\"...\")` o `.with_context(|| format!(\"...\"))` para añadir contexto.\n\
         2. Para LIBRERÍAS (errores tipados): `#[derive(Error, Debug)] enum MiError { ... }` con `thiserror`. Cada variante lleva `#[error(\"...\")]`.\n\
         3. NUNCA uses `.unwrap()`/`.expect()` en producción: propaga con `?` o maneja con `match`.\n\
         4. Convierte entre errores con `.map_err(|e| ...)?` o `impl From<OtherError> for MiError`.\n\
         5. Verifica que el error se propaga correctamente con un test que fuerce el caso de fallo.",
    ),
    (
        "tests unitarios",
        "USAR cuando: escribir/agregar tests, probar, test case, assert, test unitario, #[test]. Palabras: test, tests, probar, assert, unit test. NO para integración ni benchmarks.",
        "1. `#[cfg(test)] mod tests { use super::*; }` junto al código que prueba (NO en archivo aparte salvo tests de integración en `tests/`).\n\
         2. Cada caso es `#[test] fn nombre_descriptivo() { ... }`. Usa nombres que digan qué prueba: `fn test_parse_vacio_retorna_err()`.\n\
         3. Aserciones claras: `assert_eq!`, `assert!(matches!(...))`, `assert!(result.is_err())`. Sin lógica compleja en el test.\n\
         4. Un test por caso: feliz + bordes (vacío, null/None, máximos, error). No juntes todo en un solo `#[test]`.\n\
         5. Corre `cargo test` (o `cargo test nombre_del_test` para uno solo) y reacciona a la salida real.",
    ),
    (
        "async con tokio",
        "USAR cuando: async, tokio, spawn, select, runtime, tarea concurrente, async fn, join, sleep. Palabras: async, tokio, spawn, select, concurrente, await. NO para sync puro ni tests sin async.",
        "1. `#[tokio::main]` en `main()` (o `#[tokio::test]` en tests async). El runtime por defecto es multi-thread; para single-thread usa `#[tokio::main(flavor = \"current_thread\")]`.\n\
         2. Lanza tareas con `tokio::spawn(async { ... })`; recolecta con `JoinHandle.await?`. Las tareas deben ser `Send` si usas multi-thread.\n\
         3. Para esperar varias tareas: `tokio::join!(a, b)` (en paralelo) o `tokio::try_join!(a, b)?` (corta en el primer error).\n\
         4. `tokio::select!` para correr varias ramas y tomar la que termine primero; SIEMPRE con un brazo cancelable o timeout (`tokio::time::sleep`).\n\
         5. NUNCA bloquees el runtime con I/O síncrona (`std::fs::read` dentro de async): usa `tokio::fs` o `spawn_blocking`.\n\
         6. Verifica con `cargo test` y, si hay timeouts, con `cargo test -- --nocapture` para ver logs.",
    ),
    (
        "serializar con serde (JSON / TOML / YAML)",
        "USAR cuando: JSON, serde, serializar, deserializar, parsear JSON, leer config, TOML, YAML. Palabras: serde, JSON, serializar, deserializar, toml, config. NO para bases de datos ni SQL.",
        "1. `#[derive(Serialize, Deserialize)]` en structs/enums. Usa `#[serde(rename_all = \"camelCase\")]` si el formato externo usa camelCase.\n\
         2. Para JSON: `serde_json::from_str(&s)?` (leer) y `serde_json::to_string_pretty(&v)?` (escribir). Para TOML: `toml` crate. Para YAML: `serde_yaml`.\n\
         3. Campos opcionales: `Option<T>` + `#[serde(default)]` o `#[serde(skip_serializing_if = \"Option::is_none\")]`. Campos renombrados: `#[serde(rename = \"otro_nombre\")]`.\n\
         4. Valida DESPUÉS de deserializar: serde solo valida tipos, no reglas de negocio. Haz la validación en una fn `validate(&self) -> Result<()>`.\n\
         5. Test: deserializa un string de ejemplo y asierta los campos; serializa y compara con el JSON esperado (usa `serde_json::json!`).",
    ),
    (
        "implementar un trait (Debug, Display, From, custom)",
        "USAR cuando: implementar un trait, Debug, Display, Clone, From, Into, PartialEq, Hash, Error, trait personalizado. Palabras: trait, impl, derivar, derive, Debug, Display, Clone, From. NO para async ni manejo de errores (ya tienen su playbook).",
        "1. Traits estándar que siempre van: `#[derive(Debug, Clone)]` (y `PartialEq, Eq, Hash` si el tipo lo permite).\n\
         2. `Display` manual cuando el debug no basta para el usuario: `impl fmt::Display for Tipo { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, \"...\") } }`.\n\
         3. Conversiones con `From`/`Into`: `impl From<A> for B` (automáticamente da `Into<B> for A`). Prefiere `From` sobre `Into`.\n\
         4. Para traits CUSTOM: un trait con métodos default cuando aplique; el `impl` concreto solo sobrescribe lo necesario.\n\
         5. Test: crea una instancia, llama al método del trait, asierta el resultado. Si implementaste `From`, prueba la conversión ida y vuelta.",
    ),
];
