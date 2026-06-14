//! El agente y su Model Router.
//!
//! dpx usa SOLO DeepSeek como cerebro (razonador, fuerte en tool-calling nativo
//! y agéntico). El Model Router se conserva como punto de extensión por si
//! algún día vuelve otro proveedor, pero hoy solo enruta a DeepSeek.

mod router;
pub mod diagnostic;
pub mod roles;
pub mod search;
pub mod tools;

pub use router::{Brain, ChatReply, Mentor, ModelRouter, is_transient_error};
