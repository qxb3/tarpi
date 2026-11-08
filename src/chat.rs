//! Chat file model. Messages are kept as raw JSON so fields we don't know
//! about (written by the web UI or extensions) survive a load/save round trip.

use crate::jv::*;
use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};
use uuid::Uuid;

pub struct ChatState {
    pub avatar: String,
    pub char_name: String,
    /// File name without the `.jsonl` extension.
    pub file: String,
    pub header: Value,
    pub messages: Vec<Value>,
}

pub fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// A chat file name that is safe on every filesystem.
pub fn safe_name(char_name: &str) -> String {
    let safe: String = char_name
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    safe.trim().to_string()
}

/// Next free incremental chat name: `Sofia01`, `Sofia02`, ...
pub fn next_chat_name(char_name: &str, existing: &[String]) -> String {
    let safe = safe_name(char_name);
    let lower = safe.to_lowercase();
    let max = existing
        .iter()
        .filter_map(|id| {
            let l = id.to_lowercase();
            let rest = l.strip_prefix(lower.as_str())?;
            if !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()) {
                rest.parse::<u64>().ok()
            } else {
                None
            }
        })
        .max()
        .unwrap_or(0);
    format!("{}{:02}", safe, max + 1)
}

/// File name (without `.jsonl`) of an entry from `/api/characters/chats`.
pub fn file_id(c: &Value) -> String {
    let id = jstr(c, "file_id");
    if !id.is_empty() {
        return id;
    }
    jstr(c, "file_name").trim_end_matches(".jsonl").to_string()
}

fn new_header() -> Value {
    json!({
        "chat_metadata": { "integrity": Uuid::new_v4().to_string() },
        "user_name": "unused",
        "character_name": "unused",
    })
}

impl ChatState {
    pub fn new_empty(avatar: &str, char_name: &str, file: &str) -> ChatState {
        ChatState {
            avatar: avatar.to_string(),
            char_name: char_name.to_string(),
            file: file.to_string(),
            header: new_header(),
            messages: Vec::new(),
        }
    }

    pub fn from_lines(avatar: &str, char_name: &str, file: &str, lines: Vec<Value>) -> ChatState {
        let mut header: Option<Value> = None;
        let mut messages: Vec<Value> = Vec::new();
        for (idx, v) in lines.into_iter().enumerate() {
            if v.get("mes").is_some() {
                messages.push(v);
            } else if idx == 0 {
                header = Some(v);
            }
        }
        let mut header = header.unwrap_or_else(new_header);
        if header.get("chat_metadata").is_none() {
            header["chat_metadata"] = json!({});
        }
        ChatState {
            avatar: avatar.to_string(),
            char_name: char_name.to_string(),
            file: file.to_string(),
            header,
            messages,
        }
    }

    /// Header followed by messages, ready for `/api/chats/save`.
    pub fn lines(&self) -> Vec<Value> {
        let mut out = Vec::with_capacity(self.messages.len() + 1);
        out.push(self.header.clone());
        out.extend(self.messages.iter().cloned());
        out
    }

    /// The same lines with a fresh integrity id, to save the chat as a second
    /// file (a copy must not claim to be the file it was made from).
    pub fn copy_lines(&self) -> Vec<Value> {
        let mut out = self.lines();
        if let Some(meta) = out.first_mut().and_then(|h| h.get_mut("chat_metadata")) {
            if meta.is_object() {
                meta["integrity"] = json!(Uuid::new_v4().to_string());
            }
        }
        out
    }
}

// ---------------------------------------------------------------- accessors

pub fn mes(m: &Value) -> &str {
    m.get("mes").and_then(|x| x.as_str()).unwrap_or("")
}

pub fn is_user(m: &Value) -> bool {
    jbool(m, "is_user", false)
}

pub fn is_system(m: &Value) -> bool {
    jbool(m, "is_system", false)
}

pub fn name(m: &Value) -> String {
    jstr(m, "name")
}

/// `(current, total)` swipe position, 0-based current. `None` when the
/// message has no swipes.
pub fn swipe_pos(m: &Value) -> Option<(usize, usize)> {
    let total = m.get("swipes")?.as_array()?.len();
    if total == 0 {
        return None;
    }
    let cur = ji64(m, "swipe_id", 0).max(0) as usize;
    Some((cur.min(total - 1), total))
}

// ------------------------------------------------------------- constructors

pub fn user_message(name: &str, text: &str) -> Value {
    json!({
        "name": name,
        "is_user": true,
        "is_system": false,
        "send_date": now_iso(),
        "mes": text,
        "extra": {},
    })
}

pub fn assistant_message(
    name: &str,
    text: &str,
    model: &str,
    started: &str,
    finished: &str,
) -> Value {
    let extra = json!({ "api": "openai", "model": model });
    json!({
        "name": name,
        "is_user": false,
        "is_system": false,
        "send_date": started,
        "gen_started": started,
        "gen_finished": finished,
        "mes": text,
        "extra": extra,
        "swipe_id": 0,
        "swipes": [text],
        "swipe_info": [{
            "send_date": started,
            "gen_started": started,
            "gen_finished": finished,
            "extra": extra,
        }],
    })
}

/// First message of a new chat. Alternate greetings become swipes, like the web UI.
pub fn greeting_message(name: &str, greetings: &[String]) -> Value {
    let now = now_iso();
    let first = greetings.first().cloned().unwrap_or_default();
    let infos: Vec<Value> = greetings
        .iter()
        .map(|_| {
            json!({
                "send_date": now,
                "gen_started": now,
                "gen_finished": now,
                "extra": {},
            })
        })
        .collect();
    json!({
        "name": name,
        "is_user": false,
        "is_system": false,
        "send_date": now,
        "mes": first,
        "extra": {},
        "swipe_id": 0,
        "swipes": greetings,
        "swipe_info": infos,
    })
}

// ------------------------------------------------------------------- swipes

fn ensure_swipes(m: &mut Value) {
    if m.get("swipes").and_then(|s| s.as_array()).is_none() {
        let text = mes(m).to_string();
        m["swipes"] = json!([text]);
        m["swipe_id"] = json!(0);
    }
    if m.get("swipe_info").and_then(|s| s.as_array()).is_none() {
        m["swipe_info"] = json!([]);
    }
}

/// Add greetings as extra swipes of a message without changing what it shows
/// (used to give an older chat the card's alternate greetings).
pub fn add_greeting_swipes(m: &mut Value, extra: &[String]) {
    ensure_swipes(m);
    let now = now_iso();
    let sent = m.get("send_date").cloned().unwrap_or_else(|| json!(now));
    let have = m["swipes"].as_array().map(|a| a.len()).unwrap_or(0);
    let info = |date: &Value| {
        json!({
            "send_date": date,
            "gen_started": date,
            "gen_finished": date,
            "extra": {},
        })
    };
    // Keep swipe_info lined up with swipes.
    if let Some(a) = m["swipe_info"].as_array_mut() {
        while a.len() < have {
            a.push(info(&sent));
        }
    }
    let stamp = json!(now);
    for t in extra {
        if let Some(a) = m["swipes"].as_array_mut() {
            a.push(json!(t));
        }
        if let Some(a) = m["swipe_info"].as_array_mut() {
            a.push(info(&stamp));
        }
    }
}

/// Make swipe `idx` the visible one (copies its text and metadata into the message).
pub fn set_swipe(m: &mut Value, idx: usize) {
    let text = m
        .get("swipes")
        .and_then(|s| s.as_array())
        .and_then(|a| a.get(idx))
        .and_then(|t| t.as_str())
        .map(|t| t.to_string());
    let Some(text) = text else { return };
    m["mes"] = json!(text);
    m["swipe_id"] = json!(idx);
    let info = m
        .get("swipe_info")
        .and_then(|a| a.as_array())
        .and_then(|a| a.get(idx))
        .cloned();
    if let Some(info) = info {
        for key in ["send_date", "gen_started", "gen_finished", "extra"] {
            if let Some(v) = info.get(key) {
                m[key] = v.clone();
            }
        }
    }
}

/// Append a freshly generated swipe and show it.
pub fn add_swipe(m: &mut Value, text: &str, model: &str, started: &str, finished: &str) {
    ensure_swipes(m);
    if let Some(a) = m["swipes"].as_array_mut() {
        a.push(json!(text));
    }
    let n = m["swipes"].as_array().map(|a| a.len()).unwrap_or(1);
    let filler = json!({
        "send_date": started,
        "gen_started": started,
        "gen_finished": started,
        "extra": {},
    });
    let info = json!({
        "send_date": started,
        "gen_started": started,
        "gen_finished": finished,
        "extra": { "api": "openai", "model": model },
    });
    if let Some(a) = m["swipe_info"].as_array_mut() {
        while a.len() + 1 < n {
            a.push(filler.clone());
        }
        a.push(info);
    }
    set_swipe(m, n - 1);
}

/// Replace a message with a freshly generated reply and drop every swipe it
/// had: afterwards it has exactly one swipe (what /regen and Ctrl+R do).
pub fn reset_swipes(m: &mut Value, text: &str, model: &str, started: &str, finished: &str) {
    let extra = json!({ "api": "openai", "model": model });
    m["mes"] = json!(text);
    m["send_date"] = json!(started);
    m["gen_started"] = json!(started);
    m["gen_finished"] = json!(finished);
    m["extra"] = extra.clone();
    m["swipe_id"] = json!(0);
    m["swipes"] = json!([text]);
    m["swipe_info"] = json!([{
        "send_date": started,
        "gen_started": started,
        "gen_finished": finished,
        "extra": extra,
    }]);
}

// ----------------------------------------------------------------- branches

/// Field on a chat's first message that makes the chat branched (see
/// `with_summary_branch`). Unknown fields survive a save / load round trip, so
/// the web UI simply ignores it. Shape: `{ "active": n, "branches": [slot, ..] }`
/// where a slot is `{ "first": message, "tail": [messages] }` and the slot of
/// the active branch is `null` (its messages are the visible ones).
pub const BRANCH_KEY: &str = "st_cli_branch";

/// `(active, total)` branch position, 0-based active, `None` when the chat
/// has no branches. Branch 0 is the actual conversation, every other branch a
/// `/summarize` summary, oldest first.
pub fn branch_pos(first: &Value) -> Option<(usize, usize)> {
    let b = first.get(BRANCH_KEY)?;
    if let Some(arr) = b.get("branches").and_then(|x| x.as_array()) {
        if arr.len() < 2 {
            return None;
        }
        let active = (ji64(b, "active", 0).max(0) as usize).min(arr.len() - 1);
        let ok = arr
            .iter()
            .enumerate()
            .all(|(i, s)| i == active || s.get("first").is_some_and(|f| f.is_object()));
        return if ok { Some((active, arr.len())) } else { None };
    }
    // Older format: just two branches, `{ "active": n, "other": slot }`.
    b.get("other")?.get("first")?.as_object()?;
    Some((if ji64(b, "active", 0) == 1 { 1 } else { 0 }, 2))
}

/// A summary branch is showing (not the actual conversation).
pub fn is_summary_branch(first: &Value) -> bool {
    matches!(branch_pos(first), Some((a, _)) if a >= 1)
}

/// The active branch and the slots of all branches (the active one is `null`).
fn take_slots(first: &Value) -> Option<(usize, Vec<Value>)> {
    let (active, _) = branch_pos(first)?;
    let b = first.get(BRANCH_KEY)?;
    if let Some(arr) = b.get("branches").and_then(|x| x.as_array()) {
        return Some((active, arr.clone()));
    }
    let mut slots = vec![Value::Null, Value::Null];
    slots[1 - active] = b.get("other")?.clone();
    Some((active, slots))
}

/// Remove the branch field from a first message.
fn strip_branch(m: &mut Value) {
    if let Some(o) = m.as_object_mut() {
        o.remove(BRANCH_KEY);
    }
}

/// Show the next (`dir` > 0) or previous branch, wrapping around: the visible
/// messages are stored in the branch they belong to and the other branch's
/// messages are shown. Returns false (and changes nothing) for a chat without
/// branches.
pub fn switch_branch(messages: &mut Vec<Value>, dir: i32) -> bool {
    let Some(first) = messages.first() else {
        return false;
    };
    let Some((active, mut slots)) = take_slots(first) else {
        return false;
    };
    let total = slots.len();
    let target = if dir < 0 {
        (active + total - 1) % total
    } else {
        (active + 1) % total
    };
    if target == active {
        return false;
    }
    let mut cur_first = messages[0].clone();
    strip_branch(&mut cur_first);
    let cur_tail: Vec<Value> = messages.drain(1..).collect();
    slots[active] = json!({ "first": cur_first, "tail": cur_tail });
    let slot = std::mem::replace(&mut slots[target], Value::Null);
    let mut next_first = slot.get("first").cloned().unwrap_or(Value::Null);
    let next_tail: Vec<Value> = slot
        .get("tail")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();
    strip_branch(&mut next_first);
    next_first[BRANCH_KEY] = json!({ "active": target, "branches": slots });
    let mut out = vec![next_first];
    out.extend(next_tail);
    *messages = out;
    true
}

/// Delete the branch that is showing and show the one before it (or, for the
/// first, the one after it). Returns the branch now showing and how many
/// branches are left as `(active, total)`; with one left the chat is no
/// longer branched. `None` (nothing changed) for a chat without branches.
pub fn delete_branch(messages: &mut Vec<Value>) -> Option<(usize, usize)> {
    let (active, mut slots) = take_slots(messages.first()?)?;
    slots.remove(active);
    let target = active.saturating_sub(1).min(slots.len() - 1);
    let slot = std::mem::replace(&mut slots[target], Value::Null);
    let mut next_first = slot.get("first").cloned().unwrap_or(Value::Null);
    let next_tail: Vec<Value> = slot
        .get("tail")
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default();
    strip_branch(&mut next_first);
    let total = slots.len();
    if total >= 2 {
        next_first[BRANCH_KEY] = json!({ "active": target, "branches": slots });
    }
    let mut out = vec![next_first];
    out.extend(next_tail);
    *messages = out;
    Some((target, total))
}

/// The messages of a chat after `/summarize`: a new branch is added after
/// the existing ones and shown, holding just one message, the summary. Every
/// branch that was there (the conversation, earlier summaries, and what was
/// said after them) stays whole. The summary is of the branch that was
/// showing.
pub fn with_summary_branch(
    messages: &[Value],
    name: &str,
    summary: &str,
    model: &str,
    started: &str,
    finished: &str,
) -> Vec<Value> {
    let Some(first) = messages.first() else {
        return Vec::new();
    };
    let (active, mut slots) = take_slots(first).unwrap_or((0, vec![Value::Null]));
    let mut cur_first = first.clone();
    strip_branch(&mut cur_first);
    slots[active] = json!({ "first": cur_first, "tail": messages[1..].to_vec() });
    slots.push(Value::Null);
    let new_active = slots.len() - 1;

    // Name, avatar and the like come from the conversation's first message.
    let template = slots[0]
        .get("first")
        .filter(|m| !is_user(m) && !is_system(m))
        .cloned();
    let info = json!({
        "send_date": started,
        "gen_started": started,
        "gen_finished": finished,
        "extra": { "api": "openai", "model": model },
    });
    let mut m = template.unwrap_or_else(|| {
        json!({
            "name": name,
            "is_user": false,
            "is_system": false,
        })
    });
    m["send_date"] = json!(started);
    m["gen_started"] = json!(started);
    m["gen_finished"] = json!(finished);
    m["extra"] = info["extra"].clone();
    m["mes"] = json!(summary);
    m["swipe_id"] = json!(0);
    m["swipes"] = json!([summary]);
    m["swipe_info"] = json!([info]);
    m[BRANCH_KEY] = json!({ "active": new_active, "branches": slots });
    vec![m]
}

// ------------------------------------------------------------------ editing

/// Tidy text before it is stored: no `\r`, no trailing spaces on any line, and
/// no blank space or empty lines at the start or end.
pub fn tidy(text: &str) -> String {
    let s = text.replace('\r', "");
    let lines: Vec<&str> = s.split('\n').map(|l| l.trim_end()).collect();
    lines.join("\n").trim().to_string()
}

/// Replace the text of a message (and of the swipe it is showing).
pub fn set_text(m: &mut Value, text: &str) {
    let id = ji64(m, "swipe_id", 0).max(0) as usize;
    m["mes"] = json!(text);
    if let Some(slot) = m
        .get_mut("swipes")
        .and_then(|s| s.as_array_mut())
        .and_then(|a| a.get_mut(id))
    {
        *slot = json!(text);
    }
}

/// Record when a message (and its current swipe) was last generated into.
pub fn touch_finished(m: &mut Value, finished: &str) {
    let id = ji64(m, "swipe_id", 0).max(0) as usize;
    m["gen_finished"] = json!(finished);
    if let Some(info) = m
        .get_mut("swipe_info")
        .and_then(|s| s.as_array_mut())
        .and_then(|a| a.get_mut(id))
    {
        info["gen_finished"] = json!(finished);
    }
}

/// Glue a continuation onto the text it continues. A space is added only
/// when the old text ended a sentence/quote/emphasis and the new text starts
/// right against it; a reply cut off mid-word is joined as is.
pub fn join_continuation(orig: &str, chunk: &str) -> String {
    let chunk = chunk.trim_end();
    let needs_space = match (orig.chars().last(), chunk.chars().next()) {
        (Some(a), Some(b)) => {
            matches!(
                a,
                '.' | '!' | '?' | '"' | '\u{201D}' | '*' | ')' | '\u{2026}' | '~'
            ) && !b.is_whitespace()
                && !matches!(
                    b,
                    '.' | ',' | '!' | '?' | ';' | ':' | ')' | ']' | '\u{2026}'
                )
        }
        _ => false,
    };
    if needs_space {
        format!("{} {}", orig, chunk)
    } else {
        format!("{}{}", orig, chunk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convo() -> Vec<Value> {
        vec![
            greeting_message("Sofia", &["Hello".to_string(), "Hi there".to_string()]),
            user_message("Me", "one"),
            assistant_message("Sofia", "two", "m", "t0", "t1"),
            user_message("Me", "three"),
        ]
    }

    #[test]
    fn summary_branch_keeps_the_whole_conversation() {
        let mut v = with_summary_branch(&convo(), "Sofia", "THE SUMMARY", "m", "t0", "t1");
        assert_eq!(v.len(), 1);
        assert_eq!(mes(&v[0]), "THE SUMMARY");
        assert_eq!(branch_pos(&v[0]), Some((1, 2)));
        assert!(is_summary_branch(&v[0]));
        assert_eq!(swipe_pos(&v[0]), Some((0, 1)));

        // Branch 0 is the conversation as it was, greeting swipes included.
        assert!(switch_branch(&mut v, -1));
        assert_eq!(v.len(), 4);
        assert_eq!(branch_pos(&v[0]), Some((0, 2)));
        assert!(!is_summary_branch(&v[0]));
        assert_eq!(mes(&v[0]), "Hello");
        assert_eq!(swipe_pos(&v[0]), Some((0, 2)));
        assert_eq!(mes(&v[3]), "three");

        // And on to the summary again (wraps around).
        assert!(switch_branch(&mut v, 1));
        assert_eq!(v.len(), 1);
        assert_eq!(mes(&v[0]), "THE SUMMARY");
    }

    #[test]
    fn each_branch_keeps_its_own_messages() {
        let mut v = with_summary_branch(&convo(), "Sofia", "S", "m", "t0", "t1");
        v.push(user_message("Me", "after the summary"));
        assert!(switch_branch(&mut v, 1));
        assert_eq!(v.len(), 4);
        assert!(switch_branch(&mut v, 1));
        assert_eq!(v.len(), 2);
        assert_eq!(mes(&v[1]), "after the summary");
    }

    #[test]
    fn every_summarize_adds_a_branch() {
        // From the conversation, then from the first summary, then from the conversation again.
        let v1 = with_summary_branch(&convo(), "Sofia", "FIRST", "m", "t0", "t1");
        assert_eq!(branch_pos(&v1[0]), Some((1, 2)));
        let mut v2 = v1.clone();
        v2.push(user_message("Me", "more"));
        let v2 = with_summary_branch(&v2, "Sofia", "SECOND", "m", "t2", "t3");
        assert_eq!(v2.len(), 1);
        assert_eq!(mes(&v2[0]), "SECOND");
        assert_eq!(branch_pos(&v2[0]), Some((2, 3)));

        let mut v = v2;
        assert!(switch_branch(&mut v, -1));
        assert_eq!((mes(&v[0]), v.len()), ("FIRST", 2));
        assert_eq!(branch_pos(&v[0]), Some((1, 3)));
        assert!(switch_branch(&mut v, -1));
        assert_eq!((mes(&v[0]), v.len()), ("Hello", 4));

        // Summarizing the conversation again adds a fourth branch; nothing is lost.
        let mut v = with_summary_branch(&v, "Sofia", "THIRD", "m", "t4", "t5");
        assert_eq!(branch_pos(&v[0]), Some((3, 4)));
        assert!(switch_branch(&mut v, 1)); // wraps to the conversation
        assert_eq!(v.len(), 4);
        assert!(switch_branch(&mut v, 1)); // first summary
        assert_eq!((mes(&v[0]), v.len()), ("FIRST", 2));
        assert_eq!(mes(&v[1]), "more");
        assert!(switch_branch(&mut v, 1)); // second summary
        assert_eq!(mes(&v[0]), "SECOND");
    }

    #[test]
    fn the_older_two_branch_format_still_loads() {
        let full = convo();
        let mut old = with_summary_branch(&full, "Sofia", "S", "m", "t0", "t1");
        let other = {
            let b = &old[0][BRANCH_KEY]["branches"][0];
            b.clone()
        };
        old[0][BRANCH_KEY] = json!({ "active": 1, "other": other });
        assert_eq!(branch_pos(&old[0]), Some((1, 2)));
        assert!(switch_branch(&mut old, 1));
        assert_eq!(old.len(), 4);
        assert_eq!(branch_pos(&old[0]), Some((0, 2)));
    }

    #[test]
    fn deleting_a_branch_shows_the_previous_one() {
        let v1 = with_summary_branch(&convo(), "Sofia", "FIRST", "m", "t0", "t1");
        let mut v = with_summary_branch(&v1, "Sofia", "SECOND", "m", "t2", "t3");
        assert_eq!(branch_pos(&v[0]), Some((2, 3)));
        assert_eq!(delete_branch(&mut v), Some((1, 2)));
        assert_eq!(mes(&v[0]), "FIRST");
        assert_eq!(branch_pos(&v[0]), Some((1, 2)));
        // One branch left: the chat is a plain chat again.
        assert_eq!(delete_branch(&mut v), Some((0, 1)));
        assert_eq!(v.len(), 4);
        assert_eq!(branch_pos(&v[0]), None);
        assert!(v[0].get(BRANCH_KEY).is_none());
        assert_eq!(delete_branch(&mut v), None);
        assert_eq!(v.len(), 4);
    }

    #[test]
    fn no_branches_no_switch() {
        let mut v = convo();
        assert!(!switch_branch(&mut v, 1));
        assert_eq!(v.len(), 4);
        assert_eq!(branch_pos(&v[0]), None);
    }
}
