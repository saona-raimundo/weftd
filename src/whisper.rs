// src/whisper.rs

use serde::{Deserialize, Serialize};

use crate::chat::{Message, Role};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Whisper {
    pub text: String,
    pub turns_remaining: u32,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct WhisperStack {
    whispers: Vec<Whisper>,
}

impl WhisperStack {
    pub fn add(&mut self, text: String, turns: u32) {
        self.whispers.push(Whisper {
            text,
            turns_remaining: turns,
        });
    }

    pub fn inject(&self) -> Vec<Message> {
        self.whispers
            .iter()
            .map(|w| Message {
                role: Role::System,
                content: w.text.clone(),
            })
            .collect()
    }

    pub fn tick(&mut self) {
        for w in &mut self.whispers {
            w.turns_remaining = w.turns_remaining.saturating_sub(1);
        }
        self.whispers.retain(|w| w.turns_remaining > 0);
    }

    pub fn cancel(&mut self, index: usize) -> Option<Whisper> {
        if index < self.whispers.len() {
            Some(self.whispers.remove(index))
        } else {
            None
        }
    }

    pub fn active(&self) -> &[Whisper] {
        &self.whispers
    }

    pub fn is_empty(&self) -> bool {
        self.whispers.is_empty()
    }
}

impl From<Vec<Whisper>> for WhisperStack {
    fn from(whispers: Vec<Whisper>) -> Self {
        Self { whispers }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_and_active() {
        let mut stack = WhisperStack::default();
        stack.add("stay tense".into(), 2);
        stack.add("speak formally".into(), 1);

        assert_eq!(stack.active().len(), 2);
        assert_eq!(stack.active()[0].text, "stay tense");
        assert_eq!(stack.active()[1].text, "speak formally");
    }

    #[test]
    fn inject_produces_system_messages() {
        let mut stack = WhisperStack::default();
        stack.add("be hostile".into(), 3);

        let messages = stack.inject();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, Role::System);
        assert_eq!(messages[0].content, "be hostile");
    }

    #[test]
    fn inject_preserves_creation_order() {
        let mut stack = WhisperStack::default();
        stack.add("first".into(), 2);
        stack.add("second".into(), 1);

        let messages = stack.inject();
        assert_eq!(messages[0].content, "first");
        assert_eq!(messages[1].content, "second");
    }

    #[test]
    fn tick_decrements_and_removes() {
        let mut stack = WhisperStack::default();
        stack.add("short".into(), 1);
        stack.add("long".into(), 3);

        stack.tick();
        assert_eq!(stack.active().len(), 1);
        assert_eq!(stack.active()[0].text, "long");
        assert_eq!(stack.active()[0].turns_remaining, 2);
    }

    #[test]
    fn tick_removes_all_when_expired() {
        let mut stack = WhisperStack::default();
        stack.add("one".into(), 1);
        stack.add("two".into(), 1);

        stack.tick();
        assert!(stack.is_empty());
    }

    #[test]
    fn tick_on_empty_stack() {
        let mut stack = WhisperStack::default();
        stack.tick();
        assert!(stack.is_empty());
    }

    #[test]
    fn cancel_valid_index() {
        let mut stack = WhisperStack::default();
        stack.add("first".into(), 2);
        stack.add("second".into(), 3);

        let removed = stack.cancel(0);
        assert_eq!(
            removed,
            Some(Whisper {
                text: "first".into(),
                turns_remaining: 2
            })
        );
        assert_eq!(stack.active().len(), 1);
        assert_eq!(stack.active()[0].text, "second");
    }

    #[test]
    fn cancel_out_of_bounds() {
        let mut stack = WhisperStack::default();
        stack.add("only".into(), 1);

        assert_eq!(stack.cancel(5), None);
        assert_eq!(stack.active().len(), 1);
    }

    #[test]
    fn cancel_on_empty_stack() {
        let mut stack = WhisperStack::default();
        assert_eq!(stack.cancel(0), None);
    }

    #[test]
    fn inject_empty_stack() {
        let stack = WhisperStack::default();
        assert!(stack.inject().is_empty());
    }

    #[test]
    fn saturating_sub_prevents_underflow() {
        let mut stack = WhisperStack::default();
        stack.add("test".into(), 0);
        stack.tick(); // should not panic, just removes it
        assert!(stack.is_empty());
    }
    #[test]
    fn from_vec() {
        let whispers = vec![
            Whisper {
                text: "stay tense".into(),
                turns_remaining: 3,
            },
            Whisper {
                text: "speak Korean".into(),
                turns_remaining: 1,
            },
        ];
        let stack = WhisperStack::from(whispers);
        assert_eq!(stack.active().len(), 2);
        assert_eq!(stack.active()[0].text, "stay tense");
        assert_eq!(stack.active()[1].turns_remaining, 1);
    }
}
