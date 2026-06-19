# DPX v1 → v2: Plan de Producto

## Diagnostico actual

dpx es un CLI agent con 3 modos (code/hack/learn), 12 tools nativas, 7 focus packs, 
persistencia en `.dpx/`, token ledger, y un sistema de ensenanza (skills + curriculum). 
151 tests verdes, 0 warnings, build limpio.

---

## Scoring comparativo (1-10)

| Categoria | dpx | Claude Code | opencode |
|-----------|-----|-------------|----------|
| Generacion de codigo | 6 | 9 | 8 |
| Velocidad percibida | 8 | 7 | 7 |
| Tool calling | 7 | 8 | 8 |
| Input UX (multilinea, TUI) | 4 | 9 | 8 |
| Subagentes / delegacion | 6 | 8 | 9 |
| **Ensenanza / mentoria** | **9** | 3 | 2 |
| Conocimiento por stack | 9 | 6 | 6 |
| Seguridad / guardrails | 8 | 7 | 6 |
| Persistencia entre sesiones | 7 | 6 | 5 |
| Costo / eficiencia | 9 | 7 | 7 |
| Multi-proveedor | 2 | 9 | 6 |
| Self-editing / auto-update | 8 | 0 | 0 |
| Tracking de progreso | 8 | 0 | 0 |
| Comite / brainstorm | 7 | 0 | 0 |

**dpx gana en:** ensenanza, enfoque por stack, eficiencia de costos, tracking de progreso.
**dpx pierde en:** input UX, capacidad bruta de codigo, multi-proveedor.

---

## PUNTOS BUENOS — MANTENER

### Identidad unica
1. **Tres modos reales** (code/hack/learn) con personalidad distinta — no flags cosmeticos
2. **Learn mode socratico** — product struggle, pistas graduales, retrieval practice
3. **Focus packs** — conocimiento de dominio inyectado por stack (7 stacks)
4. **Comite de hack** — 4 roles evaluando tu idea antes de construir
5. **Skills tracking** — visto/practicando/dominado con repaso espaciado
6. **Curriculum por stack** — orden sugerido de aprendizaje

### Calidad de ingenieria
7. **Green-gate** — no cierra turno con build/tests en rojo
8. **Cuarentena** — bloquea acciones de texto malformadas (protege de fences rotos)
9. **Safety tiers** — Forbidden/Dangerous/Safe para comandos
10. **3-layer edit strategy** — exacto → CRLF-tolerant → fuzzy indent-tolerant
11. **Token ledger real** — caché %, costo aproximado, presupuesto
12. **Elision de tool outputs** — ahorra contexto sin romper pairing

### UX
13. **Typewriter** — revelado progresivo de respuestas
14. **Syntax highlighting** — bloques de codigo con syntect
15. **Spinner con verbos** — "Pensando…", "Cocinando…", etc.
16. **Gradientes por modo** — azul code, ambar hack, verde learn
17. **Titulo de pestana** — muestra estado en la terminal

### Auto-suficiencia
18. **Self-editing** — el focus pack dpx sabe editarse a si mismo
19. **Auto-update** — `/actualizar` recompila e instala sin cerrar sesion
20. **Stack detection** — detecta el stack del proyecto al iniciar

---

## PUNTOS A MODIFICAR

### Input UX (prioridad critica)
1. **Multilinea real.** `\` al final funciona pero es incomodo. Solucion: 
   - Reemplazar rustyline con un mini TUI en crossterm (como teniamos antes, pero SIMPLIFICADO — solo 300-400 lineas, no 1017).
   - O integrar `termion`/`crossterm` para capturar Shift+Enter.
   - Alternativa: aceptar que `\` es suficiente y mejorar la ayuda visual.

2. **Separacion visual entre turnos.** Actualmente `│` + respuesta + token line + prompt. 
   Deberia haber una linea dim `───` o un espacio mas generoso entre el fin de la 
   respuesta y el siguiente prompt.

3. **El prompt `dpx-(auto-edición) code ▸` es muy largo.** Simplificar a `dpx ▸` 
   con el modo como color de fondo del `▸`. La info de focus/mode puede ir en una 
   barra de estado arriba (una sola vez al inicio, no en cada prompt).

### Sistema de prompts (prioridad alta)
4. **El system prompt sigue siendo grande.** ~2500-3500 chars de identidad + tools + 
   agentic skills + dominio + addendum. Recortar mas:
   - AGENTIC_SKILLS: de ~600 chars a ~350. Quitar lo redundante con SHARED_TOOLS.
   - SHARED_TOOLS: unificar reglas duplicadas.
   - Identidad: 2-3 frases maximo. El modelo ya sabe quien es.
   - Meta: bajar de ~3500 chars a ~2000 chars (~500 tokens → ~300 tokens).

5. **El arbol del proyecto y symbol map se inyectan en CADA turno.** Con caché 99% 
   esto es barato, pero ocupa contexto. Opcion: solo inyectarlos en el primer turno 
   de la sesion, y en turnos siguientes solo si el modelo pide `read_file` de la raiz.

### Delegacion (prioridad media)
6. **`classify_delegation` es un keyword match.** 34 palabras de research, 20 de 
   cambio. Esto falla con preguntas en espanol complejas o ambiguas. Opciones:
   - Usar el modelo FLASH para clasificar (una llamada barata de 10 tokens).
   - O mantener el keyword match pero duplicar patrones con variantes.

7. **Subagentes son secuenciales.** Si el modelo pide 3 `spawn_agent`, corren uno 
   tras otro. Deberian correr en paralelo (`tokio::join!`).

### Learn mode (prioridad media)
8. **El sistema de skills es binario (visto/practicando/dominado).** Podria mejorar:
   - Añadir `fecha_visto`, `fecha_practicando`, `fecha_dominado`.
   - Mostrar progresion temporal en `/progreso` ("hace 3 dias", "la semana pasada").
   - Sugerir automaticamente repaso en sesiones nuevas.

9. **Curriculum no se integra con el sistema de skills.** El `/temario` muestra 
   topics del curriculum, pero el learn mode no los prioriza activamente. 
   El tutor deberia guiar al usuario a traves del curriculum en orden sugerido.

### Herramientas (prioridad baja)
10. **Falta una tool de `web_fetch`.** `web_search` devuelve snippets de DuckDuckGo,
    pero no puede leer el contenido completo de una URL. Anadir `web_fetch` 
    (como tiene opencode) permitiria leer documentacion directamente.

11. **`spawn_agent` perdio los roles.** Antes tenia researcher, reviewer, debugger,
    architect, etc. Eran utiles para darle personalidad al subagente. Recuperarlos 
    sin el modulo `roles` — solo como strings en el system prompt del subagente.

---

## COSAS A ELIMINAR

1. **Text-based action parsing (`dpx:write`, `dpx:edit`, `dpx:run`).** Es fragil, 
   requiere cuarentena, y ya tenemos function calling nativo. Mantener solo 
   `dpx:plan` (checklist) que no tiene equivalente nativo. Eliminar el parser de 
   texto para writes/edits/reads/runs/searches. Esto simplifica `fs/mod.rs` 
   drasticamente (~300 lineas menos).

2. **`ui::format_input_status`** — ya no se usa (reemplazado por el tag en el prompt).

3. **`ui::truncate_visible`, `ui::real_term_width`, `ui::fmt_elapsed`** — funciones 
   muertas. Limpiar.

4. **`ui::diagnostic_panel`** — no se usa desde que quitamos el modulo diagnostic. 
   Limpiar o recuperar como feedback post-compilacion.

5. **`focus::builtin_playbooks` y `focus::general_playbooks` y PLAYBOOKS en cada 
   focus pack.** Son datos que alimentaban el sistema de agent_skill (eliminado). 
   Son ~54 playbooks (~2000 lineas de constantes) que no se usan. Fuera.

6. **El texto de bienvenida** `@archivo para leer codigo · \ al final para multilinea` 
   es muy verboso. Reemplazar por una linea: `usa /ayuda para ver los comandos`.

---

## LO QUE FALTA — NUEVAS FEATURES

### Ensenanza (diferenciador real)
1. **Racha / streak.** Contar sesiones consecutivas. "Llevas 5 sesiones seguidas 
   aprendiendo Spring Boot." Motiva.

2. **Badges / logros.** "Primer endpoint REST", "10 conceptos dominados", 
   "Primer refactor seguro". Pequenos hitos visibles en `/progreso`.

3. **Repaso espaciado automatico.** Al iniciar sesion en modo learn, dpx sugiere 
   conceptos que toca repasar segun el algoritmo de spaced repetition. Sin que el 
   usuario tenga que pedirlo.

4. **"Cuentame que sabes de X".** El tutor evalua el nivel real del usuario antes 
   de ensenar. No asume que sabe nada, no asume que no sabe nada. Pregunta.

5. **Code review con ensenanza.** El usuario pega su codigo (con `\` multilinea) 
   y dpx lo revisa como un senior: "aqui esta bien", "esto podria ser mejor asi", 
   "este patron se llama X y se usa cuando Y".

### Code mode (potencia bruta)
6. **Planificacion automatica de tareas.** Antes de empezar un turno complejo, 
   dpx emite un `dpx:plan` y lo usa como guia. Esto ya lo hace, pero podria ser 
   mas proactivo: detectar automaticamente tareas multi-archivo y planificar.

7. **Modo "arreglar este error".** El usuario pega un error de compilacion y dpx 
   lo diagnostica sin necesidad del modulo diagnostic (que era overkill). Solo 
   leer el error, buscar en el codigo, proponer fix.

8. **Ejecutar tests automaticamente.** Si el modelo escribe codigo, dpx corre los 
   tests sin preguntar (igual que hace con build). Extender el green-gate.

### UX
9. **Preview de cambios inline.** Antes de confirmar un write/edit, mostrar el 
   diff con sintaxis coloreada, no solo +/-.

10. **Undo del ultimo turno.** Simple: guardar una copia de cada archivo antes de 
    tocarlo, permitir `/undo` para revertir. No el sistema de checkpoint por 
    sesion (que borramos), sino solo el turno inmediato.

11. **Barra de progreso en compilacion.** Cuando dpx ejecuta build/test, mostrar 
    algo mas vivo que lineas sueltas.

### Infraestructura
12. **Configuracion de API key desde el wizard.** `dpx init` deberia preguntar 
    por la API key si no la detecta, y guardarla en `.env`.

13. **Soporte para proveedores alternativos.** No es urgente (DeepSeek es solido 
    y barato), pero preparar la arquitectura para que acepte OpenAI/Anthropic 
    en el futuro sin reescribir el router. Solo requiere añadir un enum Provider 
    en config y construir el cliente adecuado.

---

## ENFOQUE DEL PRODUCTO

**dpx NO debe competir con Claude Code en generacion bruta de codigo.** Claude Code 
tiene modelos mas capaces (Claude 4.x) y una integracion TUI pulida por Anthropic. 
Competir ahi es perder.

**dpx SI debe ser el mejor mentor de programacion en la terminal.** Nadie mas hace esto:
- Claude Code: explica pero no ensena. No tiene curriculum, no trackea progreso, 
  no tiene modo learn.
- opencode: agente generico de ingenieria. No tiene dimension pedagogica.
- Copilot/Cursor: autocompletado, no ensenanza.

**El camino es doble:**
1. **Code/Hack mode** — ser "bueno" compitiendo (no el mejor, pero suficientemente 
   solido para el 80% de tareas). DeepSeek v4-pro es capaz. Con function calling 
   nativo y green-gate, dpx code mode ya es mejor que el promedio.
2. **Learn mode** — ser "el mejor" sin competencia. Aqui es donde dpx debe invertir 
   el 70% del esfuerzo futuro. Badges, rachas, repaso espaciado, evaluacion de 
   nivel, code review con ensenanza, curriculum guiado.

**Metricas de exito para v2:**
- Un usuario nuevo puede pasar de "no se nada de Spring Boot" a "hice mi primer 
  endpoint REST con validacion y tests" en 3-5 sesiones guiado por dpx.
- Un usuario intermedio puede recibir una code review de su codigo y aprender 
  2-3 conceptos nuevos por sesion.
- dpx learn mode tiene mas features de ensenanza que cualquier otro CLI agent.

---

## Resumen ejecutivo

| Que | Accion |
|-----|--------|
| Input UX | Reconstruir mini TUI en crossterm (~400 lineas) para multilinea real |
| System prompt | Recortar 40% mas (~2000 chars total) |
| Text parsing | Eliminar `dpx:write/edit/run/read/search`, solo function calling |
| Playbooks | Eliminar 54 constantes muertas (~2000 lineas) |
| Delegacion | Mejorar clasificacion + paralelizar subagentes |
| Learn mode | Badges, rachas, repaso espaciado, evaluacion de nivel |
| Code mode | Auto-test en green-gate, planificacion proactiva |
| Web fetch | Tool nueva para leer documentacion |
| Undo | Simple: solo el ultimo turno |
| API key wizard | `dpx init` detecta y configura la key |
