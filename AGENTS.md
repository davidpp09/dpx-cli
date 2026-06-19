# Reglas de Desarrollo para AI Agent CLI

- **Lenguaje:** Rust estricto.
- **Frameworks Core:** `tokio` (asincronismo), `clap` (CLI), `rig-core` (Orquestacion IA).
- **Manejo de Errores:** Prohibido el uso de `.unwrap()` o `.expect()` en codigo de produccion. Propaga errores usando el tipo `Result` y el operador `?`. Los tests si pueden usar `.unwrap()`.
- **Arquitectura:** Model Router con DeepSeek como unico proveedor. Dos tiers: `deepseek-v4-pro` (cerebro principal) y `deepseek-v4-flash` (subagentes, resumenes). Solo el modo `learn` usa `reasoning_effort: max`; `code` y `hack` usan modo sin thinking para respuestas rapidas.
- **Producto:** `dpx` es un mentor senior de desarrollo en la terminal: ensena y explica el porque, deja que el usuario escriba el codigo, y se hiper-enfoca por stack mediante *Focus Packs* (primero Spring Boot). Dominio: codigo, sistemas, practicas, ensenanza y aprendizaje. (Sin deploy.)
- **Persistencia:** Carpeta `.dpx/` en el proyecto activo. El `context.md` (estado del proyecto + aprendizaje del usuario + proximos pasos + resumen) se regenera al cerrar la sesion; la transcripcion se guarda por turno en `.dpx/sessions/`.
- **Performance:** Maximo 4 rondas por turno (antes 8). Los modos code/hack corren sin thinking (respuesta inmediata). El system prompt debe ser conciso: cada palabra cuenta en el contexto de 128k tokens.
