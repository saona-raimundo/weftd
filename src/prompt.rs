// src/prompt.rs

use crate::chat::{Message, Role, SummaryState};
use crate::config::{ModelConfig, Position};

use std::collections::HashMap;

pub fn build(
    config: &ModelConfig,
    history: &[Message],
    summary: &SummaryState,
    task_outputs: &HashMap<String, Vec<Message>>,
    whispers: &[Message],
) -> Vec<Message> {
    let mut sections: HashMap<String, Vec<Message>> = HashMap::new();

    // Start wrappers
    let start: Vec<Message> = config
        .wrapper
        .iter()
        .filter(|w| matches!(w.position, Position::Start))
        .map(|w| Message {
            role: Role::System,
            content: w.content.trim().to_string(),
        })
        .collect();
    if !start.is_empty() {
        sections.insert("start_wrappers".to_string(), start);
    }

    // Summary
    if !summary.text.is_empty() {
        sections.insert(
            "summary".to_string(),
            vec![Message {
                role: Role::System,
                content: summary.text.to_string(),
            }],
        );
    }

    // Recent history
    let recent_start = if summary.up_to > 0 {
        std::cmp::min(
            summary.up_to,
            history.len().saturating_sub(config.recent_turns as usize),
        )
    } else {
        0
    };
    let recent = history[recent_start..].to_vec();
    if !recent.is_empty() {
        sections.insert("recent_history".to_string(), recent);
    }

    // Task outputs (retrieval pools, future observation tasks, etc.)
    for (name, messages) in task_outputs {
        if !messages.is_empty() {
            sections.insert(name.clone(), messages.clone());
        }
    }

    // End wrappers
    let end: Vec<Message> = config
        .wrapper
        .iter()
        .filter(|w| matches!(w.position, Position::End))
        .map(|w| Message {
            role: Role::System,
            content: w.content.trim().to_string(),
        })
        .collect();
    if !end.is_empty() {
        sections.insert("end_wrappers".to_string(), end);
    }

    // Whispers
    if !whispers.is_empty() {
        sections.insert("whispers".to_string(), whispers.to_vec());
    }

    // Assemble in configured order
    let mut messages = Vec::new();
    for name in &config.prompt_order {
        if let Some(section) = sections.remove(name.as_str()) {
            messages.extend(section);
        } else {
            tracing::debug!("prompt_order: section '{name}' has no content this turn");
        }
    }

    // Warn about sections produced but not in prompt_order
    for name in sections.keys() {
        tracing::warn!(
            "section '{name}' has content but is not in prompt_order — it will be dropped"
        );
    }

    messages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Message;
    use crate::config::{ModelConfig, Position, Wrapper};

    use std::collections::HashMap;

    /// Minimal config for testing — no wrappers, 2 recent turns.
    fn test_config() -> ModelConfig {
        toml::from_str(
            r#"
model = "hf://test"
threads = 1
ngl = 0
device = "none"
no_mmap = true
parallel = 1
batch_size = 512
cache_ram = 0
port = 8080
temperature = 1.0
min_p = 0.1
top_k = 40
repeat_pen = 1.1
ctx_size = 4096
max_tokens = 200
thinking = false
recent_turns = 2
"#,
        )
        .unwrap()
    }

    fn msg(role: Role, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
        }
    }

    #[test]
    fn build_with_retrieval_injects_after_history() {
        let config = test_config();
        let history = vec![
            msg(Role::User, "turn 1"),
            msg(Role::Assistant, "reply 1"),
            msg(Role::User, "turn 2"),
            msg(Role::Assistant, "reply 2"),
            msg(Role::User, "turn 3"),
        ];
        let summary = "Summary of earlier events.".to_string().into();
        let mut task_outputs = HashMap::new();
        task_outputs.insert(
            "context".to_string(),
            vec![Message {
                role: Role::System,
                content: "Elena has the map.".to_string(),
            }],
        );

        let prompt = build(&config, &history, &summary, &task_outputs, &[]);

        let summary_pos = prompt
            .iter()
            .position(|m| m.content.contains("Summary of earlier"))
            .unwrap();
        let history_pos = prompt.iter().position(|m| m.content == "reply 2").unwrap();
        let retrieval_pos = prompt
            .iter()
            .position(|m| m.content == "Elena has the map.")
            .unwrap();

        // Order: summary < history < retrieval
        assert!(
            summary_pos < history_pos,
            "history should come after summary"
        );
        assert!(
            history_pos < retrieval_pos,
            "retrieval should come after history"
        );
    }

    #[test]
    fn build_without_retrieval_has_no_extra_messages() {
        let config = test_config();
        let history = vec![
            msg(Role::User, "turn 1"),
            msg(Role::Assistant, "reply 1"),
            msg(Role::User, "turn 2"),
        ];
        let summary = "Summary.".to_string().into();

        let with_none = build(&config, &history, &summary, &HashMap::new(), &[]);

        assert!(!with_none.iter().any(|m| m.content.contains("Elena")));
        // Count: summary(1) + all 3 history messages (no cap pre-summary) = 4
        assert_eq!(with_none.len(), 4);
    }

    #[test]
    fn build_retrieval_position_with_wrappers() {
        let mut config = test_config();
        config.wrapper = vec![
            Wrapper {
                position: Position::Start,
                content: "Start wrapper.".to_string(),
                in_summarize: false,
            },
            Wrapper {
                position: Position::End,
                content: "End wrapper.".to_string(),
                in_summarize: false,
            },
        ];

        let history = vec![
            msg(Role::User, "turn 1"),
            msg(Role::Assistant, "reply 1"),
            msg(Role::User, "turn 2"),
        ];
        let summary = "Summary.".to_string().into();
        let mut task_outputs = HashMap::new();
        task_outputs.insert(
            "context".to_string(),
            vec![Message {
                role: Role::System,
                content: "Test memory.".to_string(),
            }],
        );
        let whispers = vec![msg(Role::System, "whisper text")];

        let prompt = build(&config, &history, &summary, &task_outputs, &whispers);

        let start_wrapper_pos = prompt
            .iter()
            .position(|m| m.content == "Start wrapper.")
            .unwrap();
        let summary_pos = prompt.iter().position(|m| m.content == "Summary.").unwrap();
        let retrieval_pos = prompt
            .iter()
            .position(|m| m.content == "Test memory.")
            .unwrap();

        let history_pos = prompt.iter().position(|m| m.content == "reply 1").unwrap();
        let input_pos = prompt.iter().position(|m| m.content == "turn 2").unwrap();
        let end_wrapper_pos = prompt
            .iter()
            .position(|m| m.content == "End wrapper.")
            .unwrap();
        let whisper_pos = prompt
            .iter()
            .position(|m| m.content == "whisper text")
            .unwrap();

        // Full ordering: start_wrapper < summary < history < input < retrieval < end_wrapper < whispers
        assert!(start_wrapper_pos < summary_pos);
        assert!(summary_pos < history_pos);
        assert!(history_pos < input_pos);
        assert!(input_pos < retrieval_pos);
        assert!(retrieval_pos < end_wrapper_pos);
        assert!(end_wrapper_pos < whisper_pos);
    }
    #[test]
    fn build_does_not_duplicate_last_message() {
        let config = test_config();
        let history = vec![
            msg(Role::User, "turn 1"),
            msg(Role::Assistant, "reply 1"),
            msg(Role::User, "turn 2"),
        ];

        let prompt = build(
            &config,
            &history,
            &("".to_string().into()),
            &HashMap::new(),
            &[],
        );

        let count = prompt.iter().filter(|m| m.content == "turn 2").count();
        assert_eq!(count, 1, "current input should appear exactly once");
    }

    #[test]
    fn build_with_custom_prompt_order_memories_before_history() {
        let mut config = test_config();
        config.prompt_order = vec![
            "start_wrappers".into(),
            "summary".into(),
            "context".into(),
            "recent_history".into(),
            "end_wrappers".into(),
            "whispers".into(),
        ];
        config.wrapper = vec![
            Wrapper {
                position: Position::Start,
                content: "Start.".to_string(),
                in_summarize: false,
            },
            Wrapper {
                position: Position::End,
                content: "End.".to_string(),
                in_summarize: false,
            },
        ];

        let history = vec![msg(Role::User, "turn 1"), msg(Role::Assistant, "reply 1")];
        let summary = "Summary.".to_string().into();
        let mut task_outputs = HashMap::new();
        task_outputs.insert(
            "context".to_string(),
            vec![Message {
                role: Role::System,
                content: "Memory entry.".to_string(),
            }],
        );
        let whispers = vec![msg(Role::System, "whisper")];

        let prompt = build(&config, &history, &summary, &task_outputs, &whispers);

        let names: Vec<&str> = prompt
            .iter()
            .map(|m| {
                if m.content == "Start." {
                    "start_wrappers"
                } else if m.content == "Summary." {
                    "summary"
                } else if m.content == "Memory entry." {
                    "context"
                } else if m.content == "End." {
                    "end_wrappers"
                } else if m.content == "whisper" {
                    "whispers"
                } else {
                    "history"
                }
            })
            .collect();

        // Memories should come before history
        let mem_pos = names.iter().position(|n| *n == "context").unwrap();
        let hist_pos = names.iter().position(|n| *n == "history").unwrap();
        assert!(
            mem_pos < hist_pos,
            "custom order: memories should precede history"
        );
    }

    #[test]
    fn build_with_custom_prompt_order_whispers_before_end_wrappers() {
        let mut config = test_config();
        config.prompt_order = vec![
            "start_wrappers".into(),
            "summary".into(),
            "recent_history".into(),
            "context".into(),
            "whispers".into(),
            "end_wrappers".into(),
        ];
        config.wrapper = vec![Wrapper {
            position: Position::End,
            content: "End.".to_string(),
            in_summarize: false,
        }];

        let history = vec![msg(Role::User, "turn 1")];
        let summary = "".to_string().into();
        let mut task_outputs = HashMap::new();
        task_outputs.insert(
            "context".to_string(),
            vec![Message {
                role: Role::System,
                content: "Mem.".to_string(),
            }],
        );
        let whispers = vec![msg(Role::System, "whisper")];

        let prompt = build(&config, &history, &summary, &task_outputs, &whispers);

        let whisper_pos = prompt.iter().position(|m| m.content == "whisper").unwrap();
        let end_pos = prompt.iter().position(|m| m.content == "End.").unwrap();
        assert!(
            whisper_pos < end_pos,
            "custom order: whispers should precede end_wrappers"
        );
    }

    #[test]
    fn build_empty_sections_skipped_silently() {
        let config = test_config();
        // Default order includes summary, memories, whispers — all empty this turn
        let history = vec![msg(Role::User, "hello")];
        let summary = "".to_string().into();

        let prompt = build(&config, &history, &summary, &HashMap::new(), &[]);

        // Only the history message should be present
        assert_eq!(prompt.len(), 1);
        assert_eq!(prompt[0].content, "hello");
    }

    #[test]
    fn build_section_not_in_prompt_order_is_dropped() {
        let mut config = test_config();
        // Deliberately omit "memories" from the order
        config.prompt_order = vec![
            "start_wrappers".into(),
            "summary".into(),
            "recent_history".into(),
            "end_wrappers".into(),
            "whispers".into(),
        ];

        let history = vec![msg(Role::User, "turn 1")];
        let summary = "".to_string().into();
        let mut task_outputs = HashMap::new();
        task_outputs.insert(
            "context".to_string(),
            vec![Message {
                role: Role::System,
                content: "Should be dropped.".to_string(),
            }],
        );

        let prompt = build(&config, &history, &summary, &task_outputs, &[]);

        assert!(
            !prompt
                .iter()
                .any(|m| m.content.contains("Should be dropped")),
            "memories not in prompt_order should be dropped"
        );
    }
}
