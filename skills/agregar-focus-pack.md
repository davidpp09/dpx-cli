---
name: agregar un focus pack de un stack nuevo
focus: dpx
cuando: "USAR cuando: agregar soporte para un stack/lenguaje nuevo (un focus pack: Go, Kotlin, Vue, etc.). NO usar para skills sueltos ni para tocar un pack que ya existe."
---
Un focus pack es el conocimiento de dominio que se inyecta al system prompt según
el stack. Para añadir uno (p.ej. `go`):

1. **Archivo** `src/focus/go.rs` con `pub const SKILLS: &str = "..."` — el dominio.
   Incluye SIEMPRE un bloque de VERSIONES ACTUALES (autoritativo, junio 2026) que
   gane sobre la memoria del modelo; no inventes números de versión.
2. **Declara el módulo**: `mod go;` en `src/focus/mod.rs` (junto a los otros).
3. **Catálogo**: añade su `Focus { id, name, tagline }` en `catalog()` (el `id` es
   lo que el usuario elige, p.ej. `"go"`).
4. **Inyección**: añade el caso a `domain_skills()` (`"go" => Some(go::SKILLS)`).
5. **Detección** (opcional): enséñale a `fs::detect_stack` a reconocer el stack por
   sus archivos raíz (p.ej. `go.mod`).
6. **Verifica** clippy estricto + tests; reinstala con `/actualizar` (el prompt se
   compila en el binario).
