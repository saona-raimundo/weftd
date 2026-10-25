// src/execute.rs

use crate::chat::{Conversation, Message, Role};
use crate::command::Command;
use crate::config::{LiveSettings, Position, SamplerConfig, Wrapper};
use crate::whisper::WhisperStack;

use serde::de::DeserializeOwned;
use std::collections::HashSet;
use std::fmt::Display;
use tracing::debug;

/// What the caller (Connection) should do after command execution.
pub enum Effect {
    Sync,
    Generate,
    WhispersChanged,
    Error(String),
    Multi(Vec<Effect>),
}

pub fn dispatch(
    cmd: Command,
    conv: &mut Conversation,
    whispers: &mut WhisperStack,
    live: &mut LiveSettings,
) -> Effect {
    match cmd {
        Command::Delete { index } => execute_delete(conv, index),
        Command::Retry { text } => execute_retry(conv, whispers, text),
        Command::Whisper { turns, text } => execute_whisper(whispers, text, turns),
        Command::System { text } => execute_system(conv, text),
        Command::User { text } => execute_user(conv, text),
        Command::Assistant { text } => execute_assistant(conv, text),
        Command::Cancel { index } => execute_cancel(whispers, index),
        Command::Continue { text } => execute_continue(whispers, text),
        Command::Sync => execute_sync(),
        Command::EndWrapperEdit { index, text } => {
            execute_end_wrapper_edit(&mut live.wrappers, index, text)
        }
        Command::EndWrapperAdd { index, text } => {
            execute_end_wrapper_add(&mut live.wrappers, index, text)
        }
        Command::EndWrapperRemove { index } => {
            execute_end_wrapper_remove(&mut live.wrappers, index)
        }
        Command::SamplerSet { key, value } => execute_sampler(&mut live.sampler, key, Some(value)),
        Command::SamplerUnset { key } => execute_sampler(&mut live.sampler, key, None),
    }
}

fn execute_delete(conv: &mut Conversation, index: Option<usize>) -> Effect {
    let target = index.unwrap_or_else(|| conv.messages().len().saturating_sub(1));
    match conv.remove(target) {
        Some(removed) => {
            debug!(
                "[delete {target}] removed {}: {}",
                removed.role, removed.content
            );
            Effect::Sync
        }
        None => Effect::Error("Nothing to delete".into()),
    }
}

fn execute_retry(
    conv: &mut Conversation,
    whispers: &mut WhisperStack,
    text: Option<String>,
) -> Effect {
    let is_assistant = matches!(
        conv.messages().last(),
        Some(msg) if msg.role == Role::Assistant
    );
    if !is_assistant {
        return Effect::Error("Nothing to retry — last message isn't an assistant response".into());
    }

    conv.pop();

    let trigger = conv.messages().last().cloned();
    match trigger {
        Some(trigger_msg) => {
            debug!("[retry] regenerating from: {}", trigger_msg.content);
            let mut effects = vec![Effect::Sync];
            if let Some(whisper_text) = text {
                debug!("[whisper +1] {whisper_text}");
                whispers.add(whisper_text, 1);
                effects.push(Effect::WhispersChanged);
            }
            effects.push(Effect::Generate);
            Effect::Multi(effects)
        }
        None => Effect::Multi(vec![
            Effect::Sync,
            Effect::Error("No message to regenerate from".into()),
        ]),
    }
}

fn execute_whisper(whispers: &mut WhisperStack, text: String, turns: u32) -> Effect {
    debug!("[whisper +{turns}] {text}");
    whispers.add(text, turns);
    Effect::WhispersChanged
}

fn execute_system(conv: &mut Conversation, text: String) -> Effect {
    debug!("[system] {text}");
    conv.push(Message {
        role: Role::System,
        content: text,
    });
    Effect::Sync
}

fn execute_user(conv: &mut Conversation, text: String) -> Effect {
    debug!("[user] {text}");
    conv.push(Message {
        role: Role::User,
        content: text,
    });
    Effect::Sync
}

fn execute_assistant(conv: &mut Conversation, text: String) -> Effect {
    debug!("[assistant] {text}");
    conv.push(Message {
        role: Role::Assistant,
        content: text,
    });
    Effect::Sync
}

fn execute_cancel(whispers: &mut WhisperStack, index: usize) -> Effect {
    match whispers.cancel(index) {
        Some(removed) => {
            debug!("[cancel] removed whisper: {}", removed.text);
            Effect::WhispersChanged
        }
        None => Effect::Error(format!("No whisper at index {index}")),
    }
}

fn execute_continue(whispers: &mut WhisperStack, text: Option<String>) -> Effect {
    debug!("[continue]");
    let mut effects = vec![];
    if let Some(whisper_text) = text {
        debug!("[whisper +1] {whisper_text}");
        whispers.add(whisper_text, 1);
        effects.push(Effect::WhispersChanged);
    }
    effects.push(Effect::Generate);
    Effect::Multi(effects)
}

fn execute_sync() -> Effect {
    Effect::Multi(vec![Effect::Sync, Effect::WhispersChanged])
}

/// `index` counts end wrappers only; start wrappers are unreachable by design
/// (editing them would invalidate the cached KV prefix).
fn execute_end_wrapper_edit(wrappers: &mut [Wrapper], index: usize, text: String) -> Effect {
    match wrappers
        .iter_mut()
        .filter(|w| matches!(w.position, Position::End))
        .nth(index)
    {
        Some(wrapper) => {
            debug!("[wrapper end edit {index}] {text}");
            wrapper.content = text;
            Effect::Sync
        }
        None => Effect::Error(format!("No end wrapper at index {index}")),
    }
}

/// Positions in `wrappers` that hold end wrappers, in order.
fn end_slots(wrappers: &[Wrapper]) -> Vec<usize> {
    wrappers
        .iter()
        .enumerate()
        .filter(|(_, w)| matches!(w.position, Position::End))
        .map(|(slot, _)| slot)
        .collect()
}

/// Inserts before the `index`-th end wrapper; `index == count` appends after
/// the last one (or at the end of the list if there are none).
fn execute_end_wrapper_add(wrappers: &mut Vec<Wrapper>, index: usize, text: String) -> Effect {
    let slots = end_slots(wrappers);
    if index > slots.len() {
        return Effect::Error(format!(
            "Can't add an end wrapper at index {index}: there are {}",
            slots.len()
        ));
    }
    let slot = match slots.get(index) {
        Some(&slot) => slot,
        None => slots.last().map_or(wrappers.len(), |&last| last + 1),
    };
    debug!("[wrapper end add {index}] {text}");
    wrappers.insert(
        slot,
        Wrapper {
            position: Position::End,
            content: text,
            in_summarize: false,
        },
    );
    Effect::Sync
}

/// Sets (`Some(json)`) or clears (`None`) one sampler field.
fn execute_sampler(sampler: &mut SamplerConfig, key: String, value: Option<String>) -> Effect {
    match update_sampler_field(sampler, &key, value.as_deref()) {
        Ok(()) => {
            debug!(
                "[sampler] {key} = {}",
                value.as_deref().unwrap_or("(default)")
            );
            Effect::Sync
        }
        // Error first so a typed command stays in the input (the error marks the
        // submit failed); the sync then resets the panel to the real value.
        Err(e) => Effect::Multi(vec![Effect::Error(format!("{key}: {e}")), Effect::Sync]),
    }
}

/// Validates before assigning, so on `Err` the sampler is unchanged. Ranges
/// reject what llama-server would reject (HTTP 400) or silently clamp, so the
/// stored value is the value in effect (except unknown `samplers` names).
fn update_sampler_field(
    s: &mut SamplerConfig,
    key: &str,
    value: Option<&str>,
) -> Result<(), String> {
    match key {
        "temperature" => s.temperature = field(value, |v| at_least(v, 0.0))?,
        "min_p" => s.min_p = field(value, |v| between(v, 0.0, 1.0))?,
        "top_k" => s.top_k = field(value, |v| at_least(v, 0))?,
        "top_p" => s.top_p = field(value, |v| between(v, 0.0, 1.0))?,
        "max_tokens" => s.max_tokens = field(value, |v| at_least(v, -1))?,
        "repeat_penalty" => s.repeat_penalty = field(value, Ok)?,
        "repeat_last_n" => s.repeat_last_n = field(value, |v| at_least(v, 0))?,
        "presence_penalty" => s.presence_penalty = field(value, Ok)?,
        "frequency_penalty" => s.frequency_penalty = field(value, Ok)?,
        "dry_multiplier" => s.dry_multiplier = field(value, Ok)?,
        "dry_base" => s.dry_base = field(value, |v| at_least(v, 1.0))?,
        "dry_allowed_length" => s.dry_allowed_length = field(value, |v| at_least(v, 0))?,
        "dry_penalty_last_n" => s.dry_penalty_last_n = field(value, |v| at_least(v, 0))?,
        "dry_sequence_breakers" => s.dry_sequence_breakers = field(value, sequence_breakers)?,
        "xtc_probability" => s.xtc_probability = field(value, |v| between(v, 0.0, 1.0))?,
        "xtc_threshold" => s.xtc_threshold = field(value, |v| between(v, 0.0, 1.0))?,
        // Names pass through unchecked; llama-server drops unknown ones.
        "samplers" => s.samplers = field(value, non_empty_strings)?,
        "seed" => s.seed = field(value, |v| between(v, -1, i64::from(u32::MAX)))?,
        "stop" => s.stop = field(value, non_empty_strings)?,
        _ if SamplerConfig::READ_ONLY.contains(&key) => {
            return Err("read-only here; change it in the TOML".into());
        }
        _ => return Err("unknown sampler field".into()),
    }
    Ok(())
}

/// Parses and checks `value`, or yields `None` when unsetting.
fn field<T: DeserializeOwned>(
    value: Option<&str>,
    check: impl Fn(T) -> Result<T, String>,
) -> Result<Option<T>, String> {
    value
        .map(|raw| {
            if raw.trim() == "null" {
                return Err("use unset to clear a value".to_string());
            }
            serde_json::from_str(raw)
                .map_err(|e| e.to_string())
                .and_then(&check)
        })
        .transpose()
}

fn at_least<T: PartialOrd + Display>(v: T, min: T) -> Result<T, String> {
    if v >= min {
        Ok(v)
    } else {
        Err(format!("must be at least {min}"))
    }
}

fn between<T: PartialOrd + Display>(v: T, min: T, max: T) -> Result<T, String> {
    if min <= v && v <= max {
        Ok(v)
    } else {
        Err(format!("must be between {min} and {max}"))
    }
}

/// Lists are cleared with unset, so an empty list is a mistake.
fn non_empty_strings(list: Vec<String>) -> Result<Vec<String>, String> {
    if list.is_empty() {
        return Err("must not be empty; use unset to clear it".into());
    }
    if list.iter().any(String::is_empty) {
        return Err("entries must not be empty".into());
    }
    Ok(list)
}

/// llama-server rejects more than 64 distinct breakers.
fn sequence_breakers(list: Vec<String>) -> Result<Vec<String>, String> {
    let list = non_empty_strings(list)?;
    if list.iter().collect::<HashSet<_>>().len() > 64 {
        return Err("at most 64 distinct entries".into());
    }
    Ok(list)
}

fn execute_end_wrapper_remove(wrappers: &mut Vec<Wrapper>, index: usize) -> Effect {
    match end_slots(wrappers).get(index) {
        Some(&slot) => {
            let removed = wrappers.remove(slot);
            debug!("[wrapper end remove {index}] {}", removed.content);
            Effect::Sync
        }
        None => Effect::Error(format!("No end wrapper at index {index}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::{Conversation, Message, Role};
    use crate::config::{LiveSettings, Position, Wrapper};
    use crate::whisper::WhisperStack;

    fn msg(role: Role, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
        }
    }

    fn wrapper(position: Position, content: &str, in_summarize: bool) -> Wrapper {
        Wrapper {
            position,
            content: content.to_string(),
            in_summarize,
        }
    }

    /// Interleaved start/end wrappers: end index 0 → slot 1, end index 1 → slot 3.
    fn mixed_wrappers() -> Vec<Wrapper> {
        vec![
            wrapper(Position::Start, "start a", false),
            wrapper(Position::End, "end a", false),
            wrapper(Position::Start, "start b", false),
            wrapper(Position::End, "end b", true),
        ]
    }

    #[test]
    fn delete_last_by_default() {
        let mut conv = Conversation::new();
        conv.push(msg(Role::User, "hello"));
        conv.push(msg(Role::Assistant, "hi"));
        let effect = execute_delete(&mut conv, None);
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(conv.messages().len(), 1);
        assert_eq!(conv.messages()[0].content, "hello");
    }

    #[test]
    fn delete_by_index() {
        let mut conv = Conversation::new();
        conv.push(msg(Role::User, "a"));
        conv.push(msg(Role::Assistant, "b"));
        conv.push(msg(Role::User, "c"));
        let effect = execute_delete(&mut conv, Some(1));
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(conv.messages().len(), 2);
        assert_eq!(conv.messages()[1].content, "c");
    }

    #[test]
    fn delete_empty_is_error() {
        let mut conv = Conversation::new();
        let effect = execute_delete(&mut conv, None);
        assert!(matches!(effect, Effect::Error(_)));
    }

    #[test]
    fn retry_pops_only_assistant() {
        let mut conv = Conversation::new();
        conv.push(msg(Role::User, "hello"));
        conv.push(msg(Role::Assistant, "bad response"));
        let mut whispers = WhisperStack::default();
        let effect = execute_retry(&mut conv, &mut whispers, None);
        assert!(matches!(effect, Effect::Multi(_)));
        assert_eq!(conv.messages().len(), 1);
        assert_eq!(conv.messages()[0].content, "hello");
    }

    #[test]
    fn retry_fails_if_last_not_assistant() {
        let mut conv = Conversation::new();
        conv.push(msg(Role::User, "hello"));
        let mut whispers = WhisperStack::default();
        let effect = execute_retry(&mut conv, &mut whispers, None);
        assert!(matches!(effect, Effect::Error(_)));
        assert_eq!(conv.messages().len(), 1); // unchanged
    }

    #[test]
    fn retry_adds_whisper() {
        let mut conv = Conversation::new();
        conv.push(msg(Role::User, "hello"));
        conv.push(msg(Role::Assistant, "bad"));
        let mut whispers = WhisperStack::default();
        let effect = execute_retry(&mut conv, &mut whispers, Some("fix tone".into()));
        assert!(matches!(effect, Effect::Multi(_)));
        assert_eq!(whispers.active().len(), 1);
        assert_eq!(whispers.active()[0].text, "fix tone");
    }

    #[test]
    fn system_pushes_and_syncs() {
        let mut conv = Conversation::new();
        let effect = execute_system(&mut conv, "describe the sunset".into());
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(conv.messages().len(), 1);
        assert_eq!(conv.messages()[0].role, Role::System);
        assert_eq!(conv.messages()[0].content, "describe the sunset");
    }

    #[test]
    fn retry_syncs_before_error_when_no_trigger() {
        let mut conv = Conversation::new();
        conv.push(msg(Role::Assistant, "lonely response"));
        let mut whispers = WhisperStack::default();
        let effect = execute_retry(&mut conv, &mut whispers, None);
        assert_eq!(conv.messages().len(), 0); // assistant was popped
        match effect {
            Effect::Multi(effects) => {
                assert!(matches!(effects[0], Effect::Sync));
                assert!(matches!(effects[1], Effect::Error(_)));
            }
            _ => panic!("expected Multi with Sync + Error"),
        }
    }

    // --- execute_retry edge cases ---

    #[test]
    fn retry_after_system_uses_system_as_trigger() {
        let mut conv = Conversation::new();
        conv.push(msg(Role::System, "describe the forest"));
        conv.push(msg(Role::Assistant, "bad response"));
        let mut whispers = WhisperStack::default();
        let effect = execute_retry(&mut conv, &mut whispers, None);
        assert_eq!(conv.messages().len(), 1);
        assert_eq!(conv.messages()[0].role, Role::System);
        match effect {
            Effect::Multi(effects) => {
                assert!(matches!(effects[0], Effect::Sync));
                assert!(matches!(effects[1], Effect::Generate));
            }
            _ => panic!("expected Multi"),
        }
    }

    #[test]
    fn retry_empty_conversation_is_error() {
        let mut conv = Conversation::new();
        let mut whispers = WhisperStack::default();
        let effect = execute_retry(&mut conv, &mut whispers, None);
        assert!(matches!(effect, Effect::Error(_)));
        assert_eq!(conv.messages().len(), 0);
    }

    #[test]
    fn retry_no_trigger_does_not_add_whisper() {
        let mut conv = Conversation::new();
        conv.push(msg(Role::Assistant, "lonely"));
        let mut whispers = WhisperStack::default();
        let effect = execute_retry(&mut conv, &mut whispers, Some("steer this".into()));
        assert!(whispers.active().is_empty());
        match effect {
            Effect::Multi(effects) => {
                assert!(matches!(effects[0], Effect::Sync));
                assert!(matches!(effects[1], Effect::Error(_)));
            }
            _ => panic!("expected Multi with Sync + Error"),
        }
    }

    // --- execute_delete edge cases ---

    #[test]
    fn delete_first_shifts_remaining() {
        let mut conv = Conversation::new();
        conv.push(msg(Role::User, "a"));
        conv.push(msg(Role::Assistant, "b"));
        conv.push(msg(Role::User, "c"));
        let effect = execute_delete(&mut conv, Some(0));
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(conv.messages().len(), 2);
        assert_eq!(conv.messages()[0].content, "b");
        assert_eq!(conv.messages()[1].content, "c");
    }

    #[test]
    fn delete_out_of_bounds_is_error() {
        let mut conv = Conversation::new();
        conv.push(msg(Role::User, "a"));
        let effect = execute_delete(&mut conv, Some(5));
        assert!(matches!(effect, Effect::Error(_)));
        assert_eq!(conv.messages().len(), 1);
    }

    #[test]
    fn delete_system_message() {
        let mut conv = Conversation::new();
        conv.push(msg(Role::User, "hello"));
        conv.push(msg(Role::System, "describe the sunset"));
        conv.push(msg(Role::Assistant, "the sun set"));
        let effect = execute_delete(&mut conv, Some(1));
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(conv.messages().len(), 2);
        assert_eq!(conv.messages()[0].content, "hello");
        assert_eq!(conv.messages()[1].content, "the sun set");
    }

    // --- dispatch sequences ---

    #[test]
    fn delete_assistant_then_retry_syncs_and_errors() {
        let mut conv = Conversation::new();
        let mut whispers = WhisperStack::default();

        conv.push(msg(Role::User, "hello"));
        conv.push(msg(Role::Assistant, "hi"));

        // Delete the user message
        let effect = dispatch(
            Command::Delete { index: Some(0) },
            &mut conv,
            &mut whispers,
            &mut LiveSettings::default(),
        );
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(conv.messages().len(), 1);
        assert_eq!(conv.messages()[0].role, Role::Assistant);

        // Retry — pops assistant, no trigger left
        let effect = dispatch(
            Command::Retry { text: None },
            &mut conv,
            &mut whispers,
            &mut LiveSettings::default(),
        );
        assert_eq!(conv.messages().len(), 0);
        match effect {
            Effect::Multi(effects) => {
                assert!(matches!(effects[0], Effect::Sync));
                assert!(matches!(effects[1], Effect::Error(_)));
            }
            _ => panic!("expected Multi with Sync + Error"),
        }
    }

    #[test]
    fn whisper_then_cancel() {
        let mut conv = Conversation::new();
        let mut whispers = WhisperStack::default();

        let effect = dispatch(
            Command::Whisper {
                turns: 3,
                text: "stay dark".into(),
            },
            &mut conv,
            &mut whispers,
            &mut LiveSettings::default(),
        );
        assert!(matches!(effect, Effect::WhispersChanged));
        assert_eq!(whispers.active().len(), 1);

        let effect = dispatch(
            Command::Cancel { index: 0 },
            &mut conv,
            &mut whispers,
            &mut LiveSettings::default(),
        );
        assert!(matches!(effect, Effect::WhispersChanged));
        assert!(whispers.active().is_empty());
    }

    #[test]
    fn system_then_delete_system_message() {
        let mut conv = Conversation::new();
        let mut whispers = WhisperStack::default();

        let effect = dispatch(
            Command::System {
                text: "set the scene".into(),
            },
            &mut conv,
            &mut whispers,
            &mut LiveSettings::default(),
        );
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(conv.messages().len(), 1);
        assert_eq!(conv.messages()[0].role, Role::System);

        let effect = dispatch(
            Command::Delete { index: Some(0) },
            &mut conv,
            &mut whispers,
            &mut LiveSettings::default(),
        );
        assert!(matches!(effect, Effect::Sync));
        assert!(conv.messages().is_empty());
    }

    // --- execute_cancel edge cases ---

    #[test]
    fn cancel_empty_stack_is_error() {
        let mut whispers = WhisperStack::default();
        let effect = execute_cancel(&mut whispers, 0);
        assert!(matches!(effect, Effect::Error(_)));
    }

    #[test]
    fn cancel_out_of_bounds_is_error() {
        let mut whispers = WhisperStack::default();
        whispers.add("test".into(), 3);
        let effect = execute_cancel(&mut whispers, 5);
        assert!(matches!(effect, Effect::Error(_)));
        assert_eq!(whispers.active().len(), 1);
    }

    #[test]
    fn user_pushes_and_syncs() {
        let mut conv = Conversation::new();
        let effect = execute_user(&mut conv, "I sit down".into());
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(conv.messages().len(), 1);
        assert_eq!(conv.messages()[0].role, Role::User);
        assert_eq!(conv.messages()[0].content, "I sit down");
    }

    #[test]
    fn assistant_pushes_and_syncs() {
        let mut conv = Conversation::new();
        let effect = execute_assistant(&mut conv, "I nod".into());
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(conv.messages().len(), 1);
        assert_eq!(conv.messages()[0].role, Role::Assistant);
        assert_eq!(conv.messages()[0].content, "I nod");
    }

    // --- execute_end_wrapper_edit ---

    #[test]
    fn end_wrapper_edit_counts_end_wrappers_only() {
        let mut wrappers = mixed_wrappers();
        let effect = execute_end_wrapper_edit(&mut wrappers, 1, "new end b".into());
        assert!(matches!(effect, Effect::Sync));
        let contents: Vec<_> = wrappers.iter().map(|w| w.content.as_str()).collect();
        assert_eq!(contents, ["start a", "end a", "start b", "new end b"]);
    }

    #[test]
    fn end_wrapper_edit_first() {
        let mut wrappers = mixed_wrappers();
        let effect = execute_end_wrapper_edit(&mut wrappers, 0, "new end a".into());
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(wrappers[1].content, "new end a");
        assert_eq!(wrappers[0].content, "start a");
    }

    #[test]
    fn end_wrapper_edit_keeps_position_and_in_summarize() {
        let mut wrappers = mixed_wrappers();
        execute_end_wrapper_edit(&mut wrappers, 1, "x".into());
        assert!(matches!(wrappers[3].position, Position::End));
        assert!(wrappers[3].in_summarize);
    }

    #[test]
    fn end_wrapper_edit_out_of_bounds_is_error_and_unchanged() {
        let mut wrappers = mixed_wrappers();
        let before: Vec<_> = wrappers.iter().map(|w| w.content.clone()).collect();
        let effect = execute_end_wrapper_edit(&mut wrappers, 2, "x".into());
        assert!(matches!(effect, Effect::Error(_)));
        let after: Vec<_> = wrappers.iter().map(|w| w.content.clone()).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn end_wrapper_edit_with_only_start_wrappers_is_error() {
        let mut wrappers = vec![wrapper(Position::Start, "persona", false)];
        let effect = execute_end_wrapper_edit(&mut wrappers, 0, "x".into());
        assert!(matches!(effect, Effect::Error(_)));
        assert_eq!(wrappers[0].content, "persona");
    }

    #[test]
    fn dispatch_end_wrapper_edit_leaves_conversation_and_whispers() {
        let mut conv = Conversation::new();
        conv.push(msg(Role::User, "hello"));
        let mut whispers = WhisperStack::default();
        whispers.add("stay dark".into(), 2);
        let mut live = LiveSettings {
            wrappers: mixed_wrappers(),
            ..Default::default()
        };

        let effect = dispatch(
            Command::EndWrapperEdit {
                index: 0,
                text: "Two paragraphs.".into(),
            },
            &mut conv,
            &mut whispers,
            &mut live,
        );
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(live.wrappers[1].content, "Two paragraphs.");
        assert_eq!(conv.messages().len(), 1);
        assert_eq!(whispers.active().len(), 1);
    }

    // --- execute_end_wrapper_add ---

    fn contents(wrappers: &[Wrapper]) -> Vec<&str> {
        wrappers.iter().map(|w| w.content.as_str()).collect()
    }

    #[test]
    fn end_wrapper_add_inserts_before_nth_end_wrapper() {
        let mut wrappers = mixed_wrappers();
        let effect = execute_end_wrapper_add(&mut wrappers, 1, "new".into());
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(
            contents(&wrappers),
            ["start a", "end a", "start b", "new", "end b"]
        );
    }

    #[test]
    fn end_wrapper_add_at_zero_goes_before_first_end_wrapper() {
        let mut wrappers = mixed_wrappers();
        execute_end_wrapper_add(&mut wrappers, 0, "new".into());
        assert_eq!(
            contents(&wrappers),
            ["start a", "new", "end a", "start b", "end b"]
        );
    }

    #[test]
    fn end_wrapper_add_at_count_appends_after_last_end_wrapper() {
        let mut wrappers = mixed_wrappers();
        wrappers.push(wrapper(Position::Start, "start c", false));
        let effect = execute_end_wrapper_add(&mut wrappers, 2, "new".into());
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(
            contents(&wrappers),
            ["start a", "end a", "start b", "end b", "new", "start c"]
        );
    }

    #[test]
    fn end_wrapper_add_with_no_end_wrappers_appends() {
        let mut wrappers = vec![wrapper(Position::Start, "persona", false)];
        let effect = execute_end_wrapper_add(&mut wrappers, 0, "new".into());
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(contents(&wrappers), ["persona", "new"]);
    }

    #[test]
    fn end_wrapper_add_is_end_and_not_in_summarize() {
        let mut wrappers = Vec::new();
        execute_end_wrapper_add(&mut wrappers, 0, "new".into());
        assert_eq!(wrappers.len(), 1);
        assert!(matches!(wrappers[0].position, Position::End));
        assert!(!wrappers[0].in_summarize);
    }

    #[test]
    fn end_wrapper_add_past_count_is_error_and_unchanged() {
        let mut wrappers = mixed_wrappers();
        let effect = execute_end_wrapper_add(&mut wrappers, 3, "new".into());
        assert!(matches!(effect, Effect::Error(_)));
        assert_eq!(contents(&wrappers), contents(&mixed_wrappers()));
    }

    // --- execute_end_wrapper_remove ---

    #[test]
    fn end_wrapper_remove_counts_end_wrappers_only() {
        let mut wrappers = mixed_wrappers();
        let effect = execute_end_wrapper_remove(&mut wrappers, 1);
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(contents(&wrappers), ["start a", "end a", "start b"]);
    }

    #[test]
    fn end_wrapper_remove_all_leaves_start_wrappers() {
        let mut wrappers = mixed_wrappers();
        execute_end_wrapper_remove(&mut wrappers, 0);
        execute_end_wrapper_remove(&mut wrappers, 0);
        assert_eq!(contents(&wrappers), ["start a", "start b"]);
        let effect = execute_end_wrapper_remove(&mut wrappers, 0);
        assert!(matches!(effect, Effect::Error(_)));
        assert_eq!(contents(&wrappers), ["start a", "start b"]);
    }

    #[test]
    fn end_wrapper_remove_out_of_bounds_is_error_and_unchanged() {
        let mut wrappers = mixed_wrappers();
        let effect = execute_end_wrapper_remove(&mut wrappers, 2);
        assert!(matches!(effect, Effect::Error(_)));
        assert_eq!(contents(&wrappers), contents(&mixed_wrappers()));
    }

    #[test]
    fn dispatch_end_wrapper_add_edit_remove() {
        let mut conv = Conversation::new();
        let mut whispers = WhisperStack::default();
        let mut live = LiveSettings {
            wrappers: mixed_wrappers(),
            ..Default::default()
        };

        let mut run = |cmd| dispatch(cmd, &mut conv, &mut whispers, &mut live);
        assert!(matches!(
            run(Command::EndWrapperAdd {
                index: 2,
                text: "third".into()
            }),
            Effect::Sync
        ));
        assert!(matches!(
            run(Command::EndWrapperEdit {
                index: 2,
                text: "third, edited".into()
            }),
            Effect::Sync
        ));
        assert!(matches!(
            run(Command::EndWrapperRemove { index: 0 }),
            Effect::Sync
        ));

        assert_eq!(
            contents(&live.wrappers),
            ["start a", "start b", "end b", "third, edited"]
        );
    }

    // --- execute_sampler ---

    fn set(s: &mut SamplerConfig, key: &str, value: &str) -> Effect {
        execute_sampler(s, key.into(), Some(value.into()))
    }

    fn is_error_then_sync(effect: &Effect) -> bool {
        matches!(effect, Effect::Multi(e) if e.len() == 2
            && matches!(e[0], Effect::Error(_))
            && matches!(e[1], Effect::Sync))
    }

    #[test]
    fn sampler_set_each_value_kind() {
        let mut s = SamplerConfig::default();
        assert!(matches!(set(&mut s, "temperature", "0.9"), Effect::Sync));
        assert!(matches!(set(&mut s, "top_k", "40"), Effect::Sync));
        assert!(matches!(set(&mut s, "seed", "-1"), Effect::Sync));
        assert!(matches!(
            set(&mut s, "stop", r#"["\n\n", "User:"]"#),
            Effect::Sync
        ));
        assert_eq!(s.temperature, Some(0.9));
        assert_eq!(s.top_k, Some(40));
        assert_eq!(s.seed, Some(-1));
        assert_eq!(s.stop, Some(vec!["\n\n".to_string(), "User:".to_string()]));
    }

    #[test]
    fn sampler_float_field_accepts_integer_json() {
        let mut s = SamplerConfig::default();
        assert!(matches!(set(&mut s, "temperature", "1"), Effect::Sync));
        assert_eq!(s.temperature, Some(1.0));
    }

    #[test]
    fn sampler_unset_clears() {
        let mut s = SamplerConfig {
            top_k: Some(40),
            ..Default::default()
        };
        let effect = execute_sampler(&mut s, "top_k".into(), None);
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(s.top_k, None);
    }

    #[test]
    fn sampler_accepts_range_boundaries() {
        let max_seed = u32::MAX.to_string();
        let ok = [
            ("temperature", "0"),
            ("min_p", "0"),
            ("min_p", "1"),
            ("top_k", "0"),
            ("top_p", "1"),
            ("max_tokens", "-1"),
            ("repeat_last_n", "0"),
            ("dry_base", "1"),
            ("dry_allowed_length", "0"),
            ("dry_penalty_last_n", "0"),
            ("xtc_probability", "1"),
            ("xtc_threshold", "0"),
            ("seed", max_seed.as_str()),
            ("repeat_penalty", "-3.5"),
            ("dry_multiplier", "100"),
        ];
        for (key, value) in ok {
            let mut s = SamplerConfig::default();
            assert!(
                matches!(set(&mut s, key, value), Effect::Sync),
                "{key} = {value} should be accepted"
            );
        }
    }

    #[test]
    fn sampler_rejects_out_of_range_and_leaves_value() {
        let too_big_seed = (u64::from(u32::MAX) + 1).to_string();
        let bad = [
            ("temperature", "-0.1"),
            ("min_p", "1.01"),
            ("min_p", "-0.01"),
            ("top_k", "-1"),
            ("top_p", "2"),
            ("max_tokens", "-2"),
            ("repeat_last_n", "-1"),
            ("dry_base", "0.99"),
            ("dry_allowed_length", "-1"),
            ("dry_penalty_last_n", "-1"),
            ("xtc_probability", "1.5"),
            ("xtc_threshold", "-1"),
            ("seed", "-2"),
            ("seed", too_big_seed.as_str()),
        ];
        for (key, value) in bad {
            let mut s = SamplerConfig {
                temperature: Some(0.7),
                ..Default::default()
            };
            let before = serde_json::to_value(&s).unwrap();
            let effect = set(&mut s, key, value);
            assert!(
                is_error_then_sync(&effect),
                "{key} = {value} should be rejected"
            );
            assert_eq!(serde_json::to_value(&s).unwrap(), before, "{key} changed");
        }
    }

    #[test]
    fn sampler_rejects_wrong_types() {
        for (key, value) in [
            ("top_k", "40.0"),
            ("top_k", "\"40\""),
            ("temperature", "true"),
            ("stop", "\"User:\""),
            ("samplers", "[1]"),
            ("temperature", "warm"),
        ] {
            let mut s = SamplerConfig::default();
            assert!(
                is_error_then_sync(&set(&mut s, key, value)),
                "{key} = {value}"
            );
            assert_eq!(serde_json::to_value(&s).unwrap(), serde_json::json!({}));
        }
    }

    #[test]
    fn sampler_rejects_null_and_points_to_unset() {
        let mut s = SamplerConfig {
            top_k: Some(40),
            ..Default::default()
        };
        match set(&mut s, "top_k", "null") {
            Effect::Multi(e) => match &e[0] {
                Effect::Error(msg) => assert!(msg.contains("unset"), "{msg}"),
                _ => panic!("expected Error first"),
            },
            _ => panic!("expected Multi"),
        }
        assert_eq!(s.top_k, Some(40));
    }

    #[test]
    fn sampler_rejects_empty_lists_and_entries() {
        for (key, value) in [
            ("stop", "[]"),
            ("samplers", "[]"),
            ("dry_sequence_breakers", "[]"),
            ("stop", r#"["User:", ""]"#),
        ] {
            let mut s = SamplerConfig::default();
            assert!(
                is_error_then_sync(&set(&mut s, key, value)),
                "{key} = {value}"
            );
        }
    }

    #[test]
    fn sampler_names_pass_through_unchecked() {
        let mut s = SamplerConfig::default();
        let effect = set(&mut s, "samplers", r#"["penalties", "not_a_sampler"]"#);
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(
            s.samplers,
            Some(vec!["penalties".to_string(), "not_a_sampler".to_string()])
        );
    }

    #[test]
    fn sampler_sequence_breakers_limit_counts_distinct() {
        let distinct: Vec<String> = (0..64).map(|i| format!("b{i}")).collect();
        let mut with_dupes = distinct.clone();
        with_dupes.push("b0".to_string());
        let mut s = SamplerConfig::default();
        let json = serde_json::to_string(&with_dupes).unwrap();
        assert!(matches!(
            set(&mut s, "dry_sequence_breakers", &json),
            Effect::Sync
        ));

        let mut too_many = distinct;
        too_many.push("b64".to_string());
        let json = serde_json::to_string(&too_many).unwrap();
        assert!(is_error_then_sync(&set(
            &mut s,
            "dry_sequence_breakers",
            &json
        )));
        assert_eq!(s.dry_sequence_breakers.as_ref().map(Vec::len), Some(65));
    }

    #[test]
    fn sampler_cache_prompt_is_read_only() {
        let mut s = SamplerConfig {
            cache_prompt: Some(true),
            ..Default::default()
        };
        assert!(is_error_then_sync(&set(&mut s, "cache_prompt", "false")));
        assert!(is_error_then_sync(&execute_sampler(
            &mut s,
            "cache_prompt".into(),
            None
        )));
        assert_eq!(s.cache_prompt, Some(true));
    }

    #[test]
    fn sampler_unknown_key_is_error() {
        let mut s = SamplerConfig::default();
        assert!(is_error_then_sync(&set(&mut s, "temprature", "0.9")));
        assert!(is_error_then_sync(&execute_sampler(
            &mut s,
            "mirostat".into(),
            None
        )));
    }

    #[test]
    fn sampler_every_editable_field_is_handled() {
        for key in SamplerConfig::FIELDS {
            let mut s = SamplerConfig::default();
            let result = update_sampler_field(&mut s, key, None);
            if SamplerConfig::READ_ONLY.contains(key) {
                assert!(result.is_err(), "{key} should be read-only");
            } else {
                assert!(result.is_ok(), "{key} is in FIELDS but not handled");
            }
        }
    }

    #[test]
    fn dispatch_sampler_set_changes_only_sampler() {
        let mut conv = Conversation::new();
        let mut whispers = WhisperStack::default();
        let mut live = LiveSettings {
            wrappers: mixed_wrappers(),
            ..Default::default()
        };
        let effect = dispatch(
            Command::SamplerSet {
                key: "min_p".into(),
                value: "0.05".into(),
            },
            &mut conv,
            &mut whispers,
            &mut live,
        );
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(live.sampler.min_p, Some(0.05));
        assert_eq!(contents(&live.wrappers), contents(&mixed_wrappers()));
    }
}
