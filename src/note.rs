//! Author's Note: a short text that belongs to one chat and is added to the
//! prompt of every reply (or every N-th one), to steer the story.
//!
//! It is kept in the chat file's metadata under the names SillyTavern uses
//! (`note_prompt`, `note_position`, `note_depth`, `note_role`,
//! `note_interval`), so a chat copied from SillyTavern keeps its note and the
//! web UI shows the one written here. `/note` edits it (see `App::open_note`);
//! `textgen::build_prompt` puts it into the prompt. A persona whose position is
//! *Top / Bottom of the Author's Note* (see `posedit`) is added to the note's
//! text there.

use crate::chat::ChatState;
use crate::jv::*;
use crate::posedit::{BOTTOM_NOTE, TOP_NOTE};
use serde_json::json;

/// After the system prompt and the character card (SillyTavern: "After
/// Main Prompt / Story String"): `{{anchorAfter}}` of the story string.
pub const IN_PROMPT: i64 = 0;
/// Into the chat history, `depth` messages from the end.
pub const IN_CHAT: i64 = 1;
/// Before the system prompt and the character card (`{{anchorBefore}}`).
pub const BEFORE_PROMPT: i64 = 2;

const DEFAULT_DEPTH: i64 = 4;
pub const MAX_DEPTH: usize = 999;
pub const MAX_INTERVAL: usize = 9999;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    /// `IN_PROMPT`, `IN_CHAT` or `BEFORE_PROMPT`.
    pub position: i64,
    /// Messages from the end, for the chat position (0 = after the last one).
    pub depth: usize,
    /// 0 system, 1 user, 2 assistant: who the chat position says it is from.
    pub role: u8,
    /// The note is sent with every N-th message of yours; 0 = never.
    pub interval: usize,
}

impl Default for Note {
    fn default() -> Note {
        Note {
            text: String::new(),
            position: IN_CHAT,
            depth: DEFAULT_DEPTH as usize,
            role: 0,
            interval: 1,
        }
    }
}

impl Note {
    /// The note of this chat (the default, empty one when it has none).
    pub fn from_chat(chat: &ChatState) -> Note {
        let d = Note::default();
        let Some(m) = chat.header.get("chat_metadata") else {
            return d;
        };
        let position = ji64(m, "note_position", d.position);
        Note {
            text: jstr(m, "note_prompt"),
            position: if matches!(position, IN_PROMPT | IN_CHAT | BEFORE_PROMPT) {
                position
            } else {
                d.position
            },
            depth: ji64(m, "note_depth", DEFAULT_DEPTH).clamp(0, MAX_DEPTH as i64) as usize,
            role: ji64(m, "note_role", 0).clamp(0, 2) as u8,
            interval: ji64(m, "note_interval", 1).clamp(0, MAX_INTERVAL as i64) as usize,
        }
    }

    /// Write the note into the chat's metadata (the caller saves the chat).
    pub fn store(&self, chat: &mut ChatState) {
        if !chat
            .header
            .get("chat_metadata")
            .map(|m| m.is_object())
            .unwrap_or(false)
        {
            chat.header["chat_metadata"] = json!({});
        }
        let m = &mut chat.header["chat_metadata"];
        m["note_prompt"] = json!(self.text);
        m["note_position"] = json!(self.position);
        m["note_depth"] = json!(self.depth);
        m["note_role"] = json!(self.role);
        m["note_interval"] = json!(self.interval);
    }

    /// Is the note sent with this reply? `user_messages` is how many messages
    /// of yours the prompt holds.
    pub fn due(&self, user_messages: usize) -> bool {
        self.interval > 0 && user_messages % self.interval == 0
    }
}

/// What goes into the prompt as the note: the persona's description above or
/// below the note's text, when the persona's position says so, each part on
/// its own line. Empty when there is nothing to send.
pub fn compose(note_text: &str, persona: &str, persona_position: i64) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if persona_position == TOP_NOTE {
        parts.push(persona);
    }
    parts.push(note_text);
    if persona_position == BOTTOM_NOTE {
        parts.push(persona);
    }
    parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect::<Vec<&str>>()
        .join("\n")
}

// ---------------------------------------------------- words in the /note form

pub fn position_word(p: i64) -> &'static str {
    match p {
        IN_PROMPT => "after",
        BEFORE_PROMPT => "before",
        _ => "chat",
    }
}

pub fn parse_position(s: &str) -> Option<i64> {
    match s.trim().to_lowercase().as_str() {
        "after" | "prompt" | "0" => Some(IN_PROMPT),
        "chat" | "depth" | "1" => Some(IN_CHAT),
        "before" | "2" => Some(BEFORE_PROMPT),
        _ => None,
    }
}

pub fn role_word(r: u8) -> &'static str {
    match r {
        1 => "user",
        2 => "assistant",
        _ => "system",
    }
}

pub fn parse_role(s: &str) -> Option<u8> {
    match s.trim().to_lowercase().as_str() {
        "system" | "sys" | "0" => Some(0),
        "user" | "1" => Some(1),
        "assistant" | "char" | "2" => Some(2),
        _ => None,
    }
}

/// A whole number from `0` to `max`.
pub fn parse_number(s: &str, max: usize) -> Option<usize> {
    s.trim().parse::<usize>().ok().filter(|n| *n <= max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat() -> ChatState {
        ChatState::new_empty("Sofia.png", "Sofia", "Sofia01")
    }

    #[test]
    fn a_chat_without_a_note_has_the_default_one() {
        let c = chat();
        let n = Note::from_chat(&c);
        assert_eq!(n, Note::default());
        assert!(n.text.is_empty());
        assert_eq!(n.position, IN_CHAT);
        assert_eq!(n.depth, 4);
        assert_eq!(n.interval, 1);
    }

    #[test]
    fn the_note_survives_a_round_trip_through_the_metadata() {
        let mut c = chat();
        let n = Note {
            text: "Keep it slow.".into(),
            position: BEFORE_PROMPT,
            depth: 7,
            role: 2,
            interval: 3,
        };
        n.store(&mut c);
        assert_eq!(c.header["chat_metadata"]["note_prompt"], "Keep it slow.");
        assert_eq!(Note::from_chat(&c), n);
        // The other metadata is left alone.
        assert!(c.header["chat_metadata"]["integrity"].is_string());
    }

    #[test]
    fn strange_values_in_the_file_fall_back() {
        let mut c = chat();
        c.header["chat_metadata"]["note_position"] = json!(9);
        c.header["chat_metadata"]["note_depth"] = json!(-3);
        c.header["chat_metadata"]["note_role"] = json!(7);
        let n = Note::from_chat(&c);
        assert_eq!(n.position, IN_CHAT);
        assert_eq!(n.depth, 0);
        assert_eq!(n.role, 2);
    }

    #[test]
    fn the_interval_counts_your_messages() {
        let mut n = Note::default();
        assert!(n.due(0) && n.due(1) && n.due(5));
        n.interval = 3;
        assert!(n.due(3) && n.due(6));
        assert!(!n.due(1) && !n.due(4));
        n.interval = 0;
        assert!(!n.due(0) && !n.due(3));
    }

    #[test]
    fn the_persona_goes_above_or_below_the_text() {
        assert_eq!(compose("note", "me", TOP_NOTE), "me\nnote");
        assert_eq!(compose("note", "me", BOTTOM_NOTE), "note\nme");
        assert_eq!(compose("note", "me", 0), "note");
        assert_eq!(compose("", "me", TOP_NOTE), "me");
        assert_eq!(compose("", "me", 4), "");
        assert_eq!(compose(" note \n", " me ", BOTTOM_NOTE), "note\nme");
    }

    #[test]
    fn the_words_of_the_form() {
        assert_eq!(parse_position(" Chat "), Some(IN_CHAT));
        assert_eq!(parse_position("before"), Some(BEFORE_PROMPT));
        assert_eq!(parse_position("after"), Some(IN_PROMPT));
        assert_eq!(parse_position("middle"), None);
        assert_eq!(position_word(parse_position("before").unwrap()), "before");
        assert_eq!(parse_role("User"), Some(1));
        assert_eq!(parse_role("narrator"), None);
        assert_eq!(role_word(parse_role("assistant").unwrap()), "assistant");
        assert_eq!(parse_number(" 12 ", MAX_DEPTH), Some(12));
        assert_eq!(parse_number("1000", MAX_DEPTH), None);
        assert_eq!(parse_number("-1", MAX_DEPTH), None);
        assert_eq!(parse_number("", MAX_DEPTH), None);
    }
}
