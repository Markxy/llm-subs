//! Provider-agnostic chat model access. The pipeline only ever sees [`ChatModel`];
//! the concrete HTTP shapes live in `openai.rs`, `anthropic.rs` and `chatgpt/`.

mod anthropic;
pub mod chatgpt;
mod http;
mod ledger;
mod openai;
mod pricing;
mod provider;
mod retry;
mod sse;
mod types;

pub use ledger::{ModelUsage, UsageLedger, UsageSummary};
pub use pricing::{builtin_price, cost_for, format_usd, price_for, Cost};
pub use provider::{build_model, test_connection, LlmServices, ModelHandle};
pub use retry::complete_with_retries;
pub use types::*;

/// Selects the TLS crypto backend for every HTTPS client. Safe to call repeatedly.
pub fn install_crypto_provider() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}
