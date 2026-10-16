//! Configuration schema. Everything the UI can change is also editable in
//! `config.toml`; every field has a default so a partial file is valid.

mod languages;
mod schema;
pub mod store;

pub use languages::{
    Language, all_languages, find_language, find_language_by_name, language_name, same_language, system_language_code,
};
pub use schema::*;
pub use store::ConfigStore;
