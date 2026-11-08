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
