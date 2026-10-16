//! The translation stage: turns cues into deduplicated units, sends them to the
//! model in batches, several at once, and rebuilds timed cues from the answers.

mod assemble;
mod batching;
mod prompt;
mod response;
mod translator;
mod units;

pub use assemble::{assemble_translated_cues, AssemblyReport};
pub use batching::{make_batches, Batch};
pub use prompt::{PromptContext, batch_message, system_prompt, ContinuityLine};
pub use response::parse_numbered_lines;
pub use translator::{TranslationOutcome, Translator, TranslationProgress};
pub use units::{build_units, TranslationUnit};
