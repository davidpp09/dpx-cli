//! El agente y su Model Router.
//!
//! El router inicializa los clientes de `rig-core` disponibles y reparte el
//! trabajo según el fuerte de cada modelo. DeepSeek es el cerebro mentor; el
//! resto entrará como apoyo (Groq = rápido, Mistral = structured, Gemini =
//! contexto largo) a medida que el CLI lo necesite.

mod router;
pub mod diagnostic;
pub mod tools;

pub use router::{Brain, ChatReply, Mentor, ModelRouter};
