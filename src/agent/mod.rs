//! El agente y su Model Router. DeepSeek como unico proveedor.

mod router;
pub mod search;
pub mod tools;

pub use router::{
    ChatReply, Mentor, ModelRouter,
    BRAIN_LABEL, BRAIN_NAME, CONTEXT_BUDGET, has_key,
    is_transient_error,
};
