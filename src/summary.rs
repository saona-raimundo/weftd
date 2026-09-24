// src/summary.rs

use crate::chat::{Message, Role, SummaryState};
use crate::client::LlmClient;
use crate::config::{ModelConfig, Position};
use crate::render;
use crate::warn;

use tracing::debug;

/// Compute the prompt-to-context ratio from completion result token counts.
/// Returns None if token data is missing.
pub fn prompt_ratio(
    tokens_cached: Option<u32>,
    tokens_evaluated: Option<u32>,
    ctx_size: u32,
) -> Option<f64> {
    let cached = tokens_cached?;
    let evaluated = tokens_evaluated?;
    let total = cached + evaluated;
    if ctx_size == 0 {
        return None;
    }
    Some(total as f64 / ctx_size as f64)
}

/// Check whether the summary alert should fire.
pub fn should_alert(config: &ModelConfig, ratio: f64) -> bool {
    match config.summary_config.alert {
        Some(threshold) => ratio >= threshold,
        None => false,
    }
}

/// Run the summarization task. Builds its own prompt, calls the LLM (non-streaming),
/// returns the new summary text and coverage marker. Caller owns state mutation.
pub async fn run(
    config: &ModelConfig,
    current_summary: &SummaryState,
    history: &[Message],
    client: &LlmClient,
) -> anyhow::Result<SummaryState> {
    if config.summary_config.prompt.is_none() {
        anyhow::bail!("summary_prompt is required for auto-summarization")
    }

    debug!("[summarizing...]");

    let prompt = build_prompt(config, current_summary, history);
    let rendered = render::render(&config.prompt_structure, &prompt);

    let result = client.complete(&rendered, &config.sampler).await?;
    let text = result.text;
    debug!("[summary] {text}");

    Ok(SummaryState {
        text,
        up_to: history.len(),
    })
}

/// Build the summarization prompt.
fn build_prompt(config: &ModelConfig, summary: &SummaryState, history: &[Message]) -> Vec<Message> {
    let mut messages = Vec::new();

    let content = match &config.summary_config.prompt {
        Some(content) => content.clone(),
        None => {
            warn!("summary_prompt is required to build a summary prompt");
            return vec![];
        }
    };

    // Start wrappers with in_summarize = true
    for wrapper in &config.wrapper {
        if matches!(wrapper.position, Position::Start) && wrapper.in_summarize {
            messages.push(Message {
                role: Role::System,
                content: wrapper.content.trim().to_string(),
            });
        }
    }

    // Existing summary as context (so the model can build on it)
    if !summary.text.is_empty() {
        messages.push(Message {
            role: Role::System,
            content: format!("Previous summary:\n{}", summary.text),
        });
    }

    // Only messages since last summary — not full history
    let new_messages = if summary.up_to < history.len() {
        &history[summary.up_to..]
    } else {
        &[]
    };
    messages.extend_from_slice(new_messages);

    // Summary prompt
    messages.push(Message {
        role: Role::System,
        content,
    });

    // End wrappers with in_summarize = true
    for wrapper in &config.wrapper {
        if matches!(wrapper.position, Position::End) && wrapper.in_summarize {
            messages.push(Message {
                role: Role::System,
                content: wrapper.content.trim().to_string(),
            });
        }
    }

    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_ratio_computes_correctly() {
        assert_eq!(
            prompt_ratio(Some(3000), Some(1000), 8192),
            Some(4000.0 / 8192.0)
        );
    }

    #[test]
    fn prompt_ratio_none_when_missing() {
        assert_eq!(prompt_ratio(None, Some(1000), 8192), None);
        assert_eq!(prompt_ratio(Some(3000), None, 8192), None);
    }

    #[test]
    fn prompt_ratio_none_when_zero_ctx() {
        assert_eq!(prompt_ratio(Some(3000), Some(1000), 0), None);
    }
}
