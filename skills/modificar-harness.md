---
name: modificar el harness (doctrina/prompt de dpx)
focus: dpx
cuando: "USAR cuando: agregar o cambiar una regla, lección o doctrina del comportamiento de dpx (el system prompt / focus pack), o corregir una falla recurrente del agente."
---
El "harness" es lo que define cómo se comporta dpx. Va compilado en el binario, así
que un cambio NO surte efecto hasta reinstalar.

1. **¿Dónde va la regla?**
   - Doctrina GENERAL (aplica a todos los stacks): `src/focus/mod.rs` (las secciones
     de método: economía de rondas, cambio mínimo, errores recurrentes…).
   - Lección ESPECÍFICA de trabajar sobre dpx: `src/focus/dpx.rs`.
   - Conocimiento de un stack: el focus pack correspondiente (`spring_boot.rs`, etc.).
2. **Sé concreto y corto**: una regla imperativa con el porqué y el síntoma real que
   evita ("pasó de verdad: …"). Las reglas vagas se ignoran.
3. **No dupliques**: busca si ya existe una regla parecida y refínala en vez de
   añadir una quinta que diga lo mismo.
4. **Reinstala**: `cargo install --path . --force` (o el usuario corre `/actualizar`).
   OJO: falla si hay un dpx corriendo (binario en uso, os error 5 en Windows).
5. **Valida** con el banco de pruebas (`eval/run-eval.sh`): corre la tarea que picaba
   la falla y confirma que ahora la caza.
