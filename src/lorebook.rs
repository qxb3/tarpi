//! Lorebooks (World Info), the small part of them that character cards carry:
//! the `character_book` of a V2 / V3 card, as SillyTavern and chub.ai write it.
//!
//! An entry is some text that is added to the prompt when one of its keys
//! appears in the latest messages (or always, when it is `constant`). Where it
//! goes is the entry's position: before or after the character's definition
//! (`{{wiBefore}}` / `{{wiAfter}}` of the story string) or at a depth in the
//! chat (like the Author's Note). `textgen::build_prompt` does the placing;
//! this file only reads the book and decides which entries are on.
//!
//! Deliberately left out: regex keys, recursive scanning, probability,
//! sticky / cooldown / delay, inclusion groups, per-entry scan depth and
//! lorebook files of their own. A key is plain text.

use crate::jv::*;
use crate::prompt::est_tokens;
use serde_json::Value;

/// How many of the latest messages are searched for keys when the book does
/// not say (`scan_depth`).
const DEFAULT_SCAN_DEPTH: usize = 4;

/// Where an activated entry's text goes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Place {
    /// `{{wiBefore}}`: ahead of the character's description.
    Before,
    /// `{{wiAfter}}`: after the character's description and scenario.
    After,
    /// Into the chat, `depth` messages from the end (0 = after the last one).
    /// `role`: 0 system, 1 user, 2 assistant.
    Depth { depth: usize, role: u8 },
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub keys: Vec<String>,
    /// Used only when `selective`, as `logic` says.
    pub secondary: Vec<String>,
    pub selective: bool,
    /// How the secondary keys count (SillyTavern's `selectiveLogic`):
    /// 0 any one of them must appear, 3 every one must, 2 none may appear,
    /// 1 they must not all appear.
    pub logic: u8,
    /// On whatever the messages say.
    pub constant: bool,
    pub case_sensitive: bool,
    /// Lower comes first in the prompt; higher wins when the budget is short.
    pub order: i64,
    pub place: Place,
    pub content: String,
}

#[derive(Debug, Clone, Default)]
pub struct Lorebook {
    pub entries: Vec<Entry>,
    /// How many of the latest messages are searched.
    pub scan_depth: usize,
    /// Most tokens the entries may add together; 0 for no limit.
    pub token_budget: usize,
}

/// `["a", "b"]`, or `"a, b"`, as a list of trimmed, non-empty strings.
pub(crate) fn strings(v: Option<&Value>) -> Vec<String> {
    let clean = |s: &str| s.trim().to_string();
    let list: Vec<String> = match v {
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(clean).collect(),
        Some(Value::String(s)) => s.split(',').map(clean).collect(),
        _ => Vec::new(),
    };
    list.into_iter().filter(|s| !s.is_empty()).collect()
}

impl Entry {
    /// One entry of `character_book.entries`; `None` for one that is switched
    /// off or has no text, since it can never do anything.
    fn from_value(e: &Value) -> Option<Entry> {
        if !jbool(e, "enabled", true) {
            return None;
        }
        let content = jstr(e, "content");
        if content.trim().is_empty() {
            return None;
        }
        let null = Value::Null;
        let ext = e.get("extensions").unwrap_or(&null);
        // `extensions.position` is a number (SillyTavern's), the entry's own
        // `position` the V2 words. Of the numbers: 0 before, 1 after, 4 at a
        // depth; the Author's Note and example-message slots (2, 3, 5, 6) have
        // no counterpart here and fall to the nearest of before / after.
        let place = match ji64(ext, "position", -1) {
            4 => Place::Depth {
                depth: ji64(ext, "depth", 4).max(0) as usize,
                role: ji64(ext, "role", 0).clamp(0, 2) as u8,
            },
            1 | 3 | 6 => Place::After,
            0 | 2 | 5 => Place::Before,
            _ => {
                if jstr(e, "position") == "after_char" {
                    Place::After
                } else {
                    Place::Before
                }
            }
        };
        Some(Entry {
            keys: strings(e.get("keys")),
            secondary: strings(e.get("secondary_keys")),
            selective: jbool(e, "selective", false),
            logic: ji64(ext, "selectiveLogic", 0).clamp(0, 3) as u8,
            constant: jbool(e, "constant", false),
            case_sensitive: jbool(e, "case_sensitive", false),
            order: ji64(e, "insertion_order", 100),
            place,
            content,
        })
    }

    /// Does any of `keys` appear in the text? (`lower` is `text` lowercased.)
    fn any_key_in(&self, keys: &[String], text: &str, lower: &str) -> bool {
        keys.iter().any(|k| {
            if self.case_sensitive {
                text.contains(k.as_str())
            } else {
                lower.contains(&k.to_lowercase())
            }
        })
    }

    /// Does every one of `keys` appear in the text?
    fn all_keys_in(&self, keys: &[String], text: &str, lower: &str) -> bool {
        keys.iter().all(|k| {
            if self.case_sensitive {
                text.contains(k.as_str())
            } else {
                lower.contains(&k.to_lowercase())
            }
        })
    }

    fn fires(&self, text: &str, lower: &str) -> bool {
        if self.constant {
            return true;
        }
        if !self.any_key_in(&self.keys, text, lower) {
            return false;
        }
        // A selective entry also looks at its secondary keys; one with none
        // listed is an ordinary entry.
        if !self.selective || self.secondary.is_empty() {
            return true;
        }
        match self.logic {
            1 => !self.all_keys_in(&self.secondary, text, lower),
            2 => !self.any_key_in(&self.secondary, text, lower),
            3 => self.all_keys_in(&self.secondary, text, lower),
            _ => self.any_key_in(&self.secondary, text, lower),
        }
    }
}

impl Lorebook {
    /// The `character_book` of a card's `data` block, if it has entries that
    /// can ever be used.
    pub fn from_card_data(data: &Value) -> Option<Lorebook> {
        let book = data.get("character_book")?;
        let list = book.get("entries")?.as_array()?;
        let entries: Vec<Entry> = list.iter().filter_map(Entry::from_value).collect();
        if entries.is_empty() {
            return None;
        }
        Some(Lorebook {
            entries,
            scan_depth: ji64(book, "scan_depth", DEFAULT_SCAN_DEPTH as i64).max(0) as usize,
            token_budget: ji64(book, "token_budget", 0).max(0) as usize,
        })
    }

    /// The entries that are on for these messages (oldest first), in the order
    /// they go into the prompt: lowest `order` first. When there is a token
    /// budget the entries with the highest `order` are tried first; one that
    /// no longer fits is skipped, and smaller ones after it still get their
    /// turn. (An entry bigger than the whole budget is never used.)
    pub fn activate(&self, messages: &[String], chars_per_token: f32) -> Vec<&Entry> {
        let from = messages.len().saturating_sub(self.scan_depth);
        let text = messages[from..].join("\n");
        let lower = text.to_lowercase();
        let mut on: Vec<&Entry> = self.entries.iter().filter(|e| e.fires(&text, &lower)).collect();
        if self.token_budget > 0 {
            on.sort_by(|a, b| b.order.cmp(&a.order));
            let mut used = 0usize;
            let mut kept = Vec::new();
            for e in on {
                let cost = est_tokens(&e.content, chars_per_token);
                if used + cost > self.token_budget {
                    continue;
                }
                used += cost;
                kept.push(e);
            }
            on = kept;
        }
        on.sort_by_key(|e| e.order); // stable: equal orders stay in file order
        on
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn book(entries: Value) -> Lorebook {
        Lorebook::from_card_data(&json!({ "character_book": { "entries": entries } })).unwrap()
    }

    fn msgs(m: &[&str]) -> Vec<String> {
        m.iter().map(|s| s.to_string()).collect()
    }

    fn texts(b: &Lorebook, m: &[&str]) -> Vec<String> {
        b.activate(&msgs(m), 3.2).iter().map(|e| e.content.clone()).collect()
    }

    #[test]
    fn a_card_without_a_book_has_none() {
        assert!(Lorebook::from_card_data(&json!({})).is_none());
        assert!(Lorebook::from_card_data(&json!({ "character_book": { "entries": [] } })).is_none());
        // Only entries that are off or empty: nothing to keep.
        let off = json!([{ "keys": ["a"], "content": "x", "enabled": false },
                         { "keys": ["a"], "content": "  " }]);
        assert!(Lorebook::from_card_data(&json!({ "character_book": { "entries": off } })).is_none());
    }

    #[test]
    fn a_key_in_the_messages_turns_an_entry_on() {
        let b = book(json!([{ "keys": ["dragon"], "content": "Dragons hoard." }]));
        assert_eq!(texts(&b, &["We saw a Dragon today"]), vec!["Dragons hoard."]);
        assert!(texts(&b, &["Nothing here"]).is_empty());
    }

    #[test]
    fn case_matters_only_when_asked() {
        let b = book(json!([{ "keys": ["Rome"], "content": "r", "case_sensitive": true }]));
        assert!(texts(&b, &["rome"]).is_empty());
        assert_eq!(texts(&b, &["Rome"]).len(), 1);
    }

    #[test]
    fn constant_entries_are_always_on() {
        let b = book(json!([{ "keys": [], "content": "Always.", "constant": true }]));
        assert_eq!(texts(&b, &["hi"]), vec!["Always."]);
        assert_eq!(texts(&b, &[]), vec!["Always."]);
    }

    #[test]
    fn a_selective_entry_needs_a_secondary_key_too() {
        let b = book(json!([{ "keys": ["king"], "secondary_keys": ["crown"],
                               "selective": true, "content": "c" }]));
        assert!(texts(&b, &["the king"]).is_empty());
        assert_eq!(texts(&b, &["the king lost his crown"]).len(), 1);
        // Not selective: the secondary keys are not looked at.
        let b = book(json!([{ "keys": ["king"], "secondary_keys": ["crown"], "content": "c" }]));
        assert_eq!(texts(&b, &["the king"]).len(), 1);
    }

    #[test]
    fn secondary_logic_decides_how_the_secondary_keys_count() {
        let with = |logic: i64| {
            book(json!([{ "keys": ["king"], "secondary_keys": ["crown", "throne"],
                          "selective": true, "content": "c",
                          "extensions": { "selectiveLogic": logic } }]))
        };
        let both = ["the king: crown and throne"];
        let one = ["the king: crown"];
        let none = ["the king"];
        // 0: any one of them
        assert_eq!((texts(&with(0), &both).len(), texts(&with(0), &one).len(), texts(&with(0), &none).len()), (1, 1, 0));
        // 3: every one of them
        assert_eq!((texts(&with(3), &both).len(), texts(&with(3), &one).len(), texts(&with(3), &none).len()), (1, 0, 0));
        // 2: none of them may appear
        assert_eq!((texts(&with(2), &both).len(), texts(&with(2), &one).len(), texts(&with(2), &none).len()), (0, 0, 1));
        // 1: blocked only when all of them appear
        assert_eq!((texts(&with(1), &both).len(), texts(&with(1), &one).len(), texts(&with(1), &none).len()), (0, 1, 1));
    }

    #[test]
    fn only_the_latest_messages_are_searched() {
        let b = book(json!([{ "keys": ["old"], "content": "o" }]));
        let m = ["old news", "a", "b", "c", "d"];
        assert!(texts(&b, &m).is_empty()); // default depth 4
        assert_eq!(texts(&b, &m[..4]).len(), 1);
        let b = Lorebook::from_card_data(&json!({ "character_book": { "scan_depth": 1,
            "entries": [{ "keys": ["x"], "content": "c" }] } }))
        .unwrap();
        assert!(texts(&b, &["x", "y"]).is_empty());
        assert_eq!(texts(&b, &["y", "x"]).len(), 1);
    }

    #[test]
    fn entries_come_out_by_insertion_order() {
        let b = book(json!([
            { "keys": ["a"], "content": "second", "insertion_order": 20 },
            { "keys": ["a"], "content": "first", "insertion_order": 10 },
            { "keys": ["a"], "content": "third", "insertion_order": 20 },
        ]));
        assert_eq!(texts(&b, &["a"]), vec!["first", "second", "third"]);
    }

    #[test]
    fn positions_follow_the_card() {
        let b = book(json!([
            { "keys": ["a"], "content": "v2 after", "position": "after_char" },
            { "keys": ["a"], "content": "v2 before", "position": "before_char" },
            { "keys": ["a"], "content": "st depth", "position": "before_char",
              "extensions": { "position": 4, "depth": 2, "role": 1 } },
            { "keys": ["a"], "content": "st after", "extensions": { "position": 1 } },
        ]));
        let on = b.activate(&msgs(&["a"]), 3.2);
        let place = |c: &str| on.iter().find(|e| e.content == c).unwrap().place;
        assert_eq!(place("v2 after"), Place::After);
        assert_eq!(place("v2 before"), Place::Before);
        assert_eq!(place("st depth"), Place::Depth { depth: 2, role: 1 });
        assert_eq!(place("st after"), Place::After);
    }

    #[test]
    fn the_budget_keeps_the_highest_orders() {
        // 32 chars at 3.2 chars/token = about 10 tokens each; room for two.
        let text = "x".repeat(32);
        let b = Lorebook::from_card_data(&json!({ "character_book": { "token_budget": 25,
            "entries": [
                { "keys": ["a"], "content": text, "insertion_order": 1 },
                { "keys": ["a"], "content": text, "insertion_order": 3 },
                { "keys": ["a"], "content": text, "insertion_order": 2 },
            ] } }))
        .unwrap();
        let on = b.activate(&msgs(&["a"]), 3.2);
        let orders: Vec<i64> = on.iter().map(|e| e.order).collect();
        assert_eq!(orders, vec![2, 3]);
    }

    #[test]
    fn an_entry_that_does_not_fit_does_not_stop_the_smaller_ones() {
        // ~10 tokens for 32 chars, ~3 for 8 chars; budget 16.
        let big = "x".repeat(32);
        let small = "y".repeat(8);
        let b = Lorebook::from_card_data(&json!({ "character_book": { "token_budget": 16,
            "entries": [
                { "keys": ["a"], "content": small, "insertion_order": 1 },
                { "keys": ["a"], "content": big, "insertion_order": 9 },
                { "keys": ["a"], "content": big, "insertion_order": 8 },
                { "keys": ["a"], "content": small, "insertion_order": 2 },
            ] } }))
        .unwrap();
        let on = b.activate(&msgs(&["a"]), 3.2);
        // The first big one (order 9) takes ~10, the second big one no longer
        // fits and is skipped, both small ones (~3 each) still do: 10+3+3.
        let orders: Vec<i64> = on.iter().map(|e| e.order).collect();
        assert_eq!(orders, vec![1, 2, 9]);
    }

    #[test]
    fn an_entry_bigger_than_the_whole_budget_is_skipped_not_fatal() {
        let big = "x".repeat(64); // ~20 tokens
        let small = "y".repeat(8); // ~3 tokens
        let b = Lorebook::from_card_data(&json!({ "character_book": { "token_budget": 10,
            "entries": [
                { "keys": ["a"], "content": big, "insertion_order": 50 },
                { "keys": ["a"], "content": small, "insertion_order": 1 },
            ] } }))
        .unwrap();
        let on = b.activate(&msgs(&["a"]), 3.2);
        assert_eq!(on.len(), 1);
        assert_eq!(on[0].order, 1);
    }

    #[test]
    fn keys_may_be_a_comma_list() {
        let b = book(json!([{ "keys": "elf, dwarf", "content": "c" }]));
        assert_eq!(texts(&b, &["a dwarf"]).len(), 1);
    }
}
