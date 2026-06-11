# Reglas de Desarrollo para AI Agent CLI

- **Lenguaje:** Rust estricto.
- **Frameworks Core:** `tokio` (asincronismo), `clap` (CLI), `rig-core` (Orquestación IA).
- **Manejo de Errores:** Prohibido el uso de `.unwrap()` o `.expect()` en código de producción. Propaga errores usando el tipo `Result` y el operador `?`.
- **Arquitectura:** Diseña pensando en un Model Router (Enrutador Multi-Modelo). La aplicación debe poder inicializar múltiples clientes de `rig-core` simultáneamente. Modelos en uso: **DeepSeek (cerebro mentor)**, Groq (respuestas rápidas), Mistral (salida estructurada) y Gemini (contexto largo). No hay clave de Anthropic.
- **Producto:** `dpx` es un mentor senior de desarrollo en la terminal: enseña y explica el porqué, deja que el usuario escriba el código, y se hiper-enfoca por stack mediante *Focus Packs* (primero Spring Boot). Dominio: código, sistemas, prácticas, enseñanza y aprendizaje. (Sin deploy.)
- **Persistencia:** Carpeta `.dpx/` en el proyecto activo. El `context.md` (estado del proyecto + aprendizaje del usuario + próximos pasos + resumen) se regenera al cerrar la sesión; la transcripción se guarda por turno en `.dpx/sessions/`.
