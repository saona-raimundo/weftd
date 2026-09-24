// src/session.rs

use crate::chat::{Conversation, SummaryState};
use crate::config::ModelConfig;
use crate::utils;
use crate::whisper::WhisperStack;

use std::fs;
use std::path::PathBuf;

/// Build a saveable snapshot by overwriting runtime state fields on the config.
pub fn snapshot(
    config: &ModelConfig,
    conversation: &Conversation,
    summary: &SummaryState,
    whispers: &WhisperStack,
) -> ModelConfig {
    let mut snap = config.clone();
    snap.message = conversation.messages().to_vec();
    snap.summary = summary.clone();
    snap.whisper = whispers.active().to_vec();
    snap
}

pub fn save(config: &ModelConfig, path: &PathBuf) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(config)?;
    fs::write(path, content)?;
    Ok(())
}

pub fn save_path() -> PathBuf {
    let filename = format!("sessions/{}.toml", utils::file_timestamp());
    PathBuf::from(filename)
}
