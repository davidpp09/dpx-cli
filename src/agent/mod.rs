//! El agente y su Model Router.
//!
//! El router inicializa los clientes de `rig-core` disponibles y reparte el
//! trabajo según el fuerte de cada modelo. DeepSeek es el cerebro principal;
//! Kimi (Moonshot) y Qwen (vía OpenRouter) son los apoyos del plan
//! anti-suscripción — los tres fuertes en tool-calling nativo.

mod router;
pub mod diagnostic;
pub mod search;
pub mod tools;

pub use router::{Brain, ChatReply, Mentor, ModelRouter, is_transient_error};
