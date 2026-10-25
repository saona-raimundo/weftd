// src/command.rs

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum Command {
    Whisper {
        turns: u32,
        text: String,
    },
    Retry {
        text: Option<String>,
    },
    Delete {
        index: Option<usize>,
    },
    System {
        text: String,
    },
    User {
        text: String,
    },
    Assistant {
        text: String,
    },
    Cancel {
        index: usize,
    },
    Continue {
        text: Option<String>,
    },
    Sync,
    /// Replace the content of the `index`-th end wrapper (0-based, end wrappers only).
    EndWrapperEdit {
        index: usize,
        text: String,
    },
    /// Insert a new end wrapper before the `index`-th one; `index == count` appends.
    EndWrapperAdd {
        index: usize,
        text: String,
    },
    /// Remove the `index`-th end wrapper (0-based, end wrappers only).
    EndWrapperRemove {
        index: usize,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ParseResult {
    Fiction,
    Command(Command),
    Unknown(String),
}

pub fn parse(input: &str) -> ParseResult {
    let trimmed = input.trim();

    if !trimmed.starts_with('/') {
        return ParseResult::Fiction;
    }

    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap().to_lowercase();
    let rest = parts.next().map(|s| s.trim()).unwrap_or("");

    match cmd.as_str() {
        "/whisper" => {
            if rest.is_empty() {
                return ParseResult::Unknown(trimmed.to_string());
            }

            // Try parsing first token of rest as turn count
            let mut rest_parts = rest.splitn(2, char::is_whitespace);
            let first = rest_parts.next().unwrap();
            let remainder = rest_parts.next().map(|s| s.trim()).unwrap_or("");

            if let Ok(turns) = first.parse::<u32>() {
                if remainder.is_empty() {
                    // "/whisper 3" with no text — error
                    ParseResult::Unknown(trimmed.to_string())
                } else {
                    ParseResult::Command(Command::Whisper {
                        turns,
                        text: remainder.to_string(),
                    })
                }
            } else {
                // First token isn't a number, so turns=1 and all of rest is the text
                ParseResult::Command(Command::Whisper {
                    turns: 1,
                    text: rest.to_string(),
                })
            }
        }
        "/retry" => {
            let text = if rest.is_empty() {
                None
            } else {
                Some(rest.to_string())
            };
            ParseResult::Command(Command::Retry { text })
        }
        "/continue" => {
            let text = if rest.is_empty() {
                None
            } else {
                Some(rest.to_string())
            };
            ParseResult::Command(Command::Continue { text })
        }
        "/delete" => {
            if rest.is_empty() {
                ParseResult::Command(Command::Delete { index: None })
            } else {
                match rest.parse::<usize>() {
                    Ok(index) => ParseResult::Command(Command::Delete { index: Some(index) }),
                    Err(_) => ParseResult::Unknown(trimmed.to_string()),
                }
            }
        }
        "/system" => {
            if rest.is_empty() {
                return ParseResult::Unknown(trimmed.to_string());
            }
            ParseResult::Command(Command::System {
                text: rest.to_string(),
            })
        }
        "/user" => {
            if rest.is_empty() {
                return ParseResult::Unknown(trimmed.to_string());
            }
            ParseResult::Command(Command::User {
                text: rest.to_string(),
            })
        }
        "/assistant" => {
            if rest.is_empty() {
                return ParseResult::Unknown(trimmed.to_string());
            }
            ParseResult::Command(Command::Assistant {
                text: rest.to_string(),
            })
        }
        "/cancel" => match rest.parse::<usize>() {
            Ok(index) => ParseResult::Command(Command::Cancel { index }),
            _ => ParseResult::Unknown(trimmed.to_string()),
        },
        "/sync" => ParseResult::Command(Command::Sync),
        "/wrapper" => parse_wrapper(trimmed, rest),
        _ => ParseResult::Unknown(trimmed.to_string()),
    }
}

/// `/wrapper end edit|add <n> <text>` and `/wrapper end remove <n>`.
/// Position and verb are case-insensitive.
fn parse_wrapper(raw: &str, rest: &str) -> ParseResult {
    let (position, rest) = split_first(rest);
    let (verb, rest) = split_first(rest);

    match (
        position.to_lowercase().as_str(),
        verb.to_lowercase().as_str(),
    ) {
        ("end", "edit") => match index_and_text(rest) {
            Some((index, text)) => ParseResult::Command(Command::EndWrapperEdit { index, text }),
            None => ParseResult::Unknown(raw.to_string()),
        },
        ("end", "add") => match index_and_text(rest) {
            Some((index, text)) => ParseResult::Command(Command::EndWrapperAdd { index, text }),
            None => ParseResult::Unknown(raw.to_string()),
        },
        ("end", "remove") => match rest.parse::<usize>() {
            Ok(index) => ParseResult::Command(Command::EndWrapperRemove { index }),
            Err(_) => ParseResult::Unknown(raw.to_string()),
        },
        _ => ParseResult::Unknown(raw.to_string()),
    }
}

/// `<n> <text>` with a non-empty text.
fn index_and_text(rest: &str) -> Option<(usize, String)> {
    let (index, text) = split_first(rest);
    match index.parse::<usize>() {
        Ok(index) if !text.is_empty() => Some((index, text.to_string())),
        _ => None,
    }
}

/// Split off the first whitespace-delimited token. The remainder is trimmed
/// at both ends but keeps its inner whitespace (newlines included).
fn split_first(s: &str) -> (&str, &str) {
    let mut parts = s.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or("");
    let rest = parts.next().map(|s| s.trim()).unwrap_or("");
    (first, rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Fiction (no slash) ---

    #[test]
    fn plain_text_is_fiction() {
        assert_eq!(parse("hello world"), ParseResult::Fiction);
    }

    #[test]
    fn empty_string_is_fiction() {
        assert_eq!(parse(""), ParseResult::Fiction);
    }

    #[test]
    fn whitespace_only_is_fiction() {
        assert_eq!(parse("   "), ParseResult::Fiction);
    }

    // --- /whisper ---

    #[test]
    fn whisper_with_turns_and_text() {
        assert_eq!(
            parse("/whisper 3 stay tense and hostile"),
            ParseResult::Command(Command::Whisper {
                turns: 3,
                text: "stay tense and hostile".to_string(),
            })
        );
    }

    #[test]
    fn whisper_default_turns() {
        assert_eq!(
            parse("/whisper stay tense"),
            ParseResult::Command(Command::Whisper {
                turns: 1,
                text: "stay tense".to_string(),
            })
        );
    }

    #[test]
    fn whisper_no_text_is_unknown() {
        assert_eq!(
            parse("/whisper"),
            ParseResult::Unknown("/whisper".to_string())
        );
    }

    #[test]
    fn whisper_turns_but_no_text_is_unknown() {
        assert_eq!(
            parse("/whisper 3"),
            ParseResult::Unknown("/whisper 3".to_string())
        );
    }

    #[test]
    fn whisper_case_insensitive() {
        assert_eq!(
            parse("/Whisper keep it dark"),
            ParseResult::Command(Command::Whisper {
                turns: 1,
                text: "keep it dark".to_string(),
            })
        );
    }

    // --- /retry ---

    #[test]
    fn retry_with_text() {
        assert_eq!(
            parse("/retry fix her tone"),
            ParseResult::Command(Command::Retry {
                text: Some("fix her tone".to_string()),
            })
        );
    }

    #[test]
    fn retry_without_text() {
        assert_eq!(
            parse("/retry"),
            ParseResult::Command(Command::Retry { text: None })
        );
    }

    #[test]
    fn retry_case_insensitive() {
        assert_eq!(
            parse("/RETRY be more dramatic"),
            ParseResult::Command(Command::Retry {
                text: Some("be more dramatic".to_string()),
            })
        );
    }

    // --- /delete ---

    #[test]
    fn delete_no_index() {
        assert_eq!(
            parse("/delete"),
            ParseResult::Command(Command::Delete { index: None })
        );
    }

    #[test]
    fn delete_with_index() {
        assert_eq!(
            parse("/delete 3"),
            ParseResult::Command(Command::Delete { index: Some(3) })
        );
    }

    #[test]
    fn delete_index_zero() {
        assert_eq!(
            parse("/delete 0"),
            ParseResult::Command(Command::Delete { index: Some(0) })
        );
    }

    #[test]
    fn delete_non_numeric_is_unknown() {
        assert_eq!(
            parse("/delete abc"),
            ParseResult::Unknown("/delete abc".to_string())
        );
    }

    #[test]
    fn delete_case_insensitive() {
        assert_eq!(
            parse("/Delete 5"),
            ParseResult::Command(Command::Delete { index: Some(5) })
        );
    }

    #[test]
    fn undo_is_now_unknown() {
        assert_eq!(parse("/undo"), ParseResult::Unknown("/undo".to_string()));
    }

    // --- /system ---

    #[test]
    fn system_with_text() {
        assert_eq!(
            parse("/system describe the sunset"),
            ParseResult::Command(Command::System {
                text: "describe the sunset".to_string(),
            })
        );
    }

    #[test]
    fn system_no_text_is_unknown() {
        assert_eq!(
            parse("/system"),
            ParseResult::Unknown("/system".to_string())
        );
    }

    #[test]
    fn system_case_insensitive() {
        assert_eq!(
            parse("/System keep the mood dark"),
            ParseResult::Command(Command::System {
                text: "keep the mood dark".to_string(),
            })
        );
    }

    // --- /cancel ---

    #[test]
    fn cancel_with_index() {
        assert_eq!(
            parse("/cancel 0"),
            ParseResult::Command(Command::Cancel { index: 0 })
        );
    }

    #[test]
    fn cancel_no_index_is_unknown() {
        assert_eq!(
            parse("/cancel"),
            ParseResult::Unknown("/cancel".to_string())
        );
    }

    #[test]
    fn cancel_non_numeric_is_unknown() {
        assert_eq!(
            parse("/cancel abc"),
            ParseResult::Unknown("/cancel abc".to_string())
        );
    }

    #[test]
    fn cancel_case_insensitive() {
        assert_eq!(
            parse("/Cancel 2"),
            ParseResult::Command(Command::Cancel { index: 2 })
        );
    }

    // --- Unknown commands ---

    #[test]
    fn unknown_slash_command() {
        assert_eq!(
            parse("/blah something"),
            ParseResult::Unknown("/blah something".to_string())
        );
    }

    #[test]
    fn slash_only() {
        assert_eq!(parse("/"), ParseResult::Unknown("/".to_string()));
    }

    // --- Whitespace handling ---

    #[test]
    fn whisper_extra_whitespace() {
        assert_eq!(
            parse("/whisper   3   stay tense"),
            ParseResult::Command(Command::Whisper {
                turns: 3,
                text: "stay tense".to_string(),
            })
        );
    }

    #[test]
    fn sync_command() {
        assert_eq!(parse("/sync"), ParseResult::Command(Command::Sync));
    }

    #[test]
    fn continue_without_text() {
        assert_eq!(
            parse("/continue"),
            ParseResult::Command(Command::Continue { text: None })
        );
    }

    #[test]
    fn continue_with_text() {
        assert_eq!(
            parse("/continue You initiate"),
            ParseResult::Command(Command::Continue {
                text: Some("You initiate".to_string()),
            })
        );
    }

    #[test]
    fn continue_case_insensitive() {
        assert_eq!(
            parse("/Continue keep the tension"),
            ParseResult::Command(Command::Continue {
                text: Some("keep the tension".to_string()),
            })
        );
    }

    // --- /user ---

    #[test]
    fn user_with_text() {
        assert_eq!(
            parse("/user I sit down at the table"),
            ParseResult::Command(Command::User {
                text: "I sit down at the table".to_string(),
            })
        );
    }

    #[test]
    fn user_no_text_is_unknown() {
        assert_eq!(parse("/user"), ParseResult::Unknown("/user".to_string()));
    }

    #[test]
    fn user_case_insensitive() {
        assert_eq!(
            parse("/User I look up"),
            ParseResult::Command(Command::User {
                text: "I look up".to_string(),
            })
        );
    }

    // --- /assistant ---

    #[test]
    fn assistant_with_text() {
        assert_eq!(
            parse("/assistant I turn away from the window"),
            ParseResult::Command(Command::Assistant {
                text: "I turn away from the window".to_string(),
            })
        );
    }

    #[test]
    fn assistant_no_text_is_unknown() {
        assert_eq!(
            parse("/assistant"),
            ParseResult::Unknown("/assistant".to_string())
        );
    }

    #[test]
    fn assistant_case_insensitive() {
        assert_eq!(
            parse("/Assistant I nod"),
            ParseResult::Command(Command::Assistant {
                text: "I nod".to_string(),
            })
        );
    }

    // --- /wrapper ---

    #[test]
    fn wrapper_end_edit() {
        assert_eq!(
            parse("/wrapper end edit 1 Keep it short."),
            ParseResult::Command(Command::EndWrapperEdit {
                index: 1,
                text: "Keep it short.".to_string(),
            })
        );
    }

    #[test]
    fn wrapper_end_edit_keeps_inner_newlines() {
        assert_eq!(
            parse("/wrapper end edit 0 Line one.\n\nLine two.\n"),
            ParseResult::Command(Command::EndWrapperEdit {
                index: 0,
                text: "Line one.\n\nLine two.".to_string(),
            })
        );
    }

    #[test]
    fn wrapper_end_edit_text_may_start_on_next_line() {
        assert_eq!(
            parse("/wrapper end edit 0\nOn its own line."),
            ParseResult::Command(Command::EndWrapperEdit {
                index: 0,
                text: "On its own line.".to_string(),
            })
        );
    }

    #[test]
    fn wrapper_case_insensitive() {
        assert_eq!(
            parse("/Wrapper END Edit 2 Stay in character."),
            ParseResult::Command(Command::EndWrapperEdit {
                index: 2,
                text: "Stay in character.".to_string(),
            })
        );
    }

    #[test]
    fn wrapper_end_edit_no_text_is_unknown() {
        assert_eq!(
            parse("/wrapper end edit 0"),
            ParseResult::Unknown("/wrapper end edit 0".to_string())
        );
    }

    #[test]
    fn wrapper_end_edit_non_numeric_index_is_unknown() {
        assert_eq!(
            parse("/wrapper end edit x Keep it short."),
            ParseResult::Unknown("/wrapper end edit x Keep it short.".to_string())
        );
    }

    #[test]
    fn wrapper_end_edit_no_index_is_unknown() {
        assert_eq!(
            parse("/wrapper end edit"),
            ParseResult::Unknown("/wrapper end edit".to_string())
        );
    }

    #[test]
    fn wrapper_start_is_unknown() {
        assert_eq!(
            parse("/wrapper start edit 0 New persona."),
            ParseResult::Unknown("/wrapper start edit 0 New persona.".to_string())
        );
    }

    #[test]
    fn wrapper_unknown_verb_is_unknown() {
        assert_eq!(
            parse("/wrapper end frob 0 x"),
            ParseResult::Unknown("/wrapper end frob 0 x".to_string())
        );
    }

    #[test]
    fn wrapper_bare_is_unknown() {
        assert_eq!(
            parse("/wrapper"),
            ParseResult::Unknown("/wrapper".to_string())
        );
    }

    #[test]
    fn wrapper_end_add() {
        assert_eq!(
            parse("/wrapper end add 2 No purple prose.\nShow, don't tell."),
            ParseResult::Command(Command::EndWrapperAdd {
                index: 2,
                text: "No purple prose.\nShow, don't tell.".to_string(),
            })
        );
    }

    #[test]
    fn wrapper_end_add_case_insensitive() {
        assert_eq!(
            parse("/wrapper End ADD 0 x"),
            ParseResult::Command(Command::EndWrapperAdd {
                index: 0,
                text: "x".to_string(),
            })
        );
    }

    #[test]
    fn wrapper_end_add_no_text_is_unknown() {
        assert_eq!(
            parse("/wrapper end add 0"),
            ParseResult::Unknown("/wrapper end add 0".to_string())
        );
    }

    #[test]
    fn wrapper_end_add_non_numeric_index_is_unknown() {
        assert_eq!(
            parse("/wrapper end add last x"),
            ParseResult::Unknown("/wrapper end add last x".to_string())
        );
    }

    #[test]
    fn wrapper_end_remove() {
        assert_eq!(
            parse("/wrapper end remove 1"),
            ParseResult::Command(Command::EndWrapperRemove { index: 1 })
        );
    }

    #[test]
    fn wrapper_end_remove_case_insensitive() {
        assert_eq!(
            parse("/WRAPPER end Remove 0"),
            ParseResult::Command(Command::EndWrapperRemove { index: 0 })
        );
    }

    #[test]
    fn wrapper_end_remove_no_index_is_unknown() {
        assert_eq!(
            parse("/wrapper end remove"),
            ParseResult::Unknown("/wrapper end remove".to_string())
        );
    }

    #[test]
    fn wrapper_end_remove_extra_text_is_unknown() {
        assert_eq!(
            parse("/wrapper end remove 1 now"),
            ParseResult::Unknown("/wrapper end remove 1 now".to_string())
        );
    }

    #[test]
    fn wrapper_end_remove_non_numeric_is_unknown() {
        assert_eq!(
            parse("/wrapper end remove all"),
            ParseResult::Unknown("/wrapper end remove all".to_string())
        );
    }
}
