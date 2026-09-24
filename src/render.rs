// src/render.rs

use crate::chat::{Message, Role};
use crate::config::PromptStructure;

/// Renders a Vec<Message> into a raw prompt string for the LLM.
pub fn render(structure: &PromptStructure, messages: &[Message]) -> String {
    match structure {
        PromptStructure::ChatML => render_chatml(messages),
        PromptStructure::MistralV7Tekken => render_mistral_v7_tekken(messages),
    }
}

fn render_chatml(messages: &[Message]) -> String {
    let mut out = String::new();

    for msg in messages {
        if msg.content.is_empty() {
            continue;
        }

        let role = match msg.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
        };

        out.push_str("<|im_start|>");
        out.push_str(role);
        out.push('\n');
        out.push_str(&msg.content);
        out.push_str("<|im_end|>\n");
    }

    out.push_str("<|im_start|>assistant\n");
    out
}

fn render_mistral_v7_tekken(messages: &[Message]) -> String {
    // Step 1: Partition into leading systems, body, trailing systems
    let non_empty: Vec<&Message> = messages.iter().filter(|m| !m.content.is_empty()).collect();

    let mut leading_end = 0;
    for msg in &non_empty {
        if matches!(msg.role, Role::System) {
            leading_end += 1;
        } else {
            break;
        }
    }

    let mut trailing_start = non_empty.len();
    for msg in non_empty.iter().rev() {
        if matches!(msg.role, Role::System) {
            trailing_start -= 1;
        } else {
            break;
        }
    }

    // Guard: trailing_start can't overlap into leading systems
    if trailing_start < leading_end {
        trailing_start = leading_end;
    }

    let leading = &non_empty[..leading_end];
    let body = &non_empty[leading_end..trailing_start];
    let trailing = &non_empty[trailing_start..];

    let mut out = String::new();

    // Step 2: System block
    out.push_str("<s>");
    if !leading.is_empty() {
        out.push_str("[SYSTEM_PROMPT]");
        for (i, msg) in leading.iter().enumerate() {
            if i > 0 {
                out.push_str("\n\n");
            }
            out.push_str(&msg.content);
        }
        out.push_str("[/SYSTEM_PROMPT]");
    }

    // Step 3: Walk the body
    let mut ooc_buffer: Vec<&str> = Vec::new();
    let mut in_inst = false;

    for msg in body {
        match msg.role {
            Role::System => {
                ooc_buffer.push(&msg.content);
            }
            Role::User => {
                out.push_str("[INST]");
                flush_ooc(&mut out, &mut ooc_buffer);
                out.push_str(&msg.content);
                in_inst = true;
            }
            Role::Assistant => {
                if !in_inst {
                    // /system pattern: system message(s) triggered generation
                    // with no preceding user turn
                    out.push_str("[INST]");
                    flush_ooc(&mut out, &mut ooc_buffer);
                }
                out.push_str("[/INST]");
                out.push_str(&msg.content);
                out.push_str("</s>");
                in_inst = false;
            }
        }
    }

    // Step 4: Close the final turn with trailing systems
    for msg in trailing {
        ooc_buffer.push(&msg.content);
    }

    if !in_inst && (!ooc_buffer.is_empty() || body.is_empty()) {
        // Need to open an [INST] block for trailing content,
        // or for the edge case where body is empty
        out.push_str("[INST]");
        flush_ooc(&mut out, &mut ooc_buffer);
        out.push_str("[/INST]");
    } else if in_inst {
        // Normal case: user message is the last body message
        if !ooc_buffer.is_empty() {
            out.push_str("\n\n");
        }
        flush_ooc(&mut out, &mut ooc_buffer);
        out.push_str("[/INST]");
    }

    out
}

/// Flushes buffered OOC blocks into the output string, then clears the buffer.
/// Each block becomes `[OOC: {content}]`, separated by `\n\n`.
/// A `\n\n` is also emitted before the first block and after the last,
/// to separate OOC from surrounding user content.
fn flush_ooc(out: &mut String, buffer: &mut Vec<&str>) {
    if buffer.is_empty() {
        return;
    }
    for (i, content) in buffer.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        out.push_str("[OOC: ");
        out.push_str(content);
        out.push(']');
    }
    out.push_str("\n\n");
    buffer.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::Message;

    fn sys(content: &str) -> Message {
        Message {
            role: Role::System,
            content: content.to_string(),
        }
    }
    fn user(content: &str) -> Message {
        Message {
            role: Role::User,
            content: content.to_string(),
        }
    }
    fn asst(content: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: content.to_string(),
        }
    }

    // --- ChatML tests ---

    #[test]
    fn chatml_basic() {
        let messages = vec![sys("Be helpful."), user("Hello"), asst("Hi!")];
        let rendered = render(&PromptStructure::ChatML, &messages);
        assert_eq!(
            rendered,
            "<|im_start|>system\nBe helpful.<|im_end|>\n\
             <|im_start|>user\nHello<|im_end|>\n\
             <|im_start|>assistant\nHi!<|im_end|>\n\
             <|im_start|>assistant\n"
        );
    }

    #[test]
    fn chatml_skips_empty() {
        let messages = vec![sys(""), user("Hello")];
        let rendered = render(&PromptStructure::ChatML, &messages);
        assert_eq!(
            rendered,
            "<|im_start|>user\nHello<|im_end|>\n\
             <|im_start|>assistant\n"
        );
    }

    // --- Mistral V7 Tekken tests ---

    #[test]
    fn tekken_basic_conversation() {
        let messages = vec![
            sys("You are you."),
            sys("Be concise."),
            user("Hello"),
            asst("Hi there."),
            user("How are you?"),
            sys("Stay in character."),
            sys("Keep it short."),
        ];
        let rendered = render(&PromptStructure::MistralV7Tekken, &messages);
        assert_eq!(
            rendered,
            "<s>[SYSTEM_PROMPT]You are you.\n\nBe concise.[/SYSTEM_PROMPT]\
             [INST]Hello[/INST]Hi there.</s>\
             [INST]How are you?\n\n\
             [OOC: Stay in character.]\n\n\
             [OOC: Keep it short.]\n\n\
             [/INST]"
        );
    }

    #[test]
    fn tekken_mid_conversation_system() {
        let messages = vec![
            sys("Persona."),
            user("Turn 1"),
            asst("Reply 1"),
            sys("Increase tension."),
            asst("Tense reply."),
            user("Turn 2"),
        ];
        let rendered = render(&PromptStructure::MistralV7Tekken, &messages);
        assert_eq!(
            rendered,
            "<s>[SYSTEM_PROMPT]Persona.[/SYSTEM_PROMPT]\
             [INST]Turn 1[/INST]Reply 1</s>\
             [INST][OOC: Increase tension.]\n\n\
             [/INST]Tense reply.</s>\
             [INST]Turn 2[/INST]"
        );
    }

    #[test]
    fn tekken_leading_systems_only() {
        let messages = vec![sys("System 1"), sys("System 2")];
        let rendered = render(&PromptStructure::MistralV7Tekken, &messages);
        assert_eq!(
            rendered,
            "<s>[SYSTEM_PROMPT]System 1\n\nSystem 2[/SYSTEM_PROMPT]\
             [INST][/INST]"
        );
    }

    #[test]
    fn tekken_no_leading_systems() {
        let messages = vec![user("Hello"), asst("Hi"), user("Next")];
        let rendered = render(&PromptStructure::MistralV7Tekken, &messages);
        assert_eq!(rendered, "<s>[INST]Hello[/INST]Hi</s>[INST]Next[/INST]");
    }

    #[test]
    fn tekken_no_trailing_systems() {
        let messages = vec![sys("Persona."), user("Hello")];
        let rendered = render(&PromptStructure::MistralV7Tekken, &messages);
        assert_eq!(
            rendered,
            "<s>[SYSTEM_PROMPT]Persona.[/SYSTEM_PROMPT]\
             [INST]Hello[/INST]"
        );
    }

    #[test]
    fn tekken_skips_empty() {
        let messages = vec![sys("Persona."), sys(""), user("Hello"), sys("End.")];
        let rendered = render(&PromptStructure::MistralV7Tekken, &messages);
        assert_eq!(
            rendered,
            "<s>[SYSTEM_PROMPT]Persona.[/SYSTEM_PROMPT]\
             [INST]Hello\n\n\
             [OOC: End.]\n\n\
             [/INST]"
        );
    }

    #[test]
    fn tekken_multiple_mid_conversation_systems() {
        let messages = vec![
            sys("Persona."),
            user("Turn 1"),
            asst("Reply 1"),
            sys("Directive A."),
            sys("Directive B."),
            user("Turn 2"),
            asst("Reply 2"),
            user("Turn 3"),
        ];
        let rendered = render(&PromptStructure::MistralV7Tekken, &messages);
        assert_eq!(
            rendered,
            "<s>[SYSTEM_PROMPT]Persona.[/SYSTEM_PROMPT]\
             [INST]Turn 1[/INST]Reply 1</s>\
             [INST][OOC: Directive A.]\n\n\
             [OOC: Directive B.]\n\n\
             Turn 2[/INST]Reply 2</s>\
             [INST]Turn 3[/INST]"
        );
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use crate::config::{ModelConfig, Position, Wrapper};
    use crate::prompt;

    use std::collections::HashMap;

    fn test_config(structure: PromptStructure) -> ModelConfig {
        let toml_str = r#"
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
recent_turns = 10
"#;
        let mut config: ModelConfig = toml::from_str(toml_str).unwrap();
        config.prompt_structure = structure;
        config.wrapper = vec![
            Wrapper {
                position: Position::Start,
                content: "You are you.".to_string(),
                in_summarize: false,
            },
            Wrapper {
                position: Position::Start,
                content: "Write in third person only.".to_string(),
                in_summarize: false,
            },
            Wrapper {
                position: Position::End,
                content: "Stay in character at all times.".to_string(),
                in_summarize: false,
            },
            Wrapper {
                position: Position::End,
                content: "Hard limit: 120 words.".to_string(),
                in_summarize: false,
            },
        ];
        config
    }

    fn sys(content: &str) -> Message {
        Message {
            role: Role::System,
            content: content.to_string(),
        }
    }
    fn user(content: &str) -> Message {
        Message {
            role: Role::User,
            content: content.to_string(),
        }
    }
    fn asst(content: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: content.to_string(),
        }
    }

    /// Full pipeline: build() → render() for Mistral V7 Tekken.
    /// Asserts structural properties of the rendered prompt.
    #[test]
    fn tekken_full_pipeline() {
        let config = test_config(PromptStructure::MistralV7Tekken);

        let history = vec![
            user("I walk into the kitchen."),
            asst("She looks up from her phone."),
            sys("Increase tension in the next response."),
            asst("She sets the phone down slowly."),
            user("Hey, you okay?"),
            asst("She doesn't answer right away."),
            user("I sit down next to her."),
        ];

        let summary = "Earlier: they argued about plans.".to_string().into();
        let mut task_outputs = HashMap::new();
        task_outputs.insert(
            "context".to_string(),
            vec![Message {
                role: Role::System,
                content: "I hate being asked if I am okay.".to_string(),
            }],
        );
        let whispers = vec![sys("Make yourself quieter than usual.")];

        let messages = prompt::build(&config, &history, &summary, &task_outputs, &whispers);
        let rendered = render(&PromptStructure::MistralV7Tekken, &messages);

        // --- Structural assertions ---

        let sys_block_end = rendered.find("[/SYSTEM_PROMPT]").unwrap();
        let sys_block = &rendered[..sys_block_end];
        let after_sys_block = &rendered[sys_block_end..];
        let end_wrapper_pos = rendered.find("[OOC: Hard limit: 120 words.]").unwrap();
        let whisper_pos = rendered
            .find("[OOC: Make yourself quieter than usual.]")
            .unwrap();
        let last_inst = rendered.rfind("[INST]").unwrap();
        let input_pos = rendered.rfind("I sit down next to her.").unwrap();
        let memory_pos = rendered.find("I hate being asked").unwrap();

        // Starts with BOS + system block
        assert!(
            rendered.starts_with("<s>[SYSTEM_PROMPT]"),
            "must start with <s>[SYSTEM_PROMPT]"
        );

        // Ends with [/INST] (ready for generation)
        assert!(rendered.ends_with("[/INST]"), "must end with [/INST]");

        // Exactly one system block
        assert_eq!(
            rendered.matches("[SYSTEM_PROMPT]").count(),
            1,
            "exactly one [SYSTEM_PROMPT]"
        );
        assert_eq!(
            rendered.matches("[/SYSTEM_PROMPT]").count(),
            1,
            "exactly one [/SYSTEM_PROMPT]"
        );

        // System block contains start wrappers, summary, and memories
        assert!(
            sys_block.contains("You are you"),
            "start wrapper 1 in system block"
        );
        assert!(
            sys_block.contains("Write in third person"),
            "start wrapper 2 in system block"
        );
        assert!(
            sys_block.contains("they argued about plans"),
            "summary in system block"
        );
        // Memories must NOT be in the system block — they go after history as OOC
        assert!(
            !sys_block.contains("I hate being asked"),
            "memories must not be in system block"
        );
        assert!(
            after_sys_block.contains("I hate being asked"),
            "memories as OOC after history"
        );

        // Start wrappers, summary, memories must NOT appear outside the system block
        assert!(
            !after_sys_block.contains("You are you"),
            "start wrapper must not leak outside system block"
        );
        assert!(
            !after_sys_block.contains("they argued about plans"),
            "summary must not leak outside system block"
        );

        // End wrappers appear as OOC, after the system block
        assert!(
            after_sys_block.contains("[OOC: Stay in character at all times.]"),
            "end wrapper 1 as OOC"
        );
        assert!(
            after_sys_block.contains("[OOC: Hard limit: 120 words.]"),
            "end wrapper 2 as OOC"
        );

        // Whispers appear as OOC, after end wrappers
        assert!(whisper_pos > end_wrapper_pos, "whispers after end wrappers");

        // End wrappers and whispers are inside the final [INST] block
        assert!(
            end_wrapper_pos > last_inst,
            "end wrappers inside final [INST]"
        );
        assert!(whisper_pos > last_inst, "whispers inside final [INST]");

        // Current input is inside the final [INST] block
        assert!(input_pos > last_inst, "current input inside final [INST]");
        assert!(
            input_pos < end_wrapper_pos,
            "current input before OOC blocks"
        );

        // Memories between input and end wrappers
        assert!(memory_pos > input_pos, "memories after input");
        assert!(memory_pos < end_wrapper_pos, "memories before end wrappers");

        // Mid-conversation /system message rendered as OOC
        assert!(
            after_sys_block.contains("[OOC: Increase tension"),
            "/system as OOC in conversation body"
        );

        // No whitespace between control tokens and content
        assert!(!rendered.contains("[INST] "), "no space after [INST]");
        assert!(!rendered.contains("[/INST] "), "no space after [/INST]");
        assert!(
            !rendered.contains("[SYSTEM_PROMPT] "),
            "no space after [SYSTEM_PROMPT]"
        );
        assert!(
            !rendered.contains(" [/SYSTEM_PROMPT]"),
            "no space before [/SYSTEM_PROMPT]"
        );

        // Exactly one <s>, no duplicates
        assert_eq!(rendered.matches("<s>").count(), 1, "exactly one BOS");

        // </s> appears after each completed assistant turn (3 in history)
        assert_eq!(
            rendered.matches("</s>").count(),
            3,
            "EOS after each completed assistant turn"
        );
    }

    /// Full pipeline: build() → render() for ChatML.
    /// Asserts structural properties of the rendered prompt.
    #[test]
    fn chatml_full_pipeline() {
        let config = test_config(PromptStructure::ChatML);

        let history = vec![
            user("I walk into the kitchen."),
            asst("She looks up from her phone."),
            user("Hey, you okay?"),
            asst("She doesn't answer right away."),
            user("I sit down next to her."),
        ];

        let summary = "Earlier: they argued about plans.".to_string().into();
        let mut task_outputs = HashMap::new();
        task_outputs.insert(
            "context".to_string(),
            vec![Message {
                role: Role::System,
                content: "I hate being asked if I am okay.".to_string(),
            }],
        );
        let whispers = vec![sys("Make yourself quieter than usual.")];

        let messages = prompt::build(&config, &history, &summary, &task_outputs, &whispers);
        let rendered = render(&PromptStructure::ChatML, &messages);

        // Ends with assistant prompt
        assert!(
            rendered.ends_with("<|im_start|>assistant\n"),
            "must end with assistant prompt"
        );

        // Every message wrapped in im_start/im_end
        let start_count = rendered.matches("<|im_start|>").count();
        let end_count = rendered.matches("<|im_end|>").count();
        // end_count = start_count - 1 because the final assistant prompt has no im_end
        assert_eq!(
            end_count,
            start_count - 1,
            "im_end for every complete message"
        );

        // System messages appear with system role tag
        assert!(
            rendered.contains("<|im_start|>system\nYou are you"),
            "start wrapper as system message"
        );
        assert!(
            rendered.contains("<|im_start|>system\nStay in character"),
            "end wrapper as system message"
        );
        assert!(
            rendered.contains("<|im_start|>system\nMake yourself quieter"),
            "whisper as system message"
        );

        // Ordering: start wrappers before summary before memories before history
        let wrapper_pos = rendered.find("You are you").unwrap();
        let summary_pos = rendered.find("they argued about plans").unwrap();
        let memory_pos = rendered.find("I hate being asked").unwrap();
        let history_pos = rendered.find("I walk into the kitchen").unwrap();
        let input_pos = rendered.find("I sit down next to her").unwrap();
        let end_wrapper_pos = rendered.find("Stay in character").unwrap();
        let whisper_pos = rendered.find("Make yourself quieter").unwrap();

        assert!(wrapper_pos < summary_pos, "wrappers before summary");
        assert!(summary_pos < history_pos, "summary before history");
        assert!(history_pos < input_pos, "history before input");
        assert!(input_pos < memory_pos, "memories after input");
        assert!(memory_pos < end_wrapper_pos, "memories before end wrappers");
        assert!(
            end_wrapper_pos < whisper_pos,
            "end wrappers before whispers"
        );
    }
}
