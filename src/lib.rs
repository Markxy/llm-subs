//! LLM Subs: watches media folders and translates subtitles with an LLM.

pub mod app;
pub mod cli;
pub mod config;
pub mod ffmpeg;
pub mod jobs;
pub mod llm;
pub mod logging;
pub mod media;
pub mod paths;
pub mod platform;
pub mod subtitles;
pub mod translation;
pub mod watch;
pub mod ui;
