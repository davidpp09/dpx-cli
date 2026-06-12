//! Model Router: elige el "cerebro" y construye los agentes.
//!
//! El cerebro es seleccionable en tiempo de ejecución (`--brain`). Cada proveedor
//! de `rig-core` tiene su propio tipo de `CompletionModel`, así que envolvemos el
//! `Agent` resultante en el enum [`Mentor`] para poder despachar dinámicamente.

use anyhow::{Result, anyhow};
use clap::ValueEnum;
use futures::StreamExt;
use rig_core::OneOrMany;
use rig_core::agent::Agent;
use rig_core::client::{CompletionClient, ProviderClient};
use rig_core::completion::{AssistantContent, Message, Prompt};
use rig_core::message::ToolCall;
use rig_core::providers::{deepseek, moonshot, openrouter};
use rig_core::streaming::{StreamedAssistantContent, StreamingCompletion};

use crate::focus::Mode;

/// Lo que devolvió el modelo en un turno: la narración en texto y las
/// llamadas a herramientas nativas (function calling), si las hubo.
pub struct ChatReply {
    pub text: String,
    pub calls: Vec<ToolCall>,
}

/// Proveedor que hace de cerebro mentor. Los tres del plan anti-suscripción:
/// DeepSeek (principal), Kimi y Qwen — todos fuertes en tool-calling nativo.
#[derive(Clone, Copy, Debug, PartialEq, ValueEnum)]
pub enum Brain {
    /// DeepSeek (razonador): el cerebro principal, fuerte en código y agéntico.
    Deepseek,
    /// Kimi K2.5 (Moonshot): agéntico sólido y contexto largo.
    Kimi,
    /// Qwen3 Coder (vía OpenRouter): especializado en código, muy barato.
    Qwen,
}

/// Model ID de Qwen en OpenRouter (no hay constante en rig-core para este).
const QWEN_CODER: &str = "qwen/qwen3-coder";

impl Brain {
    /// ID de modelo a usar en cada proveedor (cf. constantes de `rig-core`).
    fn model_id(self) -> &'static str {
        match self {
            Brain::Deepseek => DEEPSEEK_PRO,
            Brain::Kimi => moonshot::KIMI_K2_5,
            Brain::Qwen => QWEN_CODER,
        }
    }

    /// Nombre legible y completo del cerebro (para banners y `/status`).
    pub fn label(self) -> &'static str {
        match self {
            Brain::Deepseek => "DeepSeek Reasoner",
            Brain::Kimi => "Kimi K2.5",
            Brain::Qwen => "Qwen3 Coder",
        }
    }

    /// Identificador corto en minúsculas (el que se usa en `/brain <id>`).
    pub fn name(self) -> &'static str {
        match self {
            Brain::Deepseek => "deepseek",
            Brain::Kimi => "kimi",
            Brain::Qwen => "qwen",
        }
    }

    /// Superpoder del modelo en una frase (para que sepas a cuál cambiar y por qué).
    pub fn capability(self) -> &'static str {
        match self {
            Brain::Deepseek => "principal · razona · agéntico fuerte",
            Brain::Kimi => "agéntico · contexto largo",
            Brain::Qwen => "código · barato (vía OpenRouter)",
        }
    }

    /// Ventana de contexto utilizable del modelo (tokens). Gobierna la barra
    /// de `/status` y el umbral de compactación automática: con Kimi o Qwen
    /// no tiene sentido compactar a los 128k de DeepSeek — cada cerebro
    /// aprovecha su ventana real.
    pub fn context_budget(self) -> usize {
        match self {
            Brain::Deepseek => 128_000,
            Brain::Kimi => 256_000,
            Brain::Qwen => 256_000,
        }
    }

    /// Variable de entorno con la API key de este proveedor.
    pub fn env_var(self) -> &'static str {
        match self {
            Brain::Deepseek => "DEEPSEEK_API_KEY",
            Brain::Kimi => "MOONSHOT_API_KEY",
            Brain::Qwen => "OPENROUTER_API_KEY",
        }
    }

    /// ¿Hay una API key configurada (no vacía) para este cerebro en el entorno?
    pub fn has_key(self) -> bool {
        std::env::var(self.env_var()).map(|v| !v.trim().is_empty()).unwrap_or(false)
    }

    /// Todos los cerebros, en orden de utilidad para dpx (agéntico primero).
    pub fn all() -> [Brain; 3] {
        [Brain::Deepseek, Brain::Kimi, Brain::Qwen]
    }

    /// Parsea el nombre de un cerebro (para el comando `/brain`).
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "deepseek" => Some(Brain::Deepseek),
            "kimi" | "moonshot" => Some(Brain::Kimi),
            "qwen" => Some(Brain::Qwen),
            _ => None,
        }
    }

    /// Construye un agente con este cerebro, el system prompt y la temperatura dados.
    /// `extra` son campos adicionales para el body (p.ej. `thinking`/`reasoning_effort`
    /// en DeepSeek); en otros proveedores debe ir `None`.
    fn build(self, preamble: &str, temperature: f64, extra: Option<serde_json::Value>) -> Result<Mentor> {
        Ok(match self {
            Brain::Deepseek => {
                let c = deepseek::Client::from_env()
                    .map_err(|e| anyhow!("No pude iniciar DeepSeek (¿falta DEEPSEEK_API_KEY?): {e}"))?;
                Mentor::Deepseek(agent(c.agent(self.model_id()), preamble, temperature, extra))
            }
            Brain::Kimi => {
                let c = moonshot::Client::from_env()
                    .map_err(|e| anyhow!("No pude iniciar Kimi (¿falta MOONSHOT_API_KEY?): {e}"))?;
                Mentor::Kimi(agent(c.agent(self.model_id()), preamble, temperature, extra))
            }
            Brain::Qwen => {
                let c = openrouter::Client::from_env()
                    .map_err(|e| anyhow!("No pude iniciar Qwen (¿falta OPENROUTER_API_KEY?): {e}"))?;
                Mentor::Qwen(agent(c.agent(self.model_id()), preamble, temperature, extra))
            }
        })
    }
}

/// Model IDs de DeepSeek: `pro` razona (cerebro principal), `flash` es 12x más
/// barato para tareas mecánicas (resúmenes, verificación).
const DEEPSEEK_PRO: &str = "deepseek-reasoner";
const DEEPSEEK_FLASH: &str = "deepseek-chat";

/// Body extra para activar el "thinking" de DeepSeek con el effort dado.
/// La API solo acepta `"high"` o `"max"` (cf. docs: `xhigh`→`max`, `low/medium`→`high`).
fn deepseek_thinking(effort: &str) -> serde_json::Value {
    serde_json::json!({ "thinking": { "type": "enabled" }, "reasoning_effort": effort })
}

/// Body extra para desactivar el "thinking" (modo no-pensante, más rápido/barato).
fn deepseek_no_thinking() -> serde_json::Value {
    serde_json::json!({ "thinking": { "type": "disabled" } })
}

/// Construye un agente DeepSeek con un model ID y `additional_params` concretos.
fn build_deepseek(
    model_id: &str,
    preamble: &str,
    temperature: f64,
    extra: serde_json::Value,
) -> Result<Mentor> {
    let c = deepseek::Client::from_env()
        .map_err(|e| anyhow!("No pude iniciar DeepSeek (¿falta DEEPSEEK_API_KEY?): {e}"))?;
    Ok(Mentor::Deepseek(agent(c.agent(model_id), preamble, temperature, Some(extra))))
}

/// Helper genérico: aplica preamble + temperatura (+ params extra) y construye el `Agent`.
fn agent<M: rig_core::completion::CompletionModel>(
    builder: rig_core::agent::AgentBuilder<M>,
    preamble: &str,
    temperature: f64,
    extra: Option<serde_json::Value>,
) -> Agent<M> {
    let builder = builder.preamble(preamble).temperature(temperature);
    let builder = match extra {
        Some(params) => builder.additional_params(params),
        None => builder,
    };
    builder.build()
}

/// Agente envuelto por proveedor, para despacho dinámico.
pub enum Mentor {
    Deepseek(Agent<deepseek::CompletionModel>),
    Kimi(Agent<moonshot::CompletionModel>),
    Qwen(Agent<openrouter::CompletionModel>),
}

/// Reintentos ante errores transitorios del proveedor (saturación).
const MAX_RETRIES: u32 = 4;

/// Itera el stream de bajo nivel de un agente concreto, anunciando las
/// herramientas nativas de dpx. Emite cada delta de texto por `on_delta` y
/// recoge los tool calls SIN ejecutarlos (las confirmaciones son del CLI, no
/// de rig). Como este camino no actualiza el historial solo, lo extendemos
/// nosotros al terminar: [user input, assistant (texto + tool calls)]. Es un
/// macro (no función genérica) para esquivar los bounds por proveedor.
macro_rules! stream_dispatch {
    ($agent:expr, $input:expr, $history:expr, $on_delta:expr) => {{
        let mut stream = $agent
            .stream_completion($input, $history.clone())
            .await
            .map_err(|e| anyhow!("{e}"))?
            .tools(crate::agent::tools::definitions())
            .stream()
            .await
            .map_err(|e| anyhow!("{e}"))?;
        let mut full = String::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(StreamedAssistantContent::Text(t)) => {
                    ($on_delta)(&t.text);
                    full.push_str(&t.text);
                }
                // Los tool calls completos se acumulan en `stream.choice`.
                Ok(_) => {}
                Err(e) => return Err(anyhow!("{e}")),
            }
        }
        let calls: Vec<ToolCall> = stream
            .choice
            .iter()
            .filter_map(|c| match c {
                AssistantContent::ToolCall(tc) => Some(tc.clone()),
                _ => None,
            })
            .collect();
        $history.push(Message::user($input.to_string()));
        $history.push(Message::Assistant {
            id: None,
            content: assistant_choice(&full, &calls),
        });
        Ok::<ChatReply, anyhow::Error>(ChatReply { text: full, calls })
    }};
}

/// Contenido del mensaje assistant para el historial: el texto completo más
/// los tool calls de la ronda (al menos un item, que `OneOrMany` exige).
fn assistant_choice(text: &str, calls: &[ToolCall]) -> OneOrMany<AssistantContent> {
    let mut items: Vec<AssistantContent> = Vec::new();
    if !text.is_empty() || calls.is_empty() {
        items.push(AssistantContent::text(text));
    }
    items.extend(calls.iter().cloned().map(AssistantContent::ToolCall));
    OneOrMany::many(items).unwrap_or_else(|_| OneOrMany::one(AssistantContent::text(text)))
}

impl Mentor {
    /// Turno conversacional en streaming con las herramientas nativas de dpx
    /// anunciadas: cada fragmento de texto se entrega por `on_delta` según
    /// llega del modelo. Devuelve el texto completo + los tool calls (sin
    /// ejecutar) y extiende el historial. Reintenta ante errores transitorios
    /// SOLO si aún no se emitió nada (para no duplicar texto ya impreso).
    pub async fn chat_stream(
        &self,
        input: &str,
        history: &mut Vec<Message>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<ChatReply> {
        let mut attempt = 0;
        loop {
            let mut emitted = false;
            let res = {
                let mut wrap = |d: &str| {
                    emitted = true;
                    on_delta(d);
                };
                self.stream_once(input, history, &mut wrap).await
            };
            match res {
                Ok(full) => return Ok(full),
                Err(e) => {
                    if emitted {
                        return Err(e); // ya imprimimos texto: no reintentar
                    }
                    match next_backoff(&e, &mut attempt) {
                        Some(delay) => tokio::time::sleep(delay).await,
                        None => return Err(e),
                    }
                }
            }
        }
    }

    /// Un único intento de streaming (sin reintentos), despachando por proveedor.
    async fn stream_once(
        &self,
        input: &str,
        history: &mut Vec<Message>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<ChatReply> {
        match self {
            Mentor::Deepseek(a) => stream_dispatch!(a, input, history, on_delta),
            Mentor::Kimi(a) => stream_dispatch!(a, input, history, on_delta),
            Mentor::Qwen(a) => stream_dispatch!(a, input, history, on_delta),
        }
    }

    /// Llamada de un solo turno (sin historial), con la misma política de reintentos.
    pub async fn prompt(&self, content: &str) -> Result<String> {
        let mut attempt = 0;
        loop {
            let r = match self {
                Mentor::Deepseek(a) => a.prompt(content).await,
                Mentor::Kimi(a) => a.prompt(content).await,
                Mentor::Qwen(a) => a.prompt(content).await,
            };
            match r {
                Ok(s) => return Ok(s),
                Err(e) => match next_backoff(&e, &mut attempt) {
                    Some(delay) => tokio::time::sleep(delay).await,
                    None => return Err(anyhow!("{e}")),
                },
            }
        }
    }
}

/// Decide si reintentar: incrementa `attempt` y devuelve el tiempo de espera, o
/// `None` si el error no es transitorio o se agotaron los reintentos.
fn next_backoff<E: std::fmt::Display>(error: &E, attempt: &mut u32) -> Option<std::time::Duration> {
    if !is_transient(error) || *attempt >= MAX_RETRIES {
        return None;
    }
    *attempt += 1;
    // Backoff exponencial: 1s, 2s, 4s, 8s.
    Some(std::time::Duration::from_secs(1 << (*attempt - 1)))
}

/// ¿Un mensaje de error es transitorio? Versión pública para que el loop del
/// turno decida si reintentar una ronda cortada a mitad (red caída) o rendirse.
pub fn is_transient_error(error: &str) -> bool {
    is_transient(&error)
}

/// ¿El error es transitorio (saturación / indisponibilidad pasajera)?
/// Un 402 (sin saldo) o un 401 (key inválida) NO lo son: no vale reintentar.
fn is_transient<E: std::fmt::Display>(error: &E) -> bool {
    let s = error.to_string();
    [
        "503",
        "502",
        "500",
        "429",
        "529",
        "UNAVAILABLE",
        "overloaded",
        "high demand",
        // Errores de red transitorios (cortes, timeouts, DNS).
        "error sending request",
        "Http client error",
        "connection",
        "timed out",
        "timeout",
        "dns error",
    ]
    .iter()
    .any(|needle| s.contains(needle))
}

/// Enruta el trabajo entre los modelos disponibles.
pub struct ModelRouter {
    brain: Brain,
}

impl ModelRouter {
    pub fn new(brain: Brain) -> Self {
        Self { brain }
    }

    /// Nombre legible del cerebro activo (para el banner).
    pub fn brain_label(&self) -> &'static str {
        self.brain.label()
    }

    /// El mentor para la sesión: cerebro + system prompt + temperatura según modo.
    /// En DeepSeek, ajusta el "thinking" según el modo: `pro` razona a fondo (`max`),
    /// `hack` va más rápido (`high`).
    pub fn mentor(&self, preamble: &str, mode: Mode) -> Result<Mentor> {
        let temperature = match mode {
            Mode::Pro => 0.4,
            Mode::Hack => 0.7,
        };
        match self.brain {
            Brain::Deepseek => {
                let effort = match mode {
                    Mode::Pro => "max",
                    Mode::Hack => "high",
                };
                build_deepseek(DEEPSEEK_PRO, preamble, temperature, deepseek_thinking(effort))
            }
            other => other.build(preamble, temperature, None),
        }
    }

    /// Resumen de cierre de sesión (un turno, baja temperatura). Tarea mecánica:
    /// en DeepSeek usamos `flash` SIN thinking (12x más barato), no el caro `pro`.
    pub async fn summarize(&self, preamble: &str, content: &str) -> Result<String> {
        let mentor = match self.brain {
            Brain::Deepseek => build_deepseek(DEEPSEEK_FLASH, preamble, 0.2, deepseek_no_thinking())?,
            other => other.build(preamble, 0.2, None)?,
        };
        mentor.prompt(content).await
    }
}
