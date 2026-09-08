//! El agente y su Model Router. DeepSeek como unico proveedor.

mod router;
pub mod search;
pub mod tools;

pub use router::{
    ChatReply, Effort, Mentor, ModelRouter,
    BRAIN_LABEL, BRAIN_NAME, CONTEXT_BUDGET, CONTEXT_WINDOW, has_key,
    is_transient_error, model_id, brain_tier,
};
