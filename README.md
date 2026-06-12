# dpx · tu mentor senior de desarrollo, en la terminal

**dpx** es un mentor de ingeniería de software que vive en tu terminal. No es un autocompletador ni un generador de scaffolding: es un agente que **enseña, explica y te deja escribir a ti**, o bien **toma el teclado y resuelve la tarea** en modo autónomo. Se hiper-enfoca según el stack en el que trabajas y recuerda el contexto de tu proyecto entre sesiones.

```
dpx chat                     # el mentor te enseña
dpx code                     # el agente autónomo hace el trabajo
dpx chat --mode hack         # modo rápido, para hackathones
dpx chat --focus spring-boot # enfocado en Spring Boot
dpx chat --brain kimi        # otro cerebro (modelo LLM)
```

---

## Tabla de contenidos

- [Instalación](#instalación)
- [Arquitectura](#arquitectura)
- [Modos y personas — las combinaciones](#modos-y-personas--las-combinaciones)
  - [Personas: mentor vs code](#personas-mentor-vs-code)
  - [Modos: pro vs hack](#modos-pro-vs-hack)
  - [Matriz de combinaciones](#matriz-de-combinaciones)
- [Cerebros (modelos)](#cerebros-modelos)
- [Focus Packs (enfoques por stack)](#focus-packs-enfoques-por-stack)
- [Modo autónomo (`--auto` / `/auto`)](#modo-autónomo---auto--auto)
- [Comandos del REPL](#comandos-del-repl)
- [Herramientas (function calling)](#herramientas-function-calling)
- [Cómo funciona](#cómo-funciona)
  - [Ciclo de un turno](#ciclo-de-un-turno)
  - [Persistencia y memoria](#persistencia-y-memoria)
  - [Seguridad](#seguridad)
- [Estructura del proyecto](#estructura-del-proyecto)
- [Configuración](#configuración)
- [Desarrollo (hackear dpx)](#desarrollo-hackear-dpx)
- [Licencia](#licencia)

---

## Instalación

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

**Sin API key, dpx arranca pero no puede responder**: el primer cerebro con key será el activo.

### Inicializar un proyecto

```bash
cd mi-proyecto
dpx init        # wizard paso a paso: detecta stack, elige cerebro, modo y auto
```

Esto crea `.dpx/config.toml` con los defaults del proyecto. Después, `dpx` o `dpx chat` arranca directo con esa config.

---

## Arquitectura

```
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

## Modos y personas — las combinaciones

dpx tiene DOS ejes de configuración que se combinan: **persona** y **modo**. El resultado son cuatro formas de trabajar distintas.

### Personas: mentor vs code

| Persona | Qué hace | Cuándo usarla |
|---------|----------|---------------|
| **mentor** (default) | Enseña, explica el porqué, te deja escribir a ti el código. No genera archivos completos salvo que lo pidas explícitamente. | Aprender, entender decisiones, revisar código, diseño. |
| **code** | Agente autónomo: escribe, compila, ejecuta, corrige. Itera hasta que la tarea funciona. | Tareas hechas, implementar features, corregir bugs, refactors. |

Se activa con `--persona` (CLI) o los comandos `/mentor` y `/code` en el REPL.

```bash
dpx chat                           # mentor (default)
dpx code                           # agente autónomo
dpx code --mode hack --auto        # agente rápido y sin preguntar
```

### Modos: pro vs hack

| Modo | Actitud | Temperatura |
|------|---------|-------------|
| **pro** (default) | Metódico: arquitectura primero, cada decisión explicada, tests incluidos, señala deuda técnica. | 0.4 |
| **hack** | Rápido: defaults sensatos, mínimo boilerplate, demo funcionando YA. Enseña en una línea. | 0.7 |

Se activa con `--mode` (CLI) o `/mode pro|hack` en el REPL.

### Matriz de combinaciones

```
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

## Cerebros (modelos)

dpx usa un **Model Router**: tres cerebros intercambiables en caliente, cada uno con su fuerte. El router construye el agente con el system prompt y la temperatura correcta según el modo.

| Cerebro | Proveedor | Fuerte | Ventana | API Key |
|---------|-----------|--------|---------|---------|
| **DeepSeek** (V4 Pro) | DeepSeek | Principal · razonamiento profundo · tool-calling nativo | 128k | `DEEPSEEK_API_KEY` |
| **Kimi** (K2.5) | Moonshot | Agéntico sólido · contexto largo | 256k | `MOONSHOT_API_KEY` |
| **Qwen** (Coder) | OpenRouter | Código · muy barato | 256k | `OPENROUTER_API_KEY` |

Cambio en caliente desde el REPL: `/brain kimi`. Si el cerebro activo falla (sin saldo, saturado), dpx degrada automáticamente al siguiente con API key.

### Thinking en DeepSeek

En modo `pro`, DeepSeek razona con `reasoning_effort: max`; en `hack` usa `high` (más rápido). Kimi y Qwen no tienen este parámetro.

---

## Focus Packs (enfoques por stack)

Cada Focus Pack inyecta conocimiento de dominio en el system prompt del modelo: versiones exactas, buenas prácticas, errores comunes y herramientas propias de ese ecosistema.

| Pack | Stack | Se activa con |
|------|-------|---------------|
| `spring-boot` | Backend Java/Spring Boot | `--focus spring-boot` |
| `react` | Frontend React (Vite, TanStack Query, RTL) | `--focus react` |
| `node` | Backend Node.js (Fastify/Express, zod) | `--focus node` |
| `python` | Backend Python con FastAPI | `--focus python` |
| `rust` | Sistemas y CLIs en Rust | `--focus rust` |
| `gradle` | Proyecto JVM con Gradle (genérico) | `--focus gradle` |

Si no pasas `--focus`, dpx **detecta el stack automáticamente** analizando los archivos del proyecto (`pom.xml`, `package.json`, `Cargo.toml`, etc.). Si no reconoce nada, arranca como mentor general.

---

## Modo autónomo (`--auto` / `/auto`)

Con el modo autónomo activado, dpx **aplica cambios y ejecuta comandos seguros sin pedir confirmación**. Las puertas de seguridad se mantienen siempre:

| Tipo de acción | ¿Pregunta en auto? |
|----------------|---------------------|
| `write_file` / `edit_file` (archivos nuevos o pequeños) | ❌ No |
| `run_command` seguro (`cargo check`, `mvn compile`) | ❌ No |
| `git_status`, `git_diff`, `git_log` (solo lectura) | ❌ No |
| `write_file` que trunca un archivo grande (>200 líneas) | ✅ **Sí** (guard anti-truncado) |
| `run_command` peligroso (`rm -rf`, `git reset --hard`) | ✅ **Sí** (confirmación reforzada) |
| `run_command` prohibido (`format`, `shutdown`) | 🚫 Bloqueado siempre |
| `git_commit`, `delete_file` (mutan el repo) | ✅ **Sí** |

---

## Comandos del REPL

Dentro de una sesión, el prompt entiende estos comandos slash:

| Comando | Acción |
|---------|--------|
| `/help` | Muestra esta ayuda |
| `/status` | Estado: config, cerebros, memoria, tokens |
| `/models` | Lista los cerebros y cuál tiene API key |
| `/clear` | Reinicia la conversación (olvida la sesión) |
| `/compact` | Resume la charla para liberar contexto |
| `/context` | Muestra la memoria guardada de `.dpx/context.md` |
| `/focus [id]` | Cambia de stack (sin id: lista disponibles) |
| `/mode pro\|hack` | Cambia actitud |
| `/brain deepseek\|kimi\|qwen` | Cambia de modelo |
| `/mentor` | Activa persona mentor (enseña) |
| `/code` | Activa persona code (agente autónomo) |
| `/auto [on\|off]` | Activa/desactiva modo autónomo |
| `/update` | Recompila e instala dpx desde el repo actual |
| `/salir` | Termina la sesión y guarda el contexto |

También puedes referenciar archivos con `@ruta/al/archivo.java` y el mentor los leerá (con autocompletado por Tab).

---

## Herramientas (function calling)

dpx expone estas herramientas nativas al modelo (preferidas sobre los bloques de texto `dpx:*`):

| Herramienta | Función |
|-------------|---------|
| `read_file` | Leer archivos del proyecto |
| `search_project` | Buscar texto en todos los archivos |
| `web_search` | Buscar en DuckDuckGo (gratis, sin API key) |
| `write_file` | Crear/sobrescribir archivos |
| `edit_file` | Editar fragmentos con SEARCH/REPLACE literal |
| `delete_file` | Borrar archivos |
| `run_command` | Ejecutar comandos de shell |
| `git_status` | Estado del repo (solo lectura) |
| `git_diff` | Diff del working tree (solo lectura) |
| `git_log` | Últimos commits (solo lectura) |
| `git_commit` | Crear commit (MUTA, pide confirmación) |

---

## Cómo funciona

### Ciclo de un turno

Cada turno del usuario dispara un **loop agéntico** de hasta 8 rondas (ampliable):

1. El usuario envía un mensaje (posiblemente con `@archivo` adjuntos)
2. El modelo responde con texto + posiblemente tool calls o bloques `dpx:*`
3. dpx **cuarentena** los bloques malformados (fence roto), aplica escrituras y ediciones (con confirmación), atiende lecturas y búsquedas (libres), ejecuta comandos (con sandbox de seguridad)
4. Si hubo acciones, los resultados se realimentan al modelo y **vuelve a iterar**
5. Si el modelo termina sin pedir más acciones, el turno se cierra
6. La respuesta se guarda en la transcripción y en el historial de la sesión

Si el modelo falla a mitad de un turno (error de red transitorio), dpx **reintenta esa ronda** en vez de matar el turno entero. Si falla sin haber emitido nada, **degrada al siguiente cerebro** con API key y reintenta.

### Verificación automática de build

Si el modelo escribe código fuente (`.java`, `.rs`, `.kt`) o toca el build (`pom.xml`, `Cargo.toml`), dpx **lanza automáticamente la compilación** y le pasa los errores al modelo para que itere — sin que tenga que pedirlo.

### Persistencia y memoria

En la carpeta del proyecto, dpx crea `.dpx/`:

```
.dpx/
├── config.toml           # defaults del proyecto (creado por dpx init)
├── context.md            # memoria viva: estado + aprendizaje + próximos pasos
├── plan.md               # plan de trabajo pendiente entre sesiones
├── allowed_commands      # comandos marcados como "ejecutar siempre"
└── sessions/
    └── 20250608-141230.jsonl   # transcripción turno a turno
```

Al cerrar la sesión (`/salir`), dpx resume la conversación en `.dpx/context.md` usando el modelo barato (DeepSeek Flash sin thinking). La próxima vez que abras el proyecto, el mentor **retoma donde lo dejaste**.

### Seguridad

- **Sandbox de comandos**: cada `run_command` se clasifica en seguro / peligroso / prohibido
- **Prohibidos**: bloqueados sin preguntar (`format`, `shutdown`, `rm -rf /`)
- **Peligrosos**: confirmación reforzada (hay que reescribir la primera palabra del comando)
- **Seguros**: confirmación normal, recordables con "ejecutar siempre"
- **Rutas**: ningún archivo se escribe fuera del proyecto (rechaza `..` y paths absolutos)
- **Guard anti-truncado**: detecta escrituras que encogen >40% un archivo grande y obliga a confirmar incluso en modo auto
- **Cuarentena de bloques**: los fences `dpx:*` malformados anulan todas las acciones de esa respuesta

---

## Estructura del proyecto

```
src/
├── main.rs                    # Punto de entrada, carga .env
├── config.rs                  # Config del proyecto (.dpx/config.toml)
├── ui.rs                      # Capa visual: colores, markdown, spinner
├── cli/
│   ├── mod.rs                 # CLI con clap, despacho de comandos
│   ├── chat.rs                # Loop conversacional (REPL) + turnos agénticos
│   ├── editor.rs              # Editor de entrada propio sobre crossterm
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
│   ├── mod.rs                 # Parseo de bloques dpx:*, escritura, edición
│   └── safety.rs              # Sandbox de comandos
└── session/
    └── mod.rs                 # Persistencia: .dpx/context.md, transcripción
```

---

## Configuración

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

## Desarrollo (hackear dpx)

```bash
cargo check                     # compilación rápida
cargo test                      # tests
cargo test -- --ignored         # incluye tests de red (requieren internet)
cargo clippy -- -D warnings     # linter estricto
```

### Tests

El proyecto tiene **cobertura extensiva de tests unitarios y de integración**:

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

Dentro del repo de dpx, el comando `/update` recompila e instala el binario sin cerrar la sesión (en Windows renombra el exe en uso antes de instalar).

---

## Licencia

MIT License — Copyright (c) 2025 DPX

Se concede permiso, de forma gratuita, a cualquier persona que obtenga una copia de este software
y los archivos de documentación asociados (el "Software"), para usar, copiar, modificar, fusionar,
publicar, distribuir, sublicenciar y/o vender copias del Software sin restricción alguna, y para
permitir a las personas a las que se les proporcione el Software que hagan lo mismo, sujeto a
estas dos condiciones:

1. El aviso de copyright anterior y este aviso de permiso deben incluirse en todas las copias
   o partes sustanciales del Software.

2. EL SOFTWARE SE PROPORCIONA "TAL CUAL", SIN GARANTÍA DE NINGÚN TIPO, EXPRESA O IMPLÍCITA,
   INCLUYENDO PERO NO LIMITADO A GARANTÍAS DE COMERCIABILIDAD, IDONEIDAD PARA UN PROPÓSITO
   PARTICULAR Y NO INFRACCIÓN. EN NINGÚN CASO LOS AUTORES O TITULARES DEL COPYRIGHT SERÁN
   RESPONSABLES POR NINGUNA RECLAMACIÓN, DAÑO U OTRA RESPONSABILIDAD, YA SEA EN UNA ACCIÓN
   DE CONTRATO, AGRAVIO O DE OTRO TIPO, QUE SURJA DE O EN CONEXIÓN CON EL SOFTWARE O EL USO
   U OTRO TIPO DE ACCIONES EN EL SOFTWARE.

**¿Qué significa esto en español?**

- ✅ Puedes usar dpx en tu empresa, gratis, sin pedir permiso
- ✅ Puedes modificarlo, mejorarlo y compartir tus cambios
- ✅ Puedes integrarlo en un producto comercial que vendas
- ❌ No puedes quitar el aviso de copyright ni hacerte pasar por el autor
- ❌ Los autores no se hacen responsables si algo sale mal (el software se da "como está")
