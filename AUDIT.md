# Audit de dpx — qué es, para qué sirve y cómo hacerlo potente y barato

> Fecha: 2026-06-19 · Estado del repo: 143 tests verdes, clippy estricto limpio,
> build release instalado. Este documento se basa en lo que **compila hoy** en
> `src/`, no en promesas.

---

## 1. Qué es dpx (conclusión)

Un **mentor-agente de terminal hiper-enfocado por stack**, con DeepSeek como único
cerebro y persistencia local en `.dpx/`.

- **Cerebro:** Model Router de un solo proveedor (DeepSeek), dos tiers:
  `deepseek-v4-pro` (cerebro) y `deepseek-v4-flash` (subagentes, resúmenes,
  prompts baratos). Solo `learn` usa thinking (`reasoning_effort: max`);
  `code`/`hack` corren sin thinking para respuesta inmediata. (`agent/router.rs`)
- **Tres modos reales:** `code` (agente que construye), `hack` (rápido con
  criterio), `learn` (tutor socrático). No son flags cosméticos: cambian
  identidad, temperatura y thinking.
- **7 focus packs** (`focus/`): Spring Boot, React, Node, Python, Rust con
  conocimiento dedicado; Gradle y `dpx` (auto-edición) parciales.
- **13 tools nativas** (function calling, `agent/tools.rs`): read_file (con
  rango offset/limit), search_project, write_file, edit_file, delete_file,
  run_command, web_search, web_fetch, spawn_agent, git_status/diff/log/commit.
- **Calidad de ingeniería:** green-gate (no cierra turno con build/test en rojo),
  safety tiers (Forbidden/Dangerous/Safe), edición de 3 capas (exacto →
  CRLF-tolerante → fuzzy-indent), undo del último turno.
- **Token ledger real** (`token.rs`): cuenta tokens exactos del proveedor,
  distingue los servidos por caché (~10x más baratos), estima costo en USD.
- **Enseñanza:** skills (visto/practicando/dominado) + curriculum por stack +
  repaso espaciado + rachas.
- **UX de input** (crossterm): multilínea real (Shift+Enter), autocompletado
  inline (ghost para `@archivo` y `/comando`), resaltado de comandos/refs, barra
  de atajos al pie con medidor de contexto en vivo.

**La tesis:** dpx **no** compite —ni debe— en *generación bruta de código* contra
Claude Code (modelos más capaces, TUI pulida por Anthropic). Su foso es la
**dimensión pedagógica + enfoque por stack + costo**. Ahí es el mejor; ahí debe
doblar la apuesta.

---

## 2. Score

### ¿Para qué sirve? (1-10, verificado contra el código)

| Caso de uso | Score | Por qué |
|---|---|---|
| Aprender un stack guiado (learn + curriculum + repaso) | **9** | Único en su clase. |
| Mentoría / explicar el porqué | **9** | Es su identidad core. |
| Code review pedagógico (`/revisar`) | **8** | Senior que enseña al revisar. |
| Auto-edición (dpx se edita a sí mismo) | **8** | Focus pack `dpx` + `/actualizar`. |
| Tareas acotadas con criterio (code/hack) | **7** | Sólido para el 80%. |
| Brainstorm / planificación (comité hack) | **7** | 4 roles evalúan antes de construir. |
| Generación masiva / multi-repo / scaffolding grande | **4** | No es su terreno (4 rondas/turno). |

### Accesibilidad (qué tan barato / fácil / abierto)

| Dimensión | Score | Por qué |
|---|---|---|
| Costo por sesión | **9** | DeepSeek + caché 10x + flash para subtareas. |
| Requisitos | **9** | 1 API key, sin nube, todo local en `.dpx/`. |
| Curva de entrada | **8** | Wizard `init`, 3 modos, comandos en español. |
| UX de input | **8** | Multilínea, ghost autocomplete, resaltado, barra al pie. |
| Privacidad / offline | **6** | El código sí viaja a la API de DeepSeek. |
| Multiplataforma | **7** | Windows-first; corre en *nix, menos probado. |

**Veredicto global: 8/10 como "mentor de programación en terminal barato".**
6/10 si se midiera como "agente de código generalista" — pero esa no es la pelea.

---

## 3. Super potente PERO super barato — estrategia

**La potencia no sale de un modelo más caro. Sale de la ingeniería del harness:**
mejor contexto, mejor orquestación, disciplina de caché, y delegar lo barato a
flash. DeepSeek se queda. Levers ordenados por **impacto / esfuerzo**:

### 🔴 Alto impacto, bajo riesgo

1. **Disciplina de caché — lo #1 que mueve la factura.** DeepSeek cobra el prefijo
   cacheado ~10x menos. La regla de oro: mantener el prefijo (system prompt +
   tools + historial viejo) estable byte a byte. → **[HECHO]** ver §4.

2. **Dieta del system prompt** (`focus/mod.rs`). → **[YA LEAN]** Al revisar los
   constantes, el prompt YA está a dieta: CODE_IDENTITY + SHARED_TOOLS +
   AGENTIC_SKILLS + addendum ≈ **1500 chars** (cerca de la meta de ~2000 de
   PLAN.md). No hay solape real entre `SHARED_TOOLS` (qué tools hay) y
   `AGENTIC_SKILLS` (cómo decidir). Cortar 40% más sería quitar instrucciones que
   cargan peso → degradaría al mentor. Si se intenta, hacerlo MIDIENDO (sesión
   real antes/después), no a ciegas.

3. **Lecturas por rango.** → **[YA EN EL PROMPT]** `read_file` capa a 2500 líneas
   con aviso de "faltan N · llama con offset=X", y `AGENTIC_SKILLS` ya dice
   *"search_project antes de leer media codebase"* y *"pide TODAS las lecturas en
   UN turno"*. La higiene de contexto está madura (search capa a 100, tree/detect
   capados).

### 🟠 Alto impacto, esfuerzo medio

4. **Empujar más trabajo a FLASH.** → **[YA HECHO]** Verificado: PRO solo en el
   cerebro principal; subagentes, comité (4 roles + síntesis), compactación,
   resúmenes y clasificación de delegación YA corren en FLASH. (La clasificación
   prueba flash primero con fallback a keywords — `recall.rs`.) Candidato futuro:
   resumir archivos largos con flash *antes* de inyectarlos al cerebro pro.

5. **Planificación proactiva.** Ya existe `dpx:plan` y `AGENTIC_SKILLS` lo nudgea
   (*"dpx:plan si hay 2+ archivos"*). Hacerlo MÁS automático es posible pero es un
   nudge de prompt (no verificable sin sesión real).

### 🟡 Potencia pura (costo casi nulo)

6. **Rondas dinámicas:** OJO — CLAUDE.md fija "máximo 4 rondas/turno" como regla
   de producto. No subir sin decisión explícita; el checkpoint al tope ya evita
   la "muerte a la mitad".
7. **Paralelizar trabajo independiente.** → **[HECHO]** ver §4.2.

---

## 4. Cambios ya aplicados

### Disciplina de caché: elidir solo cuando el contexto pesa (`cli/chat/mod.rs`)

`prune_tool_outputs` elide cuerpos de tool results viejos y voluminosos para no
arrastrar archivos enteros ronda tras ronda. Estaba **bien guardado** (el prefijo
`[salida elidida` evita re-elidir, así que cada salida se elide una sola vez),
pero corría **incondicionalmente cada turno**.

**Problema:** en sesiones cortas (el caso común de un mentor) eso elide salidas y
rompe el caché de contexto *prematuramente*, cuando hay 128k de sobra. Se pagaba
el costo de romper el caché sin necesitar aún el ahorro.

**Fix:** gate por tamaño de contexto. Solo se elide cuando
`estimate_tokens(history) > prune_threshold()` (50% de la ventana = 64k). Por
debajo, el historial NO se toca → el prefijo queda estable → el caché ~10x más
barato pega al máximo. La compactación dura sigue en 75% (96k).

```
sin elidir  ──────────────┬────────── elidir tool outputs ───┬── compactar
 0                        64k (prune_threshold)              96k (compact)   128k
```

**Cómo medirlo:** el `caché %` ya aparece en cada turno y en `/cost`. En sesiones
cortas debería subir notablemente (menos cambios de prefijo).

### 4.2 Comité de hack en paralelo (`cli/chat/committee.rs`, `cli/chat/recall.rs`)

Los 4 roles del comité (juez · product · tech lead · escéptico) evalúan la MISMA
idea de forma independiente, pero corrían **secuenciales** (`for` con `.await`).
Ahora corren **en paralelo** con `future::join_all`, igual que el batch de
`spawn_agent` en `run_turn`. Mismo costo en tokens (4 llamadas flash), pero la
espera baja de "suma de los 4" a "el más lento" (~4x menos).

Para que 4 subagentes concurrentes no pisen sus spinners en la misma línea, se
añadió `run_subagent_quiet` (sin header, sin spinner por ronda, sin línea de
cierre); sus trazas de lectura (`↳ subagente lee…`) sí salen como feedback en
vivo porque son líneas sueltas, no `\r`. La síntesis (que depende de los 4) sigue
después.

---

## 5. Enfoque del producto (no perder el norte)

- **Code/Hack:** ser "bueno" (suficiente para el 80%), no "el mejor". DeepSeek
  v4-pro + function calling + green-gate ya supera el promedio.
- **Learn:** ser "el mejor" sin competencia. Aquí va el 70% del esfuerzo futuro:
  badges, rachas, repaso espaciado, evaluación de nivel, code review pedagógico,
  curriculum guiado.

**Métrica de éxito:** un usuario nuevo pasa de "no sé nada de Spring Boot" a
"hice mi primer endpoint REST con validación y tests" en 3-5 sesiones guiado por
dpx, por centavos.
