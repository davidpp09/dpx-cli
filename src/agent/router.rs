//! Model Router: DeepSeek como unico proveedor con dos tiers (pro/flash).

use anyhow::{Result, anyhow};
use futures::StreamExt;
use rig_core::OneOrMany;
use rig_core::agent::Agent;
use rig_core::client::{CompletionClient, ProviderClient};
use rig_core::completion::{AssistantContent, Message, ToolDefinition};
use rig_core::message::ToolCall;
use rig_core::providers::deepseek;
use rig_core::streaming::{StreamedAssistantContent, StreamingCompletion};

use crate::focus::Mode;
use crate::token::Tier;

pub struct ChatReply {
    pub text: String,
    pub calls: Vec<ToolCall>,
    pub usage: Option<rig_core::completion::Usage>,
}

// ── DeepSeek constants ──────────────────────────────────────────────
pub const BRAIN_LABEL: &str = "DeepSeek Reasoner";
pub const BRAIN_NAME: &str = "deepseek";

/// Ventana de contexto REAL de los modelos v4 (pro y flash): 1M tokens, con
/// 384K de salida máxima. No es el presupuesto que usa dpx (ver
/// [`CONTEXT_BUDGET`]); está aquí como referencia de cuál es el techo duro.
pub const CONTEXT_WINDOW: usize = 1_000_000;

/// Presupuesto de contexto con el que opera dpx: el punto a partir del cual el
/// historial se poda (50%) y se compacta (75%). Corre DEBAJO de
/// [`CONTEXT_WINDOW`] a propósito: no por límite del modelo, sino porque el
/// prefill de un turno de ~750k tokens se siente lento en una TUI. 400k deja
/// sesiones largas sin amnesia (compacta a los 300k) y turnos que responden
/// rápido. Súbelo si en tu uso real la compactación sigue llegando temprano.
pub const CONTEXT_BUDGET: usize = 400_000;
pub const ENV_VAR: &str = "DEEPSEEK_API_KEY";

pub fn has_key() -> bool {
    std::env::var(ENV_VAR).map(|v| !v.trim().is_empty()).unwrap_or(false)
}

/// Modelo del cerebro PRO (principal). Override con `DEEPSEEK_MODEL_PRO` por si
/// el nombre en tu plan de DeepSeek difiere del default.
fn deepseek_pro() -> String {
    std::env::var("DEEPSEEK_MODEL_PRO").unwrap_or_else(|_| "deepseek-v4-pro".to_string())
}

/// Modelo FLASH (barato) para subagentes, resúmenes y clasificación. Override con
/// `DEEPSEEK_MODEL_FLASH`. CLAVE: si este nombre NO existe en tu plan, las
/// llamadas flash fallan y TODO el trabajo cae al cerebro pro (más caro y lento)
/// — por eso en el dashboard verías solo "pro".
fn deepseek_flash() -> String {
    std::env::var("DEEPSEEK_MODEL_FLASH").unwrap_or_else(|_| "deepseek-v4-flash".to_string())
}

/// ID real del modelo de cada tier.
pub fn model_id(tier: Tier) -> String {
    match tier {
        Tier::Pro => deepseek_pro(),
        Tier::Flash => deepseek_flash(),
    }
}

/// Nivel de razonamiento (`reasoning_effort`) que se le pide al modelo.
///
/// Los v4 exponen tres modos: sin thinking, `high` y `max`. Según los evals de
/// DeepSeek, subir el esfuerzo mueve más la calidad que subir de tier — por eso
/// es una palanca explícita y no un detalle escondido en un `json!` suelto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    /// Sin thinking: respuesta inmediata.
    Off,
    High,
    Max,
}

impl Effort {
    /// El string que espera la API en `reasoning_effort` (y la etiqueta que
    /// mostramos). `Off` no viaja como esfuerzo: apaga el thinking entero.
    pub fn label(self) -> &'static str {
        match self {
            Effort::Off => "no-think",
            Effort::High => "high",
            Effort::Max => "max",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "off" | "none" | "no-think" | "nothink" | "disabled" => Some(Effort::Off),
            "high" => Some(Effort::High),
            "max" | "xhigh" => Some(Effort::Max),
            _ => None,
        }
    }

    fn params(self) -> serde_json::Value {
        match self {
            Effort::Off => serde_json::json!({ "thinking": { "type": "disabled" } }),
            other => serde_json::json!({
                "thinking": { "type": "enabled" },
                "reasoning_effort": other.label(),
            }),
        }
    }
}

/// Esfuerzo de razonamiento del modo `learn`. Override con `DPX_EFFORT_LEARN`
/// (`off`, `high` o `max`): si tu plan no acepta `max`, el thinking se cae en
/// silencio y learn deja de razonar sin avisar. `dpx bench --probe` te dice
/// cuál acepta tu cuenta de verdad.
fn learn_effort() -> Effort {
    std::env::var("DPX_EFFORT_LEARN")
        .ok()
        .and_then(|v| Effort::parse(&v))
        .unwrap_or(Effort::Max)
}

/// Tier del cerebro en `code`/`hack`. Override con `DPX_BRAIN_TIER`
/// (`pro`|`flash`).
///
/// El default es `flash` desde el banco del 2026-08-04 (144 corridas, 12 casos,
/// 3 repeticiones): contra `pro` sin thinking empató en aciertos (33/33 los
/// dos), fue un 31% más rápido y costó 2.4x menos. El update V4-Flash-0731
/// reorientó ese modelo a trabajo agéntico, que es exactamente lo que hace dpx.
///
/// Vuelve atrás con `DPX_BRAIN_TIER=pro` si en uso real notas degradación:
/// el banco mide tareas de 3-5 rondas, y un turno autónomo largo es otra cosa.
pub fn brain_tier() -> Tier {
    std::env::var("DPX_BRAIN_TIER")
        .ok()
        .and_then(|v| Tier::parse(&v))
        .unwrap_or(Tier::Flash)
}

/// Esfuerzo del cerebro en `code`/`hack`. Override con `DPX_BRAIN_EFFORT`
/// (`off`|`high`|`max`). Por defecto sin thinking: son los modos de respuesta
/// inmediata.
fn brain_effort() -> Effort {
    std::env::var("DPX_BRAIN_EFFORT")
        .ok()
        .and_then(|v| Effort::parse(&v))
        .unwrap_or(Effort::Off)
}

fn build_deepseek(
    model_id: &str,
    preamble: &str,
    temperature: f64,
    effort: Effort,
    tools: Vec<ToolDefinition>,
) -> Result<Mentor> {
    let c = deepseek::Client::from_env()
        .map_err(|e| anyhow!("No pude iniciar DeepSeek (falta DEEPSEEK_API_KEY?): {e}"))?;
    Ok(Mentor {
        agent: agent(c.agent(model_id), preamble, temperature, Some(effort.params())),
        tools,
    })
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

/// Un agente ya configurado: modelo, preamble, temperatura, esfuerzo y —clave—
/// el set de herramientas que se le ANUNCIA. Cada agente ve solo las suyas: al
/// subagente de solo lectura no se le ofrecen `write_file` ni `run_command`, así
/// no gasta rondas pidiendo cosas que el ejecutor le va a rechazar.
pub struct Mentor {
    agent: Agent<deepseek::CompletionModel>,
    tools: Vec<ToolDefinition>,
}

const MAX_RETRIES: u32 = 4;

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
        let mut stream = self
            .agent
            .stream_completion(input, history.clone())
            .await
            .map_err(|e| anyhow!("{e}"))?
            .tools(self.tools.clone())
            .stream()
            .await
            .map_err(|e| anyhow!("{e}"))?;

        let mut full = String::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(StreamedAssistantContent::Text(t)) => {
                    on_delta(&t.text);
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

        history.push(Message::user(input.to_string()));
        history.push(Message::Assistant {
            id: None,
            content: assistant_choice(&full, &calls),
        });
        Ok(ChatReply { text: full, calls, usage })
    }

    /// Un turno suelto sin historial, devolviendo TAMBIÉN lo que consumió.
    ///
    /// Va por el camino de streaming a propósito: `Agent::prompt` devuelve solo
    /// el texto, así que todo lo que pasaba por ahí —compactaciones, `/recall`,
    /// el clasificador de delegación— se gastaba sin aparecer en el ledger. Era
    /// dinero invisible: poco (todo corre en flash), pero invisible.
    pub async fn prompt_metered(
        &self,
        content: &str,
    ) -> Result<(String, Option<rig_core::completion::Usage>)> {
        let mut attempt = 0;
        loop {
            // Historial local y descartable: este camino no conversa.
            let mut history: Vec<Message> = Vec::new();
            match self.stream_once(content, &mut history, &mut |_| {}).await {
                Ok(reply) => return Ok((reply.text, reply.usage)),
                Err(e) => match next_backoff(&e, &mut attempt) {
                    Some(delay) => tokio::time::sleep(delay).await,
                    None => return Err(e),
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

    /// IDs reales de modelo que se envían a DeepSeek: `(pro, flash)`. Útil para
    /// verificar contra el dashboard que el flash es el correcto (si no, ajusta
    /// `DEEPSEEK_MODEL_FLASH`).
    pub fn model_ids(&self) -> (String, String) {
        (model_id(Tier::Pro), model_id(Tier::Flash))
    }

    /// El cerebro de cada turno, con TODAS las herramientas. El tier sale de
    /// [`brain_tier`]; quien contabilice su consumo debe usar ESE tier, no dar
    /// `pro` por hecho (las tarifas se diferencian 3.1x).
    pub fn mentor(&self, preamble: &str, mode: Mode) -> Result<Mentor> {
        let (temperature, effort) = match mode {
            Mode::Code => (0.4, brain_effort()),
            Mode::Hack => (0.55, brain_effort()),
            Mode::Learn => (0.5, learn_effort()),
        };
        build_deepseek(
            &model_id(brain_tier()),
            preamble,
            temperature,
            effort,
            crate::agent::tools::definitions(),
        )
    }

    /// Subagente de investigación: tier flash y SOLO las herramientas de lectura.
    /// El preamble ya le dice que es de solo lectura; anunciarle además
    /// `write_file`/`run_command`/`git_commit` solo lograba que las pidiera y
    /// quemara rondas contra un ejecutor que las rechaza.
    pub fn subagent_mentor(&self, preamble: &str) -> Result<Mentor> {
        build_deepseek(
            &model_id(Tier::Flash),
            preamble,
            0.2,
            Effort::Off,
            crate::agent::tools::definitions_read_only(),
        )
    }

    /// Agente a medida para el banco de pruebas (`dpx bench`): tier, esfuerzo y
    /// herramientas explícitos, para comparar configuraciones manzana con manzana.
    pub fn tuned_mentor(
        &self,
        tier: Tier,
        effort: Effort,
        preamble: &str,
        temperature: f64,
        tools: Vec<ToolDefinition>,
    ) -> Result<Mentor> {
        build_deepseek(&model_id(tier), preamble, temperature, effort, tools)
    }

    pub async fn summarize(&self, preamble: &str, content: &str) -> Result<String> {
        // Resumir no usa herramientas: no se le anuncia ninguna.
        let mentor = build_deepseek(&model_id(Tier::Flash), preamble, 0.2, Effort::Off, Vec::new())?;
        Self::metered(&mentor, content).await
    }

    pub async fn flash_prompt(&self, preamble: &str, user: &str) -> Result<String> {
        let mentor = build_deepseek(&model_id(Tier::Flash), preamble, 0.0, Effort::Off, Vec::new())?;
        Self::metered(&mentor, user).await
    }

    /// Lanza un turno de flash y lo APUNTA en el ledger antes de devolverlo.
    /// Todo lo que consume dpx pasa por aquí o por el loop principal; si añades
    /// un tercer camino, que también registre.
    async fn metered(mentor: &Mentor, content: &str) -> Result<String> {
        let (text, usage) = mentor.prompt_metered(content).await?;
        crate::token::record(Tier::Flash, &usage);
        Ok(text)
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
