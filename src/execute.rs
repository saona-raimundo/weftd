// src/execute.rs

use crate::chat::{Conversation, Message, Role};
use crate::command::Command;
use crate::config::{Position, Wrapper};
use crate::whisper::WhisperStack;

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
    wrappers: &mut Vec<Wrapper>,
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
        Command::EndWrapperEdit { index, text } => execute_end_wrapper_edit(wrappers, index, text),
        Command::EndWrapperAdd { index, text } => execute_end_wrapper_add(wrappers, index, text),
        Command::EndWrapperRemove { index } => execute_end_wrapper_remove(wrappers, index),
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
    use crate::config::{Position, Wrapper};
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
            &mut Vec::new(),
        );
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(conv.messages().len(), 1);
        assert_eq!(conv.messages()[0].role, Role::Assistant);

        // Retry — pops assistant, no trigger left
        let effect = dispatch(
            Command::Retry { text: None },
            &mut conv,
            &mut whispers,
            &mut Vec::new(),
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
            &mut Vec::new(),
        );
        assert!(matches!(effect, Effect::WhispersChanged));
        assert_eq!(whispers.active().len(), 1);

        let effect = dispatch(
            Command::Cancel { index: 0 },
            &mut conv,
            &mut whispers,
            &mut Vec::new(),
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
            &mut Vec::new(),
        );
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(conv.messages().len(), 1);
        assert_eq!(conv.messages()[0].role, Role::System);

        let effect = dispatch(
            Command::Delete { index: Some(0) },
            &mut conv,
            &mut whispers,
            &mut Vec::new(),
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
        let mut wrappers = mixed_wrappers();

        let effect = dispatch(
            Command::EndWrapperEdit {
                index: 0,
                text: "Two paragraphs.".into(),
            },
            &mut conv,
            &mut whispers,
            &mut wrappers,
        );
        assert!(matches!(effect, Effect::Sync));
        assert_eq!(wrappers[1].content, "Two paragraphs.");
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
        let mut wrappers = mixed_wrappers();

        let mut run = |cmd| dispatch(cmd, &mut conv, &mut whispers, &mut wrappers);
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
            contents(&wrappers),
            ["start a", "start b", "end b", "third, edited"]
        );
    }
}
