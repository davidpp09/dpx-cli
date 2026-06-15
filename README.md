<div align="center">
  <h1>🚀 dpx</h1>
  <p><b>Tu mentor senior de desarrollo, directamente en tu terminal.</b></p>

  [![Rust](https://img.shields.io/badge/Rust-2024-orange.svg)](https://www.rust-lang.org)
  [![Estado](https://img.shields.io/badge/versión-0.3.0-blue.svg)](#)
  [![Privado](https://img.shields.io/badge/proyecto-privado-lightgrey.svg)](#-licencia)
</div>

---

> **dpx** es un agente de ingeniería que vive en tu terminal. No es un autocompletador ni un generador de scaffolding: según el **modo** que elijas, **hace el trabajo por ti**, **construye rápido con criterio**, o **te enseña a pensar como un senior**. Se hiper-enfoca según tu stack, **recuerda** el contexto de tu proyecto entre sesiones, y **mejora con el uso** aprendiendo skills propias de tu repo.

```bash
dpx code     # 🤖 agente autónomo: escribe, ejecuta, corrige
dpx hack     # ⚡ construir rápido CON criterio (demo sólida, sin chapuza)
dpx learn    # 🎓 tutor socrático: te enseña, tú escribes
dpx          # abre el modo por defecto de la config del proyecto
```

---

## 📖 Tabla de contenidos

- [⚡ Instalación](#-instalación)
- [🎛️ Los tres modos](#️-los-tres-modos)
- [🧠 El cerebro (DeepSeek)](#-el-cerebro-deepseek)
- [🎯 Focus Packs (enfoques por stack)](#-focus-packs-enfoques-por-stack)
- [💬 Memoria de largo plazo](#-memoria-de-largo-plazo)
- [🧩 Skills auto-mejorables](#-skills-auto-mejorables)
- [💸 Auto-delegación a subagentes (ahorro)](#-auto-delegación-a-subagentes-ahorro)
- [🤖 Modo autónomo (`/auto`)](#-modo-autónomo-auto)
- [⌨️ Comandos del REPL](#️-comandos-del-repl)
- [🛠️ Herramientas (function calling)](#️-herramientas-function-calling)
- [⚙️ Cómo funciona](#️-cómo-funciona)
- [🔌 Extensibilidad (MCP, comandos y hooks)](#-extensibilidad-mcp-comandos-y-hooks)
- [📂 Estructura del proyecto](#-estructura-del-proyecto)
- [🔧 Configuración](#-configuración)
- [💻 Desarrollo](#-desarrollo)
- [⚖️ Licencia](#-licencia)

---

## ⚡ Instalación

```bash
git clone <tu-repo-privado>/dpx-cli.git
cd dpx-cli
cargo install --path .
```

### Requisitos

- Rust toolchain estable (edition 2024).
- Tu API key de DeepSeek en `~/.dpx/.env` o en un `.env` del proyecto:

```env
DEEPSEEK_API_KEY=sk-...
```

> [!IMPORTANT]
> dpx usa **solo DeepSeek**. Sin la key arranca pero no puede responder.

### Primer arranque (onboarding)

La primera vez que abres dpx en un proyecto **sin `.dpx/`**, arranca una **pantalla de configuración** (como un wizard): detecta tu stack, eliges enfoque y nivel de autonomía, y guarda `.dpx/config.toml`. El modo lo fija el subcomando con el que entraste.

```bash
cd mi-proyecto
dpx code        # primera vez → te configura y entra en modo code
```

Si entras en **`hack`** en un proyecto nuevo, dpx arranca pidiéndote tu idea y la pasa por el **comité** (lluvia de ideas) para sacar un plan antes de construir.

---

## 🎛️ Los tres modos

dpx tiene **un solo eje**: tres modos excluyentes. Los tres **piensan a fondo** y hacen las cosas **bien** — lo que cambia es el **rol**, no la calidad. Cada uno tiene su **propia identidad visual** (color de acento y banner).

| Modo | Color | Qué hace | Cuándo |
|:---|:---|:---|:---|
| 🤖 **code** | azul | Agente autónomo: implementa, ejecuta, verifica y corrige hasta dejarlo robusto. | Implementar features, corregir bugs, refactors. |
| ⚡ **hack** | ámbar | Construye rápido pero **con criterio**: defaults sensatos, mínimo boilerplate, código correcto que corre ya — **sin chapuza**. | Prototipos, hackathones, demos sólidas. |
| 🎓 **learn** | verde | Tutor socrático: te hace pensar y te enseña conceptos, patrones y arquitectura. **Tú escribes el código**, él te guía. | Aprender, entender el porqué, fijar conocimiento. |

```bash
dpx code --focus spring-boot     # agente enfocado en Spring Boot
dpx hack --auto                  # construir rápido y sin preguntar
dpx learn                        # el tutor socrático

# en vivo, dentro de la sesión:
/modo hack                       # cambia de modo (y de color) al vuelo
```

Cada modo expone **solo los comandos que le corresponden** (ver [Comandos](#️-comandos-del-repl)): p. ej. `/comité` solo en hack, `/examen` solo en learn, `/auto` solo en code/hack.

---

## 🧠 El cerebro (DeepSeek)

dpx corre sobre **DeepSeek** con dos niveles, repartidos por el **Model Router**:

| Nivel | Modelo | Para qué |
|:---|:---|:---|
| 🐋 **pro** | `deepseek-v4-pro` (razonador) | El cerebro principal de cada turno. Razona a fondo (`reasoning_effort: max`) en los tres modos. |
| ⚡ **flash** | `deepseek-v4-flash` | ~**12× más barato**. Para subagentes de investigación y resúmenes de cierre — tareas mecánicas que no necesitan el caro. |

Ventana de contexto: **128k**. dpx **compacta automáticamente** la conversación al llegar al 75%, y antes de eso aligera los resultados de herramienta viejos para que la sesión dure más.

---

## 🎯 Focus Packs (enfoques por stack)

Cada Focus Pack inyecta conocimiento de dominio en el prompt: versiones exactas, buenas prácticas y errores comunes de ese ecosistema.

| Pack | Stack |
|:---|:---|
| 🌱 `spring-boot` | Backend Java/Spring Boot |
| ⚛️ `react` | Frontend React (Vite, TanStack Query, RTL) |
| 🟩 `node` | Backend Node.js (Fastify/Express, zod) |
| 🐍 `python` | Backend Python con FastAPI |
| 🦀 `rust` | Sistemas y CLIs en Rust |
| 🐘 `gradle` | Proyecto JVM con Gradle (genérico) |
| 🛠️ `dpx` | El propio dpx: su arquitectura interna, para auto-editarse |

Sin `--focus`, dpx **detecta el stack automáticamente** por los archivos de la raíz (`pom.xml`, `package.json`, `Cargo.toml`…). En modo **learn**, el focus pack también aporta el **temario** del stack (`/temario`).

---

## 💬 Memoria de largo plazo

Para "acabar con el problema del contexto", dpx tiene **memoria semántica** recuperable: guarda fragmentos con su **embedding** (vector que captura el significado) y recupera los relevantes en cada turno por **similitud coseno**. Los embeddings son **locales** (fastembed / ONNX, modelo BGE-small): gratis, privados y **offline** tras la primera descarga.

- **`/recordar <texto>`** — guardas algo a mano.
- **Auto-ingesta** — al cerrar cada sesión, dpx embebe el resumen y lo añade a la memoria solo.
- **Recuperación automática** — antes de cada turno trae por *significado* lo relevante (notas + resúmenes de sesiones pasadas) y se lo da al modelo.

Vive en `.dpx/memory.jsonl` (no se sube a git).

---

## 🧩 Skills auto-mejorables

dpx **mejora con el uso**: acumula *playbooks* sobre cómo se hacen las cosas **en tu proyecto** y los va refinando. Cuando termina una tarea no trivial que puede repetirse (crear un endpoint, montar un test, una migración…), declara el procedimiento; el CLI lo embebe y, si ya hay uno parecido, lo **refina** (sube usos y confianza); si no, lo **crea**. Antes de cada turno (code/hack) recupera por significado los que aplican y los inyecta para aplicarlos y mejorarlos.

- **`/habilidades`** — lista las skills aprendidas, con confianza (●●●○○) y usos.
- Persisten en `.dpx/agent_skills.json`.

> Las skills empiezan vacías y se construyen **con el uso** — no esperes magia el primer día.

---

## 💸 Auto-delegación a subagentes (ahorro)

Los subagentes corren en **flash** (~12× más barato) y en **contexto aislado**: leen mucho pero te devuelven solo la conclusión. dpx **delega solo**: cuando tu petición es de **investigación** ("¿dónde se valida X?", "cómo funciona Y", "busca todos los usos de Z"), lanza un subagente flash que la resuelve y le pasa la conclusión ya digerida al cerebro caro `pro` — **antes** de responderte.

Resultado: el trabajo de lectura/búsqueda **no lo paga el cerebro caro**. Lo ves en acción (`⎿ delegando en subagente flash…`) y el reparto en **`/costo`**. En peticiones de **cambio** no delega: eso lo hace el agente principal.

---

## 🤖 Modo autónomo (`/auto`)

Disponible en **code** y **hack**. Cuatro niveles acumulativos; cada uno relaja una capa de confirmaciones. Se controla con `--auto <nivel>` (CLI) o `/auto <nivel>` en el REPL (`/auto` sin argumento alterna `all`/`off`).

| Nivel | Sin preguntar |
|:---|:---|
| `off` (default) | Nada: cada acción se confirma. |
| `reads` | Lecturas/búsquedas (ya libres) + auto-extiende rondas. |
| `writes` | + escrituras y ediciones (el diff se muestra igual). |
| `all` ⚡ | + comandos **seguros**, y tras escribir corre la **suite de tests** y se autocorrige. |

Las puertas de seguridad se mantienen **siempre**, incluso en `all`:

| Acción | ¿Pregunta aunque esté en `all`? |
|:---|:---|
| `write_file` que trunca un archivo grande (>40%) | ✅ **Sí** *(guard anti-truncado)* |
| `write_file` que sobrescribe un archivo grande (≥200 líneas) | ✅ **Sí** *(la doctrina prefiere `edit_file`)* |
| `run_command` peligroso (`rm -rf`, `git reset --hard`) | ✅ **Sí** *(hay que reescribir la 1ª palabra)* |
| `run_command` prohibido (`format`, `shutdown`, `mkfs`) | 🚫 Bloqueado siempre |
| `delete_file`, commits (mutan el repo) | ✅ **Sí** |

> [!TIP]
> Con `/deshacer` reviertes lo del último turno y con `/cambios` revisas todo lo que dpx tocó. Úsalo sin miedo.

---

## ⌨️ Comandos del REPL

Los nombres son **en español** (los ingleses como `/help`, `/focus`… siguen de alias). `/ayuda` muestra **solo los comandos de tu modo**.

| Comando | Disponible en | Acción |
|:---|:---|:---|
| `/ayuda` | todos | Lista los comandos de tu modo |
| `/estado` | todos | Config, cerebro, memoria, tokens |
| `/modelos` | todos | El cerebro y su API key |
| `/costo` | todos | Tokens reales + % de caché + costo aprox |
| `/presupuesto [N]` | todos | Tope de tokens de la sesión (ej. `/presupuesto 100k`) |
| `/contexto` | todos | La memoria guardada del proyecto |
| `/recordar <texto>` | todos | Guarda algo en la memoria de largo plazo |
| `/enfoque [id]` | todos | Cambia de stack (sin id: lista) |
| `/modo [code\|hack\|learn]` | todos | Cambia de modo (y de color) |
| `/cerebro [modelo]` | todos | Cerebro (dpx usa solo DeepSeek) |
| `/limpiar` | todos | Reinicia la conversación |
| `/compactar` | todos | Resume la charla para liberar contexto |
| `/actualizar` | todos | Recompila e instala dpx desde el repo |
| `/salir` | todos | Termina y guarda el contexto |
| `/cambios` | code · hack | Todo lo que dpx cambió en la sesión |
| `/deshacer` | code · hack | Revierte los cambios del último turno |
| `/habilidades` | code · hack | Playbooks aprendidos (se afinan con el uso) |
| `/auto [off\|all]` | code · hack | Nivel de autonomía |
| `/comité <idea>` | hack | El comité (4 roles) evalúa tu idea y da un plan |
| `/progreso` | learn | Tu progreso de aprendizaje por tema |
| `/temario` | learn | El temario del stack y cuánto llevas |
| `/examen [tema]` | learn | El tutor te interroga para fijar lo aprendido |

*También referencias archivos con `@ruta/al/archivo` (autocompletado con Tab), y defines tus propios comandos en `.dpx/commands.toml`.*

---

## 🛠️ Herramientas (function calling)

dpx expone estas herramientas nativas al modelo (preferidas sobre los bloques de texto `dpx:*`):

| Herramienta | Función |
|:---|:---|
| 📄 `read_file` | Leer archivos del proyecto |
| 🔍 `search_project` | Buscar texto en todos los archivos |
| 🌐 `web_search` | Buscar en DuckDuckGo (gratis, sin API key) |
| 🧩 `spawn_agent` | Lanzar un subagente de investigación aislado (solo lectura, con rol) |
| 🩺 `lsp_diagnostics` | Diagnósticos reales de un archivo vía language server |
| ✏️ `write_file` | Crear/sobrescribir archivos |
| ✂️ `edit_file` | Editar fragmentos con SEARCH/REPLACE literal |
| 🗑️ `delete_file` | Borrar archivos |
| 💻 `run_command` | Ejecutar comandos de shell (con sandbox) |
| 📊 `git_status` · `git_diff` · `git_log` | Estado del repo (solo lectura) |
| 💾 `git_commit` | Crear commit (muta, pide confirmación) |

dpx también puede cargar herramientas externas vía **MCP** y exponerlas como propias.

---

## ⚙️ Cómo funciona

### Ciclo de un turno

Cada mensaje dispara un **loop agéntico** de hasta 8 rondas (ampliable):

1. (code/hack) si la petición es de investigación, un **subagente flash** la resuelve y antepone su conclusión; se recuperan **memoria** y **skills** relevantes.
2. El modelo responde con texto + tool calls.
3. dpx pone en **cuarentena** los bloques malformados, aplica escrituras/ediciones (con diff y confirmación), atiende lecturas/búsquedas (libres) y ejecuta comandos (con sandbox).
4. Los resultados se realimentan y **vuelve a iterar** hasta que el modelo cierra el turno.
5. La respuesta se guarda; si dpx aprendió un playbook, lo destila en una skill.

Si el modelo falla a mitad de turno (red transitoria) **reintenta la ronda**; si falla sin emitir nada, **degrada al siguiente cerebro** con key.

### Verificación automática

Al tocar código fuente o el build (`pom.xml`, `Cargo.toml`, `build.gradle`), dpx **compila solo** (prefiriendo el wrapper del proyecto) y le pasa los errores al modelo para iterar. En `/auto all` corre además la **suite de tests** y se autocorrige.

### Mapa de símbolos · LSP · subagentes

- **Repo-map**: al arrancar, dpx mapea funciones/structs/clases por archivo (heurística por lenguaje) y lo inyecta — el modelo lee menos y se equivoca menos.
- **LSP**: `lsp_diagnostics` arranca el language server (rust-analyzer, tsserver, pyright, gopls) y devuelve errores reales con línea/columna, sin compilar todo.
- **Subagentes**: aislados, solo lectura, con roles (`researcher`, `planner`, `reviewer`, `debugger`, `architect`…). Devuelven solo su conclusión; su consumo cuenta en `/costo`.

### Seguridad

- **Sandbox de comandos**: cada `run_command` se clasifica en seguro / peligroso / prohibido.
- **Rutas**: nada se escribe fuera del proyecto (rechaza `..` y paths absolutos).
- **Guard anti-truncado** y **cuarentena de bloques** protegen incluso en modo auto.

---

## 🔌 Extensibilidad (MCP, comandos y hooks)

Todo por proyecto desde `.dpx/`, sin recompilar:

- **`.dpx/mcp.toml`** — servidores MCP: dpx hace el handshake, descubre sus tools y las fusiona con las nativas.
- **`.dpx/commands.toml`** — comandos slash propios (`/loquesea`) que inyectan un prompt.
- **`.dpx/hooks.toml`** — comandos automáticos ante eventos (`OnSessionStart`, `PostToolUse`, `PreCommit`…).

```toml
# .dpx/commands.toml
[commands.test]
description = "Ejecuta los tests y diagnostica fallos"
prompt = "Ejecuta los tests, analiza los fallos y corrígelos uno por uno"
confirm = false
```

---

## 📂 Estructura del proyecto

```text
src/
├── main.rs            # Entrada, carga .env
├── config.rs          # Config del proyecto (.dpx/config.toml)
├── ui.rs              # Capa visual: tema por modo, markdown, spinner
├── memory.rs          # Memoria semántica (embeddings + coseno)
├── agent_skill.rs     # Skills auto-mejorables (playbooks del proyecto)
├── skill.rs           # Progreso de aprendizaje del usuario (modo learn)
├── token.rs           # Ledger de tokens reales, costo y presupuesto
├── checkpoint.rs      # Snapshots para /deshacer y /cambios
├── mcp.rs · lsp.rs    # Clientes MCP y LSP
├── cli/               # CLI (clap), REPL, editor propio, init, hooks
├── agent/             # Model Router, tools, roles, búsqueda web
├── focus/             # Focus Packs, modos, comité, temario
├── fs/                # Parseo de bloques, escritura/edición, repo-map, sandbox
└── session/           # Persistencia .dpx/
```

---

## 🔧 Configuración

`.dpx/config.toml` (creado por el onboarding o `dpx init`):

```toml
focus = "spring-boot"
brain = "deepseek"
mode  = "code"      # code | hack | learn
auto  = "off"       # off | reads | writes | all
```

Son los defaults: los flags de CLI (`--focus`, `--brain`, `--auto`) los pisan, y los comandos del REPL (`/enfoque`, `/modo`, `/auto`…) los cambian en caliente.

---

## 💻 Desarrollo

```bash
cargo check                     # compilación rápida
cargo test                      # tests (211+)
cargo test -- --ignored         # incluye tests de red / embeddings
cargo clippy --all-targets -- -D warnings   # linter estricto (cero warnings)
```

Dentro del repo de dpx, **`/actualizar`** recompila e instala el binario sin cerrar la sesión (en Windows renombra el exe en uso antes de instalar).

> [!NOTE]
> En Windows, `cargo install` falla con `os error 5` si tienes una sesión de dpx abierta (el `.exe` está bloqueado). Cierra la sesión o usa `/actualizar`.

---

## ⚖️ Licencia

**Proyecto privado. Todos los derechos reservados.**

Este software no se distribuye bajo ninguna licencia de código abierto: no hay permiso de uso, copia, modificación ni distribución salvo autorización explícita del autor.
