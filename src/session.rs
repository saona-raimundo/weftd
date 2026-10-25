// src/session.rs

use crate::chat::{Conversation, SummaryState};
use crate::config::{LiveSettings, ModelConfig};
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
    live: &LiveSettings,
) -> ModelConfig {
    let mut snap = config.clone();
    snap.message = conversation.messages().to_vec();
    snap.summary = summary.clone();
    snap.whisper = whispers.active().to_vec();
    snap.wrapper = live.wrappers.clone();
    snap.sampler = live.sampler.clone();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Position, Wrapper};

    #[test]
    fn snapshot_writes_live_settings_and_round_trips() {
        let config = ModelConfig {
            wrapper: vec![
                Wrapper {
                    position: Position::Start,
                    content: "Persona.".to_string(),
                    in_summarize: false,
                },
                Wrapper {
                    position: Position::End,
                    content: "Stale.".to_string(),
                    in_summarize: true,
                },
            ],
            ..Default::default()
        };
        let mut live = LiveSettings::from_config(&config);
        live.wrappers[1].content = "Edited.".to_string();
        live.sampler.temperature = Some(0.9);
        live.sampler.stop = Some(vec!["\n\n".to_string(), "User:".to_string()]);

        let snap = snapshot(
            &config,
            &Conversation::new(),
            &SummaryState::new(),
            &WhisperStack::default(),
            &live,
        );
        let reloaded: ModelConfig =
            toml::from_str(&toml::to_string_pretty(&snap).unwrap()).unwrap();

        assert_eq!(
            config.wrapper[1].content, "Stale.",
            "source config untouched"
        );
        assert_eq!(reloaded.wrapper.len(), 2);
        assert!(matches!(reloaded.wrapper[0].position, Position::Start));
        assert_eq!(reloaded.wrapper[0].content, "Persona.");
        assert!(matches!(reloaded.wrapper[1].position, Position::End));
        assert_eq!(reloaded.wrapper[1].content, "Edited.");
        assert!(reloaded.wrapper[1].in_summarize);
        assert_eq!(config.sampler.temperature, None, "source config untouched");
        assert_eq!(reloaded.sampler.temperature, Some(0.9));
        assert_eq!(
            reloaded.sampler.stop,
            Some(vec!["\n\n".to_string(), "User:".to_string()])
        );
    }
}
