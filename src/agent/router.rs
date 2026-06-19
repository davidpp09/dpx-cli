//! Model Router: DeepSeek como unico proveedor con dos tiers (pro/flash).

use anyhow::{Result, anyhow};
use futures::StreamExt;
use rig_core::OneOrMany;
use rig_core::agent::Agent;
use rig_core::client::{CompletionClient, ProviderClient};
use rig_core::completion::{AssistantContent, Message, Prompt};
use rig_core::message::ToolCall;
use rig_core::providers::deepseek;
use rig_core::streaming::{StreamedAssistantContent, StreamingCompletion};

use crate::focus::Mode;

pub struct ChatReply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub usage: Option<rig_core::completion::Usage>,
}

// ── DeepSeek constants ──────────────────────────────────────────────
pub const BRAIN_LABEL: &str = "DeepSeek Reasoner";
pub const BRAIN_NAME: &str = "deepseek";
pub const CONTEXT_BUDGET: usize = 128_000;
pub const ENV_VAR: &str = "DEEPSEEK_API_KEY";

pub fn has_key() -> bool {
    std::env::var(ENV_VAR).map(|v| !v.trim().is_empty()).unwrap_or(false)
}

const DEEPSEEK_PRO: &str = "deepseek-v4-pro";
const DEEPSEEK_FLASH: &str = "deepseek-v4-flash";

fn deepseek_thinking(effort: &str) -> serde_json::Value {
    serde_json::json!({ "thinking": { "type": "enabled" }, "reasoning_effort": effort })
}

fn deepseek_no_thinking() -> serde_json::Value {
    serde_json::json!({ "thinking": { "type": "disabled" } })
}

fn build_deepseek(
    model_id: &str,
    preamble: &str,
    temperature: f64,
    extra: serde_json::Value,
) -> Result<Mentor> {
    let c = deepseek::Client::from_env()
        .map_err(|e| anyhow!("No pude iniciar DeepSeek (falta DEEPSEEK_API_KEY?): {e}"))?;
    Ok(Mentor(agent(c.agent(model_id), preamble, temperature, Some(extra))))
}

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

pub struct Mentor(Agent<deepseek::CompletionModel>);

const MAX_RETRIES: u32 = 4;

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
        let usage = stream
            .response
            .as_ref()
            .and_then(rig_core::completion::GetTokenUsage::token_usage);
        $history.push(Message::user($input.to_string()));
        $history.push(Message::Assistant {
            id: None,
            content: assistant_choice(&full, &calls),
        });
        Ok::<ChatReply, anyhow::Error>(ChatReply { text: full, calls, usage })
    }};
}

fn assistant_choice(text: &str, calls: &[ToolCall]) -> OneOrMany<AssistantContent> {
    let mut items: Vec<AssistantContent> = Vec::new();
    if !text.is_empty() || calls.is_empty() {
        items.push(AssistantContent::text(text));
    }
    items.extend(calls.iter().cloned().map(AssistantContent::ToolCall));
    OneOrMany::many(items).unwrap_or_else(|_| OneOrMany::one(AssistantContent::text(text)))
}

impl Mentor {
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
                        return Err(e);
                    }
                    match next_backoff(&e, &mut attempt) {
                        Some(delay) => tokio::time::sleep(delay).await,
                        None => return Err(e),
                    }
                }
            }
        }
    }

    async fn stream_once(
        &self,
        input: &str,
        history: &mut Vec<Message>,
        on_delta: &mut dyn FnMut(&str),
    ) -> Result<ChatReply> {
        match self {
            Mentor(a) => stream_dispatch!(a, input, history, on_delta),
        }
    }

    pub async fn prompt(&self, content: &str) -> Result<String> {
        let mut attempt = 0;
        loop {
            let r = match self {
                Mentor(a) => a.prompt(content).await,
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

fn next_backoff<E: std::fmt::Display>(error: &E, attempt: &mut u32) -> Option<std::time::Duration> {
    if !is_transient(error) || *attempt >= MAX_RETRIES {
        return None;
    }
    *attempt += 1;
    Some(std::time::Duration::from_secs(1 << (*attempt - 1)))
}

pub fn is_transient_error(error: &str) -> bool {
    is_transient(&error)
}

fn is_transient<E: std::fmt::Display>(error: &E) -> bool {
    let s = error.to_string();
    [
        "503", "502", "500", "429", "529",
        "UNAVAILABLE", "overloaded", "high demand",
        "error sending request", "Http client error",
        "connection", "timed out", "timeout", "dns error",
    ]
    .iter()
    .any(|needle| s.contains(needle))
}

// ── ModelRouter (sin estado, DeepSeek hardcodeado) ──────────────────
pub struct ModelRouter;

impl ModelRouter {
    pub fn new() -> Self {
        Self
    }

    pub fn brain_label(&self) -> &'static str {
        BRAIN_LABEL
    }

    pub fn mentor(&self, preamble: &str, mode: Mode) -> Result<Mentor> {
        let (temperature, extra) = match mode {
            Mode::Code => (0.4, deepseek_no_thinking()),
            Mode::Hack => (0.55, deepseek_no_thinking()),
            Mode::Learn => (0.5, deepseek_thinking("max")),
        };
        build_deepseek(DEEPSEEK_PRO, preamble, temperature, extra)
    }

    pub fn subagent_mentor(&self, preamble: &str) -> Result<Mentor> {
        build_deepseek(DEEPSEEK_FLASH, preamble, 0.2, deepseek_no_thinking())
    }

    pub async fn summarize(&self, preamble: &str, content: &str) -> Result<String> {
        let mentor = build_deepseek(DEEPSEEK_FLASH, preamble, 0.2, deepseek_no_thinking())?;
        mentor.prompt(content).await
    }

    pub async fn flash_prompt(&self, preamble: &str, user: &str) -> Result<String> {
        let mentor = build_deepseek(DEEPSEEK_FLASH, preamble, 0.0, deepseek_no_thinking())?;
        mentor.prompt(user).await
    }
}

#[cfg(test)]
mod integration {
    use super::*;
    use crate::focus::Mode;
    use rig_core::completion::Message;

    fn cargar_env() {
        dotenvy::dotenv().ok();
        if let Some(home) = dirs::home_dir() {
            dotenvy::from_path(home.join(".dpx").join(".env")).ok();
        }
    }

    fn mentor_deepseek() -> Mentor {
        cargar_env();
        ModelRouter::new()
            .mentor("Eres un asistente de pruebas. Responde muy corto.", Mode::Hack)
            .expect("no pude construir el mentor DeepSeek (falta DEEPSEEK_API_KEY?)")
    }

    #[tokio::test]
    #[ignore = "requiere red + DEEPSEEK_API_KEY"]
    async fn streaming_responde_y_extiende_historial() {
        let m = mentor_deepseek();
        let mut history: Vec<Message> = Vec::new();
        let mut emitido = String::new();
        let reply = m
            .chat_stream("Responde solo: pong", &mut history, &mut |d| emitido.push_str(d))
            .await
            .expect("el stream fallo");
        assert!(!reply.text.trim().is_empty(), "respuesta vacia");
        assert!(!emitido.trim().is_empty(), "no se emitio ningun delta por on_delta");
        assert_eq!(history.len(), 2, "el historial no quedo bien formado");
    }

    #[tokio::test]
    #[ignore = "requiere red + DEEPSEEK_API_KEY"]
    async fn streaming_reporta_usage_real() {
        let m = mentor_deepseek();
        let mut history: Vec<Message> = Vec::new();
        let reply = m
            .chat_stream("Di hola.", &mut history, &mut |_| {})
            .await
            .expect("el stream fallo");
        let usage = reply.usage.expect("DeepSeek no reporto usage en streaming");
        assert!(
            usage.input_tokens > 0 && usage.output_tokens > 0,
            "usage con ceros: {usage:?}"
        );
    }

    #[tokio::test]
    #[ignore = "requiere red + DEEPSEEK_API_KEY"]
    async fn tool_calling_no_rompe_el_protocolo() {
        let m = mentor_deepseek();
        let mut history: Vec<Message> = Vec::new();
        let reply = m
            .chat_stream(
                "Usa tus herramientas para leer el archivo Cargo.toml.",
                &mut history,
                &mut |_| {},
            )
            .await
            .expect("el stream con tools fallo");
        assert!(
            !reply.text.trim().is_empty() || !reply.calls.is_empty(),
            "ni texto ni tool calls: el turno salio vacio"
        );
        assert_eq!(history.len(), 2);
    }
}
