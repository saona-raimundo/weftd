// src/chat.rs

use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::User => write!(f, "user"),
            Role::Assistant => write!(f, "assistant"),
            Role::System => write!(f, "system"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum Action {
    Submit {
        role: Role,
        content: String,
        #[serde(default)]
        submit_id: Option<String>,
    },
    Edit {
        index: usize,
        content: String,
    },
    #[serde(rename = "summary_edit")]
    SummaryEdit {
        text: String,
        up_to: usize,
    },
    #[serde(rename = "summary_trigger")]
    SummaryTrigger,
}

pub struct Conversation {
    messages: Vec<Message>,
}

impl Conversation {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    pub fn edit(&mut self, index: usize, content: String) {
        if let Some(msg) = self.messages.get_mut(index) {
            msg.content = content;
        }
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Remove and return the last message, regardless of role.
    pub fn pop(&mut self) -> Option<Message> {
        self.messages.pop()
    }

    /// Remove message at index. Returns the removed message, or None if out of bounds.
    pub fn remove(&mut self, index: usize) -> Option<Message> {
        if index < self.messages.len() {
            Some(self.messages.remove(index))
        } else {
            None
        }
    }
}

pub type SharedConversation = Arc<Mutex<Conversation>>;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SummaryState {
    pub text: String,
    /// index where last summary ended
    pub up_to: usize,
}

impl SummaryState {
    pub fn new() -> Self {
        Self::default()
    }
}

impl From<String> for SummaryState {
    fn from(text: String) -> Self {
        Self {
            text,
            ..Self::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: Role, content: &str) -> Message {
        Message {
            role,
            content: content.to_string(),
        }
    }

    #[test]
    fn pop_returns_last() {
        let mut conv = Conversation::new();
        conv.push(msg(Role::User, "hello"));
        conv.push(msg(Role::Assistant, "hi"));
        let popped = conv.pop().unwrap();
        assert_eq!(popped.role, Role::Assistant);
        assert_eq!(popped.content, "hi");
        assert_eq!(conv.messages().len(), 1);
    }

    #[test]
    fn pop_empty_returns_none() {
        let mut conv = Conversation::new();
        assert!(conv.pop().is_none());
    }

    #[test]
    fn remove_valid_index() {
        let mut conv = Conversation::new();
        conv.push(msg(Role::User, "a"));
        conv.push(msg(Role::Assistant, "b"));
        conv.push(msg(Role::User, "c"));
        let removed = conv.remove(1).unwrap();
        assert_eq!(removed.content, "b");
        assert_eq!(conv.messages().len(), 2);
        assert_eq!(conv.messages()[0].content, "a");
        assert_eq!(conv.messages()[1].content, "c");
    }

    #[test]
    fn remove_out_of_bounds_returns_none() {
        let mut conv = Conversation::new();
        conv.push(msg(Role::User, "a"));
        assert!(conv.remove(5).is_none());
    }

    #[test]
    fn remove_empty_returns_none() {
        let mut conv = Conversation::new();
        assert!(conv.remove(0).is_none());
    }

    #[test]
    fn submit_with_id_parses() {
        let json = r#"{"action":"submit","role":"user","content":"hi","submit_id":"abc"}"#;
        let action: Action = serde_json::from_str(json).unwrap();
        match action {
            Action::Submit { submit_id, .. } => assert_eq!(submit_id.as_deref(), Some("abc")),
            _ => panic!("expected Submit"),
        }
    }

    #[test]
    fn submit_without_id_parses() {
        let json = r#"{"action":"submit","role":"user","content":"hi"}"#;
        let action: Action = serde_json::from_str(json).unwrap();
        match action {
            Action::Submit { submit_id, .. } => assert!(submit_id.is_none()),
            _ => panic!("expected Submit"),
        }
    }
}
