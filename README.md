<div align="center">
  <h1>dpx</h1>
  <p><b>Tu mentor senior de desarrollo, directamente en tu terminal.</b></p>

  [![Rust](https://img.shields.io/badge/Rust-2024-orange.svg)](https://www.rust-lang.org)
  [![Estado](https://img.shields.io/badge/versión-0.3.0-blue.svg)](#)
  [![Privado](https://img.shields.io/badge/proyecto-privado-lightgrey.svg)](#-licencia)
</div>

---

> **dpx** es un agente de ingeniería que vive en tu terminal. No es un autocompletador ni un generador de scaffolding: según el **modo** que elijas, **hace el trabajo por ti**, **construye rápido con criterio**, o **te enseña a pensar como un senior**. Se hiper-enfoca según tu stack y **recuerda** el contexto de tu proyecto entre sesiones.

```bash
dpx code     # agente autónomo: escribe, ejecuta, corrige
dpx hack     # construir rápido CON criterio (demo sólida, sin chapuza)
dpx learn    # tutor socrático: te enseña, tú escribes
dpx          # abre el modo por defecto de la config del proyecto
```

---

## Tabla de contenidos

- [Instalación](#instalación)
- [Los tres modos](#los-tres-modos)
- [El cerebro (DeepSeek)](#el-cerebro-deepseek)
- [Focus Packs (enfoques por stack)](#focus-packs-enfoques-por-stack)
- [Memoria de proyecto](#memoria-de-proyecto)
- [Modo autónomo (`/auto`)](#modo-autónomo-auto)
- [Comandos del REPL](#comandos-del-repl)
- [Herramientas (function calling)](#herramientas-function-calling)
- [Cómo funciona](#cómo-funciona)
- [Estructura del proyecto](#estructura-del-proyecto)
- [Configuración](#configuración)
- [Desarrollo](#desarrollo)
- [Licencia](#licencia)

---

## Instalación

```bash
git clone <tu-repo-privado>/dpx-cli.git
cd dpx-cli
cargo install --path .
```

### Requisitos

- Rust toolchain estable (edition 2024).
- API key de DeepSeek en `~/.dpx/.env` o en `.env` del proyecto:

```env
DEEPSEEK_API_KEY=sk-...
```

> [!IMPORTANT]
> dpx usa **solo DeepSeek**. Sin la key arranca pero no puede responder.

### Primer arranque

La primera vez en un proyecto **sin `.dpx/`**, arranca un **wizard de configuración**: detecta tu stack, eliges enfoque y nivel de autonomía, y guarda `.dpx/config.toml`.

En **hack** con proyecto nuevo, dpx te pide tu idea y la pasa por el **comité** (4 roles) para sacar un plan antes de construir.

---

## Los tres modos

dpx tiene **un solo eje**: tres modos excluyentes. Lo que cambia es el **rol**, no la calidad. Cada modo tiene su propia identidad visual (color de acento y banner).

| Modo | Color | Qué hace | Cuándo |
|:---|:---|:---|:---|
| **code** | azul | Agente autónomo: implementa, ejecuta, verifica y corrige hasta dejarlo robusto. | Implementar features, corregir bugs, refactors. |
| **hack** | ámbar | Construye rápido pero **con criterio**: defaults sensatos, mínimo boilerplate, código correcto que corre ya. | Prototipos, hackathones, demos sólidas. |
| **learn** | verde | Tutor socrático: te hace pensar y te enseña conceptos, patrones y arquitectura. **Tú escribes el código**, él te guía. | Aprender, entender el porqué, fijar conocimiento. |

```bash
dpx code --focus spring-boot     # agente enfocado en Spring Boot
dpx hack --auto all              # construir rápido y sin preguntar
dpx learn                        # el tutor socrático

# en vivo, dentro de la sesión:
/modo hack                       # cambia de modo (y de color) al vuelo
```

---

## El cerebro (DeepSeek)

dpx corre sobre **DeepSeek** con dos niveles:

| Nivel | Modelo | Para qué |
|:---|:---|:---|
| **pro** | `deepseek-v4-pro` | El cerebro principal de cada turno. En learn usa razonamiento profundo; en code/hack responde rápido sin thinking. |
| **flash** | `deepseek-v4-flash` | ~**12× más barato**. Para subagentes de investigación y resúmenes de cierre. |

Ventana de contexto: **128k**. dpx **compacta automáticamente** la conversación al llegar al 75%, y antes de eso aligera los resultados de herramienta viejos para que la sesión dure más.

---

## Focus Packs (enfoques por stack)

Cada Focus Pack inyecta conocimiento de dominio en el prompt: versiones exactas, buenas prácticas y errores comunes de ese ecosistema. En **learn**, también aporta el **temario** del stack (`/temario`).

| Pack | Stack |
|:---|:---|
| `spring-boot` | Backend Java/Spring Boot |
| `react` | Frontend React (Vite, TanStack Query, RTL) |
| `node` | Backend Node.js (Fastify/Express, zod) |
| `python` | Backend Python con FastAPI |
| `rust` | Sistemas y CLIs en Rust |
| `gradle` | Proyecto JVM con Gradle (genérico) |
| `dpx` | El propio dpx: su arquitectura interna, para auto-editarse |

Sin `--focus`, dpx **detecta el stack automáticamente** por los archivos de la raíz (`pom.xml`, `package.json`, `Cargo.toml`…).

---

## Memoria de proyecto

dpx guarda el contexto de cada sesión en `.dpx/context.md`: estado del proyecto, progreso del usuario y próximos pasos. Al arrancar, lo retoma y pregunta si continuar.

En **learn**, también persiste el progreso de aprendizaje por tema en `.dpx/skills.md` y la racha de sesiones consecutivas en `.dpx/streak.md`.

Nada de esto se sube a git (añade `.dpx/` a tu `.gitignore`).

---

## Modo autónomo (`/auto`)

Disponible en **code** y **hack**. Se controla con `--auto <nivel>` o `/auto <nivel>` en el REPL.

| Nivel | Sin preguntar |
|:---|:---|
| `off` (default) | Nada: cada acción se confirma. |
| `reads` | Lecturas/búsquedas + auto-extiende rondas. |
| `writes` | + escrituras y ediciones (el diff se muestra igual). |
| `all` | + comandos **seguros**; tras escribir corre la **suite de tests** completa y se autocorrige. |

Las puertas de seguridad se mantienen **siempre**, incluso en `all`:

| Acción | ¿Pregunta aunque esté en `all`? |
|:---|:---|
| `write_file` que trunca >40% de un archivo grande | Sí |
| `write_file` sobre un archivo existente grande (≥200 líneas) | Sí |
| `run_command` peligroso (`rm -rf`, `git reset --hard`…) | Sí |
| `run_command` prohibido (`format`, `shutdown`, `mkfs`…) | Bloqueado siempre |
| `delete_file`, commits | Sí |

> [!TIP]
> Con `/undo` reviertes los archivos del último turno a su estado anterior.

---

## Comandos del REPL

Los nombres son **en español** (los ingleses siguen como alias). `/ayuda` muestra **solo los comandos de tu modo**.

| Comando | Disponible en | Acción |
|:---|:---|:---|
| `/ayuda` | todos | Lista los comandos de tu modo |
| `/estado` | todos | Config, cerebro, tokens |
| `/modelos` | todos | El cerebro y su API key |
| `/costo` | todos | Tokens reales + % de caché + costo aprox |
| `/presupuesto [N]` | todos | Tope de tokens de la sesión (ej. `/presupuesto 100k`) |
| `/contexto` | todos | La memoria guardada del proyecto |
| `/enfoque [id]` | todos | Cambia de stack (sin id: lista) |
| `/modo [code\|hack\|learn]` | todos | Cambia de modo (y de color) |
| `/cerebro` | todos | Info del cerebro activo |
| `/limpiar` | todos | Reinicia la conversación |
| `/compactar` | todos | Resume la charla para liberar contexto |
| `/undo` | todos | Revierte los archivos del último turno |
| `/actualizar` | todos | Recompila e instala dpx desde el repo |
| `/salir` | todos | Termina y guarda el contexto |
| `/auto [off\|all]` | code · hack | Nivel de autonomía |
| `/comité <idea>` | hack | El comité (4 roles) evalúa tu idea y da un plan |
| `/progreso` | learn | Tu progreso de aprendizaje por tema y racha |
| `/temario` | learn | El temario del stack y cuánto llevas |
| `/evaluar [tema]` | learn | El tutor te pregunta qué sabes antes de enseñarte |
| `/revisar [archivo]` | learn | Code review pedagógico: qué está bien, qué mejorar, por qué |
| `/examen [tema]` | learn | El tutor te interroga para fijar lo aprendido |

Referencias de archivo con `@ruta/al/archivo` en cualquier mensaje.

---

## Herramientas (function calling)

dpx expone estas herramientas nativas al modelo:

| Herramienta | Función |
|:---|:---|
| `read_file` | Leer archivos del proyecto |
| `search_project` | Buscar texto en todos los archivos |
| `web_search` | Buscar en DuckDuckGo (gratis, sin API key) |
| `spawn_agent` | Lanzar un subagente de investigación aislado (solo lectura) |
| `write_file` | Crear/sobrescribir archivos |
| `edit_file` | Editar fragmentos con SEARCH/REPLACE literal |
| `delete_file` | Borrar archivos |
| `run_command` | Ejecutar comandos de shell (con sandbox) |
| `git_status` · `git_diff` · `git_log` | Estado del repo (solo lectura) |
| `git_commit` | Crear commit (muta, pide confirmación) |

---

## Cómo funciona

### Ciclo de un turno

Cada mensaje dispara un **loop agéntico** de hasta **4 rondas** (ampliable):

1. (code/hack) Si la petición es de investigación, un **subagente flash** la resuelve y antepone su conclusión al turno principal.
2. El modelo responde con texto + tool calls.
3. dpx aplica escrituras/ediciones (con diff y confirmación), atiende lecturas/búsquedas (libres) y ejecuta comandos (con sandbox).
4. Los resultados se realimentan y **vuelve a iterar** hasta que el modelo cierra el turno.

Si el modelo falla a mitad de turno por error de red transitorio, **reintenta la ronda** sin perder el trabajo previo.

### Verificación automática

Al tocar código fuente, dpx propone **build + tests** automáticamente:

- En cualquier modo: sugiere ambos vía confirmación (puedes omitirlos).
- En `/auto all`: los ejecuta sin preguntar, realimenta errores y se autocorrige hasta verde (**green-gate**). En Rust corre además `clippy -D warnings` antes de los tests.

### Undo del último turno

Antes de cada `write_file`, `edit_file` o `delete_file`, dpx guarda el estado original del archivo en `.dpx/undo/`. El comando `/undo` restaura todos los archivos modificados por el turno anterior. Al empezar un nuevo turno, el snapshot anterior se descarta.

### Seguridad

- **Sandbox de comandos**: cada `run_command` se clasifica en seguro / peligroso / prohibido.
- **Rutas**: nada se escribe fuera del proyecto (rechaza `..` y paths absolutos).
- **Guard anti-truncado**: bloquea `write_file` que borraría >40% de un archivo grande.

### Mapa de símbolos y subagentes

- **Repo-map**: al arrancar, dpx mapea funciones/structs/clases por archivo y lo inyecta — el modelo lee menos y se equivoca menos.
- **Subagentes**: aislados, solo lectura. Devuelven solo su conclusión; su consumo cuenta en `/costo`.

---

## Modo learn en detalle

El tutor socrático no resuelve: **enseña**. Tú escribes el código, él te guía.

**Al arrancar** una sesión learn, dpx muestra:
- Racha de sesiones consecutivas (si aplica).
- Conceptos que toca repasar hoy (repaso espaciado automático).
- Siguiente tema del temario sugerido.

**Durante la sesión:**
- Método socrático: pistas graduales, preguntas que te llevan al error, cierre con retrieval practice.
- Registra automáticamente tu progreso (`dpx:skill`) en `visto / practicando / dominado`.

**Al cerrar**, muestra un resumen: qué aprendiste hoy, qué subió de nivel, racha actual y siguiente paso.

**`/progreso`** muestra tu progreso por tema, racha y logros desbloqueados (badges).

---

## Estructura del proyecto

```text
src/
├── main.rs            # Entrada, carga .env
├── config.rs          # Config del proyecto (.dpx/config.toml)
├── ui.rs              # Capa visual: tema por modo, markdown, spinner, paneles
├── skill.rs           # Progreso de aprendizaje del usuario (modo learn)
├── streak.rs          # Racha de sesiones consecutivas
├── token.rs           # Ledger de tokens reales, costo y presupuesto
├── cli/               # CLI (clap), REPL, editor propio, init
├── agent/             # Model Router, tools, búsqueda web, subagentes
├── focus/             # Focus Packs, modos, comité, temario (curriculum)
├── fs/                # Escritura/edición, repo-map, sandbox, detect build/test
└── session/           # Persistencia .dpx/ (context, skills, streak, undo, plan)
```

---

## Configuración

`.dpx/config.toml` (creado por el onboarding o `dpx init`):

```toml
focus = "spring-boot"
mode  = "code"      # code | hack | learn
auto  = "off"       # off | reads | writes | all
```

Los flags de CLI (`--focus`, `--auto`) pisan estos defaults, y los comandos del REPL (`/enfoque`, `/modo`, `/auto`) los cambian en caliente.

---

## Desarrollo

```bash
cargo check                                      # compilación rápida
cargo test                                       # 143 tests
cargo clippy --all-targets -- -D warnings        # linter estricto (cero warnings)
```

Dentro del repo de dpx, **`/actualizar`** recompila e instala el binario sin cerrar la sesión (en Windows renombra el exe en uso antes de instalar).

> [!NOTE]
> En Windows, `cargo install` falla con `os error 5` si tienes una sesión de dpx abierta (el `.exe` está bloqueado). Cierra la sesión o usa `/actualizar`.

---

## Licencia

**Proyecto privado. Todos los derechos reservados.**

Este software no se distribuye bajo ninguna licencia de código abierto: no hay permiso de uso, copia, modificación ni distribución salvo autorización explícita del autor.
