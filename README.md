<div align="center">
  <h1>🚀 dpx</h1>
  <p><b>Tu mentor senior de desarrollo, directamente en tu terminal.</b></p>

  [![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
  [![Rust](https://img.shields.io/badge/Rust-2024-orange.svg)](https://www.rust-lang.org)
</div>

---

> **dpx** es un mentor de ingeniería de software que vive en tu terminal. No es un autocompletador ni un generador de scaffolding: es un agente que **enseña, explica y te deja escribir a ti**, o bien **toma el teclado y resuelve la tarea** en modo autónomo. Se hiper-enfoca según el stack en el que trabajas y recuerda el contexto de tu proyecto entre sesiones.

```bash
dpx chat                     # el mentor te enseña
dpx code                     # el agente autónomo hace el trabajo
dpx chat --mode hack         # modo rápido, para hackathones
dpx chat --focus spring-boot # enfocado en Spring Boot
dpx chat --brain kimi        # otro cerebro (modelo LLM)
```

---

## 📖 Tabla de contenidos

- [⚡ Instalación](#-instalación)
- [🏗️ Arquitectura](#-arquitectura)
- [🎭 Modos y personas — las combinaciones](#-modos-y-personas--las-combinaciones)
  - [Personas: mentor vs code](#personas-mentor-vs-code)
  - [Modos: pro vs hack](#modos-pro-vs-hack)
  - [Matriz de combinaciones](#matriz-de-combinaciones)
- [🧠 Cerebros (modelos)](#-cerebros-modelos)
- [🎯 Focus Packs (enfoques por stack)](#-focus-packs-enfoques-por-stack)
- [🤖 Modo autónomo (`--auto` / `/auto`)](#-modo-autónomo---auto--auto)
- [⌨️ Comandos del REPL](#-comandos-del-repl)
- [🛠️ Herramientas (function calling)](#-herramientas-function-calling)
- [💰 Tokens, costo y presupuesto](#-tokens-costo-y-presupuesto)
- [↩️ Deshacer y revisar cambios](#️-deshacer-y-revisar-cambios)
- [🔌 Extensibilidad (MCP, comandos y hooks)](#-extensibilidad-mcp-comandos-y-hooks)
- [⚙️ Cómo funciona](#-cómo-funciona)
  - [Ciclo de un turno](#ciclo-de-un-turno)
  - [Verificación automática de build y tests](#verificación-automática-de-build-y-tests)
  - [Mapa de símbolos (repo-map)](#mapa-de-símbolos-repo-map)
  - [Diagnósticos LSP](#diagnósticos-lsp)
  - [Subagentes](#subagentes)
  - [Persistencia y memoria](#persistencia-y-memoria)
  - [Seguridad](#seguridad)
- [📂 Estructura del proyecto](#-estructura-del-proyecto)
- [🔧 Configuración](#-configuración)
- [💻 Desarrollo (hackear dpx)](#-desarrollo-hackear-dpx)
- [⚖️ Licencia](#-licencia)

---

## ⚡ Instalación

```bash
git clone https://github.com/tu/dpx.git
cd dpx
cargo install --path .
```

### Requisitos previos

- Rust toolchain estable (edition 2024, ≥1.96)
- Al menos una API key de LLM en `~/.dpx/.env` o en un `.env` del proyecto:

```env
DEEPSEEK_API_KEY=sk-...
MOONSHOT_API_KEY=sk-...    # Kimi
OPENROUTER_API_KEY=sk-...  # Qwen
```

> [!IMPORTANT]
> **Sin API key, dpx arranca pero no puede responder**: el primer cerebro con key será el activo.

### Inicializar un proyecto

```bash
cd mi-proyecto
dpx init        # wizard paso a paso: detecta stack, elige cerebro, modo y auto
```

Esto crea `.dpx/config.toml` con los defaults del proyecto. Después, `dpx` o `dpx chat` arranca directo con esa config.

---

## 🏗️ Arquitectura

```text
                   ┌──────────────────────────┐
                   │         dpx CLI          │
                   │  (clap: chat/code/init)  │
                   └────────────┬─────────────┘
                                │
              ┌─────────────────┼─────────────────┐
              │                 │                 │
     ┌────────▼───────┐ ┌──────▼───────┐ ┌───────▼───────┐
     │  Model Router  │ │ Focus Packs  │ │  Persistencia │
     │ (DeepSeek/Kimi │ │ (Spring Boot,│ │  .dpx/        │
     │  /Qwen)        │ │  React, Rust,│ │  context.md   │
     │                │ │  Python, ...)│ │  sessions/    │
     └────────┬───────┘ └──────┬───────┘ └───────┬───────┘
              │                │                 │
              └────────────────┼─────────────────┘
                               │
                    ┌──────────▼──────────┐
                    │    System Prompt    │
                    │  (identidad persona │
                    │   + skills dominio  │
                    │   + modo actitud    │
                    │   + memoria .dpx)   │
                    └──────────┬──────────┘
                               │
                    ┌──────────▼──────────┐
                    │   Loop agéntico     │
                    │  (turnos con tools  │
                    │   + confirmaciones) │
                    └─────────────────────┘
```

---

## 🎭 Modos y personas — las combinaciones

`dpx` tiene DOS ejes de configuración que se combinan: **persona** y **modo**. El resultado son cuatro formas de trabajar distintas.

### Personas: mentor vs code

| Persona | Qué hace | Cuándo usarla |
|:---|:---|:---|
| 🎓 **mentor** (default) | Enseña, explica el porqué, te deja escribir a ti el código. No genera archivos completos salvo que lo pidas explícitamente. | Aprender, entender decisiones, revisar código, diseño. |
| 🧑‍💻 **code** | Agente autónomo: escribe, compila, ejecuta, corrige. Itera hasta que la tarea funciona. | Tareas hechas, implementar features, corregir bugs, refactors. |

Se activa con `--persona` (CLI) o los comandos `/mentor` y `/code` en el REPL.

```bash
dpx chat                           # mentor (default)
dpx code                           # agente autónomo
dpx code --mode hack --auto        # agente rápido y sin preguntar
```

### Modos: pro vs hack

| Modo | Actitud | Temperatura |
|:---|:---|:---|
| 👔 **pro** (default) | Metódico: arquitectura primero, cada decisión explicada, tests incluidos, señala deuda técnica. | `0.4` |
| ⚡ **hack** | Rápido: defaults sensatos, mínimo boilerplate, demo funcionando YA. Enseña en una línea. | `0.7` |

Se activa con `--mode` (CLI) o `/mode pro|hack` en el REPL.

### Matriz de combinaciones

```text
                 MENTOR (enseña)              CODE (hace)
              ┌───────────────────────┬──────────────────────────┐
   PRO        │  "Te explico por qué   │  "Implemento, compilo,  │
   (metódico) │   conviene X sobre Y;  │   corrijo y te entrego  │
              │   ahora escríbelo tú"  │   funcionando con tests" │
              ├───────────────────────┼──────────────────────────┤
   HACK       │  "En prod sería X,    │  "CRUD listo, H2 en RAM, │
   (rápido)   │   pero ahora simpli-  │   endpoints andando en   │
              │   fico así para salir" │   5 minutos, sin tests"  │
              └───────────────────────┴──────────────────────────┘
```

#### Ejemplos de comandos

```bash
# El caso clásico: mentor pro enseña Spring Boot
dpx chat --focus spring-boot

# Hackathon: agente rápido y autónomo
dpx code --mode hack --auto

# Aprendizaje: el mentor explica en detalle
dpx chat --mode pro --brain deepseek

# Producción de código: el agente resuelve solo
dpx code --brain kimi --auto

# Stack específico: mentor Python en modo hack
dpx chat --focus python --mode hack
```

---

## 🧠 Cerebros (modelos)

`dpx` usa un **Model Router**: tres cerebros intercambiables en caliente, cada uno con su fuerte. El router construye el agente con el system prompt y la temperatura correcta según el modo.

| Cerebro | Proveedor | Fuerte | Ventana | API Key |
|:---|:---|:---|:---|:---|
| 🐋 **DeepSeek** (V4 Pro) | DeepSeek | Principal · razonamiento profundo · tool-calling nativo | 128k | `DEEPSEEK_API_KEY` |
| 🌙 **Kimi** (K2.5) | Moonshot | Agéntico sólido · contexto largo | 256k | `MOONSHOT_API_KEY` |
| 🤖 **Qwen** (Coder) | OpenRouter | Código · muy barato | 256k | `OPENROUTER_API_KEY` |

> [!TIP]
> Cambio en caliente desde el REPL: `/brain kimi`. Si el cerebro activo falla (sin saldo, saturado), dpx degrada automáticamente al siguiente con API key.

### Thinking en DeepSeek

En modo `pro`, DeepSeek razona con `reasoning_effort: max`; en `hack` usa `high` (más rápido). Kimi y Qwen no tienen este parámetro.

---

## 🎯 Focus Packs (enfoques por stack)

Cada Focus Pack inyecta conocimiento de dominio en el system prompt del modelo: versiones exactas, buenas prácticas, errores comunes y herramientas propias de ese ecosistema.

| Pack | Stack | Se activa con |
|:---|:---|:---|
| 🌱 `spring-boot` | Backend Java/Spring Boot | `--focus spring-boot` |
| ⚛️ `react` | Frontend React (Vite, TanStack Query, RTL) | `--focus react` |
| 🟩 `node` | Backend Node.js (Fastify/Express, zod) | `--focus node` |
| 🐍 `python` | Backend Python con FastAPI | `--focus python` |
| 🦀 `rust` | Sistemas y CLIs en Rust | `--focus rust` |
| 🐘 `gradle` | Proyecto JVM con Gradle (genérico) | `--focus gradle` |

Si no pasas `--focus`, `dpx` **detecta el stack automáticamente** analizando los archivos del proyecto (`pom.xml`, `package.json`, `Cargo.toml`, etc.). Si no reconoce nada, arranca como mentor general.

---

## 🤖 Modo autónomo (`--auto` / `/auto`)

El modo autónomo tiene **cuatro niveles acumulativos**: cada uno relaja una capa más de confirmaciones. Se controla con `--auto <nivel>` (CLI) o `/auto <nivel>` en el REPL (`/auto` sin argumento alterna entre `all` y `off`).

| Nivel | Qué hace sin preguntar |
|:---|:---|
| `off` (default) | Nada: cada acción se confirma. |
| `reads` | Lecturas y búsquedas (ya eran libres) — útil como punto de partida. |
| `writes` | + escrituras y ediciones de archivos (el diff se muestra igual). |
| `all` ⚡ | + comandos **seguros**. Además, tras escribir código corre la **suite de tests** y se autocorrige. |

Las puertas de seguridad se mantienen **siempre**, incluso en `all`:

| Tipo de acción | ¿Pregunta aunque esté en `all`? |
|:---|:---|
| `write_file` que trunca un archivo grande (>40%) | ✅ **Sí** *(guard anti-truncado)* |
| `write_file` que sobrescribe un archivo grande (≥200 líneas) | ✅ **Sí** *(la doctrina prefiere `edit_file`)* |
| `run_command` peligroso (`rm -rf`, `git reset --hard`) | ✅ **Sí** *(hay que reescribir la 1ª palabra)* |
| `run_command` prohibido (`format`, `shutdown`, `mkfs`) | 🚫 Bloqueado siempre |
| `git_commit`, `delete_file` (mutan el repo) | ✅ **Sí** |

> [!TIP]
> En `all`, el loop agéntico se auto-extiende hasta 32 rondas y, si pones un `/budget`, se pausa al llegar al tope de tokens. Con `/undo` puedes revertir cualquier cosa que dpx haya tocado sin miedo.

---

## ⌨️ Comandos del REPL

Dentro de una sesión, el prompt entiende estos comandos slash:

| Comando | Acción |
|:---|:---|
| `/help` | Muestra esta ayuda |
| `/status` | Estado: config, cerebros, memoria, tokens |
| `/models` | Lista los cerebros y cuál tiene API key |
| `/cost` | Tokens reales gastados en la sesión + % de caché y costo aprox |
| `/budget [N]` | Tope de tokens de la sesión (ej. `/budget 100k`); `off` lo quita |
| `/diff` | Muestra todo lo que dpx cambió en la sesión (base vs. actual) |
| `/undo` | Deshace los cambios de archivos del último turno de dpx |
| `/clear` | Reinicia la conversación (olvida la sesión) |
| `/compact` | Resume la charla para liberar contexto (también automático) |
| `/context` | Muestra la memoria guardada de `.dpx/context.md` |
| `/focus [id]` | Cambia de stack (sin id: lista disponibles) |
| `/mode pro\|hack` | Cambia actitud |
| `/brain deepseek\|kimi\|qwen` | Cambia de modelo |
| `/mentor` | Activa persona mentor (enseña) |
| `/code` | Activa persona code (agente autónomo) |
| `/auto [off\|reads\|writes\|all]` | Nivel de autonomía (sin arg: alterna all/off) |
| `/update` | Recompila e instala dpx desde el repo actual |
| `/salir` | Termina la sesión y guarda el contexto |

*También puedes referenciar archivos con `@ruta/al/archivo.java` y el mentor los leerá (con autocompletado por Tab). Y puedes definir tus propios comandos `/loquesea` en `.dpx/commands.toml` — ver [Extensibilidad](#-extensibilidad-mcp-comandos-y-hooks).*

---

## 🛠️ Herramientas (function calling)

`dpx` expone estas herramientas nativas al modelo (preferidas sobre los bloques de texto `dpx:*`):

| Herramienta | Función |
|:---|:---|
| 📄 `read_file` | Leer archivos del proyecto |
| 🔍 `search_project` | Buscar texto en todos los archivos |
| 🌐 `web_search` | Buscar en DuckDuckGo (gratis, sin API key) |
| 🧩 `spawn_agent` | Lanzar un subagente de investigación aislado (solo lectura) |
| 🩺 `lsp_diagnostics` | Diagnósticos reales de un archivo vía language server |
| ✏️ `write_file` | Crear/sobrescribir archivos |
| ✂️ `edit_file` | Editar fragmentos con SEARCH/REPLACE literal |
| 🗑️ `delete_file` | Borrar archivos |
| 💻 `run_command` | Ejecutar comandos de shell |
| 📊 `git_status` | Estado del repo (solo lectura) |
| 📉 `git_diff` | Diff del working tree (solo lectura) |
| 📜 `git_log` | Últimos commits (solo lectura) |
| 💾 `git_commit` | Crear commit (MUTA, pide confirmación) |

> [!NOTE]
> Además, dpx puede cargar herramientas externas vía **MCP** (Model Context Protocol) y exponerlas al modelo como si fueran propias. Ver [Extensibilidad](#-extensibilidad-mcp-comandos-y-hooks).

---

## 💰 Tokens, costo y presupuesto

dpx mide el **consumo real de tokens** que reporta la API (no una estimación), incluyendo el porcentaje servido desde el **caché de contexto** — la métrica accionable, porque un caché alto puede salir ~10× más barato.

- Tras cada turno verás una línea con `X in · Y out · caché Z% · ~$N` (delta del turno).
- `/cost` muestra el acumulado de la sesión, con % de caché y costo aproximado.
- `/budget 100k` pone un tope de tokens; al superarlo, el modo auto **deja de auto-extenderse** y vuelve a preguntar (`/budget off` lo quita, `/budget` muestra el estado).

La ventana de contexto se ajusta al cerebro activo (DeepSeek 128k · Kimi/Qwen 256k) y dpx **compacta automáticamente** la conversación al llegar al 75%.

Antes de eso, una **compactación ligera** actúa en cada turno: el cuerpo de los resultados de herramienta **viejos y voluminosos** (archivos leídos o salidas de comandos de rondas anteriores) se sustituye por un stub corto — sin borrar mensajes ni romper el emparejamiento `tool_call`/`tool_result`. Así no arrastras archivos enteros ronda tras ronda y la sesión dura más antes de necesitar la compactación completa.

---

## ↩️ Deshacer y revisar cambios

Antes de modificar cualquier archivo, dpx guarda su contenido anterior (en memoria, por sesión). Esto hace que el modo `/auto all` sea usable sin miedo:

- **`/undo`** revierte todos los cambios de archivos del **último turno** de dpx: restaura lo que existía y borra lo que creó nuevo. **No toca git ni tus propios cambios** — solo deshace lo que escribió dpx.
- **`/diff`** muestra **todo** lo que dpx ha cambiado en la sesión (línea base vs. disco actual), para revisarlo antes de confiar o commitear. Solo lectura.

---

## 🔌 Extensibilidad (MCP, comandos y hooks)

dpx se extiende por proyecto desde la carpeta `.dpx/`, sin recompilar:

### Servidores MCP — `.dpx/mcp.toml`

dpx actúa como **cliente MCP**: arranca servidores externos, hace el handshake JSON-RPC 2.0, descubre sus herramientas y las fusiona con las nativas (con namespace `mcp__<server>__<tool>`) para que el modelo las use como propias.

### Comandos personalizados — `.dpx/commands.toml`

Define tus propios comandos slash. Cada uno inyecta un prompt como tarea al modelo:

```toml
[commands.test]
description = "Ejecuta los tests y diagnostica fallos"
prompt = "Ejecuta los tests del proyecto, analiza los fallos y corrígelos uno por uno"
confirm = false
```

Luego, dentro de la sesión: `/test`.

### Hooks de ciclo de vida — `.dpx/hooks.toml`

Ejecuta comandos automáticamente ante eventos (`OnSessionStart`, `OnSessionEnd`, `PreToolUse`, `PostToolUse`, `PreCommit`):

```toml
[[hooks]]
event = "PostToolUse"
tools = ["write_file", "edit_file"]   # opcional: filtra por tool
command = "cargo fmt"

[[hooks]]
event = "PreCommit"
command = "cargo test"
```

---

## ⚙️ Cómo funciona

### Ciclo de un turno

Cada turno del usuario dispara un **loop agéntico** de hasta 8 rondas (ampliable):

1. El usuario envía un mensaje (posiblemente con `@archivo` adjuntos)
2. El modelo responde con texto + posiblemente tool calls o bloques `dpx:*`
3. `dpx` **cuarentena** los bloques malformados (fence roto), aplica escrituras y ediciones (con confirmación), atiende lecturas y búsquedas (libres), ejecuta comandos (con sandbox de seguridad)
4. Si hubo acciones, los resultados se realimentan al modelo y **vuelve a iterar**
5. Si el modelo termina sin pedir más acciones, el turno se cierra
6. La respuesta se guarda en la transcripción y en el historial de la sesión

Si el modelo falla a mitad de un turno (error de red transitorio), `dpx` **reintenta esa ronda** en vez de matar el turno entero. Si falla sin haber emitido nada, **degrada al siguiente cerebro** con API key y reintenta.

### Verificación automática de build y tests

Si el modelo escribe código fuente (`.java`, `.rs`, `.kt`, …) o toca el build (`pom.xml`, `Cargo.toml`, `build.gradle`), `dpx` **lanza automáticamente la compilación** (Maven/Gradle/Cargo, prefiriendo el wrapper del proyecto) y le pasa los errores al modelo para que itere — sin que tenga que pedirlo.

En modo `/auto all` da un paso más: tras escribir código corre la **suite de tests completa** (`cargo test` / `mvn test` / `gradle test`) y se autocorrige con los fallos, no solo verifica que compile. El manifiesto real del proyecto se inyecta al prompt para que el modelo no invente dependencias ni versiones.

Cuando un `edit_file` no encuentra su bloque exacto, dpx tolera diferencias de **CRLF/LF e indentación** (edición fuzzy) y, si aun así falla, le devuelve al modelo la zona real del archivo para que reintente con conocimiento en vez de a ciegas.

### Mapa de símbolos (repo-map)

Al arrancar, `dpx` construye un **mapa de símbolos** del proyecto (funciones, structs, clases, traits… por archivo) mediante heurística por lenguaje, sin compiladores en C. Se inyecta al prompt para que el modelo sepa **qué define cada archivo y dónde** — así lee menos archivos, gasta menos tokens y se equivoca menos.

### Diagnósticos LSP

Vía la tool `lsp_diagnostics`, dpx arranca el **language server** del lenguaje (rust-analyzer, typescript-language-server, pyright, gopls), abre un archivo y devuelve sus **errores y warnings reales** con línea y columna — grounding de calidad de compilador **sin compilar el proyecto entero**. Ideal para verificar un archivo recién editado o ubicar un error con precisión.

- Cliente LSP propio (JSON-RPC sobre stdio, framing Content-Length), sin dependencias nuevas.
- El servidor se **cachea por lenguaje** durante la sesión: el primer diagnóstico paga el indexado; los siguientes reusan el server caliente.
- Soporta `.rs`, `.ts/.tsx`, `.js/.jsx`, `.py` y `.go`. Si el language server no está instalado, lo dice sin romper la tarea.
- Los comandos por defecto se pueden sobrescribir en `.dpx/lsp.toml`:

```toml
[servers.rust]
command = "rust-analyzer"
args = []
```

### Subagentes

Cuando una investigación requiere leer muchos archivos largos para extraer una conclusión concreta (localizar dónde se hace algo, mapear un flujo, recopilar contexto disperso), el agente puede delegar en un **subagente** vía la tool `spawn_agent`. El subagente:

- Corre en **aislamiento**: su propio contexto e historial, con el mismo cerebro.
- Es de **solo lectura** (`read_file`, `search_project`, `web_search`) — sin escrituras, comandos ni recursión, así que no hay efectos secundarios ni confirmaciones.
- Devuelve **solo su conclusión** al agente principal: los archivos que leyó **no** contaminan el contexto del padre → menos tokens y más foco.

Su consumo de tokens cuenta en el mismo ledger de la sesión (`/cost` lo refleja).

### Persistencia y memoria

En la carpeta del proyecto, `dpx` crea `.dpx/`:

```text
.dpx/
├── config.toml           # defaults del proyecto (creado por dpx init)
├── context.md            # memoria viva: estado + aprendizaje + próximos pasos
├── plan.md               # plan de trabajo pendiente entre sesiones
├── allowed_commands      # comandos marcados como "ejecutar siempre"
├── commands.toml         # comandos slash personalizados (opcional)
├── hooks.toml            # hooks de ciclo de vida (opcional)
├── mcp.toml              # servidores MCP a cargar (opcional)
├── lsp.toml              # overrides de language servers (opcional)
└── sessions/
    └── 20250608-141230.jsonl   # transcripción turno a turno
```

Al cerrar la sesión (`/salir`), `dpx` resume la conversación en `.dpx/context.md` usando el modelo barato (DeepSeek Flash sin thinking). La próxima vez que abras el proyecto, el mentor **retoma donde lo dejaste**.

### Seguridad

- **Sandbox de comandos**: cada `run_command` se clasifica en seguro / peligroso / prohibido.
- **Prohibidos**: bloqueados sin preguntar (`format`, `shutdown`, `rm -rf /`).
- **Peligrosos**: confirmación reforzada (hay que reescribir la primera palabra del comando).
- **Seguros**: confirmación normal, recordables con "ejecutar siempre".
- **Rutas**: ningún archivo se escribe fuera del proyecto (rechaza `..` y paths absolutos).
- **Guard anti-truncado**: detecta escrituras que encogen >40% un archivo grande y obliga a confirmar incluso en modo auto.
- **Cuarentena de bloques**: los fences `dpx:*` malformados anulan todas las acciones de esa respuesta.

---

## 📂 Estructura del proyecto

```text
src/
├── main.rs                    # Punto de entrada, carga .env
├── config.rs                  # Config del proyecto (.dpx/config.toml)
├── ui.rs                      # Capa visual: colores, markdown, spinner
├── token.rs                   # Ledger de tokens reales, costo y presupuesto
├── checkpoint.rs              # Snapshots para /undo y /diff
├── mcp.rs                     # Cliente MCP (JSON-RPC sobre stdio)
├── lsp.rs                     # Cliente LSP (diagnósticos vía language server)
├── cli/
│   ├── mod.rs                 # CLI con clap, despacho de comandos
│   ├── chat.rs                # Loop conversacional (REPL) + turnos agénticos
│   ├── editor.rs              # Editor de entrada propio sobre crossterm
│   ├── commands.rs            # Comandos slash personalizados (.dpx/commands.toml)
│   ├── hooks.rs               # Hooks de ciclo de vida (.dpx/hooks.toml)
│   └── init.rs                # Wizard dpx init
├── agent/
│   ├── mod.rs                 # Re-exports
│   ├── router.rs              # Model Router: Brain, Mentor, streaming
│   ├── tools.rs               # Definiciones de tools (function calling)
│   ├── search.rs              # Búsqueda web vía DuckDuckGo
│   └── diagnostic.rs          # Diagnóstico multi-lenguaje de fallos
├── focus/
│   ├── mod.rs                 # Focus Packs: system prompt, catálogo
│   ├── spring_boot.rs         # Skills de Spring Boot
│   ├── react.rs               # Skills de React
│   ├── node.rs                # Skills de Node.js
│   ├── python.rs              # Skills de Python
│   └── rust.rs                # Skills de Rust
├── fs/
│   ├── mod.rs                 # Parseo de bloques dpx:*, escritura, edición, repo-map
│   └── safety.rs              # Sandbox de comandos
└── session/
    └── mod.rs                 # Persistencia: .dpx/context.md, transcripción
```

---

## 🔧 Configuración

### `dpx init`

El wizard configura el proyecto paso a paso:

1. Detecta el stack por los archivos de la raíz (pom.xml, package.json, etc.)
2. Pregunta el cerebro por defecto (mostrando cuáles tienen API key)
3. Modo de trabajo (pro / hack)
4. Modo autónomo sí/no
5. Guarda `.dpx/config.toml`

### `.dpx/config.toml`

```toml
focus = "spring-boot"
brain = "deepseek"
mode = "pro"
auto = false
```

Estos valores son los defaults: los flags de CLI (`--focus`, `--brain`, `--mode`, `--auto`) los pisan, y los comandos del REPL (`/focus`, `/brain`, `/mode`, `/auto`) los cambian en caliente durante la sesión.

---

## 💻 Desarrollo (hackear dpx)

```bash
cargo check                     # compilación rápida
cargo test                      # tests
cargo test -- --ignored         # incluye tests de red (requieren internet)
cargo clippy -- -D warnings     # linter estricto
```

### Tests

El proyecto tiene **cobertura extensiva** (160+ tests unitarios y de integración, más algunos `#[ignore]` que requieren red o una API key):

- `chat.rs`: 35+ tests del loop agéntico, confirmaciones, cuarentena, guards, planes
- `fs/mod.rs`: 30+ tests de parseo de bloques, edits, writes, detección de stacks
- `router.rs`: tests de reintentos y backoff
- `tools.rs`: tests de definiciones y parseo de tool calls
- `diagnostic.rs`: tests de diagnóstico multi-lenguaje (Rust, TS, Python, Java)
- `session.rs`: tests de persistencia y allowlist
- `search.rs`: tests de búsqueda web
- `editor.rs`: tests de wrapping, cursor, autocompletado
- `safety.rs`: sandbox de comandos

### `/update`

Dentro del repo de `dpx`, el comando `/update` recompila e instala el binario sin cerrar la sesión (en Windows renombra el exe en uso antes de instalar).

---

## ⚖️ Licencia

Este proyecto está licenciado bajo la **MIT License** - consulta el archivo [LICENSE](LICENSE) para ver el texto legal completo.

**¿Qué significa esto en español?**

- ✅ Puedes usar dpx en tu empresa, gratis, sin pedir permiso
- ✅ Puedes modificarlo, mejorarlo y compartir tus cambios
- ✅ Puedes integrarlo en un producto comercial que vendas
- ❌ No puedes quitar el aviso de copyright ni hacerte pasar por el autor
- ❌ Los autores no se hacen responsables si algo sale mal (el software se da "como está")
