//! Builds the chat-completion prompt the same way SillyTavern's frontend does
//! (prompt manager order, character card fields, persona, depth injections,
//! example dialogue, history trimming). The server only forwards what we send,
//! so this is where the "character" actually gets applied.
//!
//! Not supported yet: World Info / lorebooks, Author's Note, summaries,
//! vector memory, group chats.

use crate::character::Character;
use crate::chat;
use crate::jv::*;
use crate::textgen::replace_ci;
use chrono::Local;
use serde::Serialize;
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize)]
pub struct ChatMsg {
    pub role: String,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMsg {
    fn new(role: &str, content: String) -> ChatMsg {
        ChatMsg {
            role: role.to_string(),
            content,
            name: None,
        }
    }

    fn named(role: &str, content: String, name: &str) -> ChatMsg {
        ChatMsg {
            role: role.to_string(),
            content,
            name: Some(name.to_string()),
        }
    }
}

// ------------------------------------------------------------------ macros

#[derive(Default)]
pub struct Ctx {
    pub user: String,
    pub char_name: String,
    pub description: String,
    pub personality: String,
    pub scenario: String,
    pub persona: String,
    pub mes_examples: String,
    pub last_message: String,
    pub last_user_message: String,
    pub last_char_message: String,
    pub original: String,
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len()
        && s.is_char_boundary(prefix.len())
        && s[..prefix.len()].eq_ignore_ascii_case(prefix)
    {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

fn rand_below(n: usize) -> usize {
    use std::sync::atomic::{AtomicU64, Ordering};
    static STATE: AtomicU64 = AtomicU64::new(0);
    if n == 0 {
        return 0;
    }
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as u64;
    // xorshift mixed with the clock, so several rolls in one message differ.
    let mut x = STATE.load(Ordering::Relaxed) ^ nanos ^ 0x9E37_79B9_7F4A_7C15;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    STATE.store(x, Ordering::Relaxed);
    (x % n as u64) as usize
}

/// `{{roll:1d6}}`, `{{roll:d20}}`, `{{roll:20}}`, with an optional `+N` / `-N`.
fn roll_dice(formula: &str) -> Option<String> {
    let f: String = formula.chars().filter(|c| !c.is_whitespace()).collect();
    let f = f.to_lowercase();
    let (body, modifier) = match f.find(|c| c == '+' || c == '-') {
        Some(i) if i > 0 => (&f[..i], f[i..].parse::<i64>().ok()?),
        _ => (f.as_str(), 0),
    };
    let (count, sides) = match body.split_once('d') {
        Some((c, s)) => (
            if c.is_empty() { 1 } else { c.parse::<usize>().ok()? },
            s.parse::<usize>().ok()?,
        ),
        None => (1, body.parse::<usize>().ok()?),
    };
    if count == 0 || count > 100 || sides == 0 {
        return None;
    }
    let mut total = modifier;
    for _ in 0..count {
        total += rand_below(sides) as i64 + 1;
    }
    Some(total.to_string())
}

/// `{{time_UTC+2}}`: the current time at a UTC offset.
fn utc_time(offset: &str) -> Option<String> {
    let hours: i64 = if offset.trim().is_empty() {
        0
    } else {
        offset.trim().parse().ok()?
    };
    if hours.abs() > 14 {
        return None;
    }
    let t = chrono::Utc::now() + chrono::Duration::hours(hours);
    Some(t.format("%-I:%M %p").to_string())
}

const TRIM_MARK: &str = "\u{1}TRIM\u{1}";

fn expand_macro(inner: &str, c: &Ctx) -> Option<String> {
    let t = inner.trim();
    if t.starts_with("//") {
        return Some(String::new());
    }
    if let Some(rest) = strip_prefix_ci(t, "random:") {
        let items: Vec<&str> = if let Some(r) = rest.strip_prefix(':') {
            r.split("::").collect()
        } else {
            rest.split(',').collect()
        };
        let items: Vec<&str> = items
            .into_iter()
            .map(|x| x.trim())
            .filter(|x| !x.is_empty())
            .collect();
        if items.is_empty() {
            return Some(String::new());
        }
        return Some(items[rand_below(items.len())].to_string());
    }
    if let Some(rest) = strip_prefix_ci(t, "roll:") {
        return roll_dice(rest);
    }
    if let Some(rest) = strip_prefix_ci(t, "reverse:") {
        return Some(rest.chars().rev().collect());
    }
    if let Some(rest) = strip_prefix_ci(t, "time_utc") {
        return utc_time(rest);
    }
    if strip_prefix_ci(t, "banned ").is_some() {
        return Some(String::new());
    }
    let lower = t.to_lowercase();
    let now = Local::now();
    let out = match lower.as_str() {
        "noop" => String::new(),
        "user" => c.user.clone(),
        "char" | "bot" | "group" | "charifnotgroup" => c.char_name.clone(),
        "description" => c.description.clone(),
        "personality" => c.personality.clone(),
        "scenario" => c.scenario.clone(),
        "persona" => c.persona.clone(),
        "mesexamples" | "mesexamplesraw" => c.mes_examples.clone(),
        "original" => c.original.clone(),
        "lastmessage" => c.last_message.clone(),
        "lastusermessage" => c.last_user_message.clone(),
        "lastcharmessage" => c.last_char_message.clone(),
        "input" => String::new(),
        "newline" => "\n".to_string(),
        "trim" => TRIM_MARK.to_string(),
        "time" => now.format("%-I:%M %p").to_string(),
        "date" => now.format("%B %-d, %Y").to_string(),
        "weekday" => now.format("%A").to_string(),
        "isotime" => now.format("%H:%M").to_string(),
        "isodate" => now.format("%Y-%m-%d").to_string(),
        _ => return None,
    };
    Some(out)
}

fn apply_trim(s: String) -> String {
    if !s.contains(TRIM_MARK) {
        return s;
    }
    let parts: Vec<&str> = s.split(TRIM_MARK).collect();
    let mut out = String::new();
    for (i, part) in parts.iter().enumerate() {
        let mut seg: &str = part;
        if i > 0 {
            seg = seg.trim_start_matches('\n');
        }
        if i + 1 < parts.len() {
            seg = seg.trim_end_matches('\n');
        }
        out.push_str(seg);
    }
    out
}

/// Replace `{{macro}}` placeholders. Unknown macros are left untouched.
pub fn subst(text: &str, c: &Ctx) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let inner = &after[..end];
                match expand_macro(inner, c) {
                    Some(rep) => out.push_str(&rep),
                    None => {
                        out.push_str("{{");
                        out.push_str(inner);
                        out.push_str("}}");
                    }
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    apply_trim(legacy_tags(out, &c.user, &c.char_name))
}

/// The old `<USER>`, `<BOT>`, `<CHAR>` and `<GROUP>` placeholders that many
/// cards still use. SillyTavern replaces them like `{{user}}` / `{{char}}`.
fn legacy_tags(text: String, user: &str, char_name: &str) -> String {
    if !text.contains('<') {
        return text;
    }
    let mut out = text;
    if !user.is_empty() {
        out = replace_ci(&out, "<user>", user);
    }
    if !char_name.is_empty() {
        for tag in ["<bot>", "<char>", "<group>"] {
            out = replace_ci(&out, tag, char_name);
        }
    }
    out
}

/// Replace only the name placeholders (`{{user}}`, `{{char}}`, `<USER>`, ...).
/// Unlike `subst` it is deterministic (no dice, no clock), so it is safe to
/// run on every redraw of the chat.
pub fn subst_display(text: &str, user: &str, char_name: &str) -> String {
    if !text.contains("{{") && !text.contains('<') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("}}") {
            Some(end) => {
                let inner = &after[..end];
                match inner.trim().to_lowercase().as_str() {
                    "user" => out.push_str(user),
                    "char" | "bot" | "group" | "charifnotgroup" => out.push_str(char_name),
                    _ => {
                        out.push_str("{{");
                        out.push_str(inner);
                        out.push_str("}}");
                    }
                }
                rest = &after[end + 2..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    legacy_tags(out, user, char_name)
}

/// Full macro substitution for text typed by the user or taken from the card
/// (greetings): the card's own fields and the persona are available too, as in
/// SillyTavern.
pub fn subst_card(text: &str, user: &str, persona: &str, ch: &Character) -> String {
    let mut c = Ctx {
        user: user.to_string(),
        char_name: ch.name.clone(),
        ..Ctx::default()
    };
    c.description = subst(&ch.description, &c).trim().to_string();
    c.personality = subst(&ch.personality, &c).trim().to_string();
    c.scenario = subst(&ch.scenario, &c).trim().to_string();
    c.persona = subst(persona, &c).trim().to_string();
    c.mes_examples = subst(&ch.mes_example, &c);
    subst(text, &c)
}

/// Public wrapper for substituting `{{char}}` / `{{user}}` in greetings.
pub fn subst_names(text: &str, user: &str, char_name: &str) -> String {
    let c = Ctx {
        user: user.to_string(),
        char_name: char_name.to_string(),
        ..Ctx::default()
    };
    subst(text, &c)
}

// ---------------------------------------------------------------- examples

fn parse_examples(raw: &str, user: &str, char_name: &str) -> Vec<Vec<ChatMsg>> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }
    let text = if trimmed.to_ascii_uppercase().starts_with("<START>") {
        trimmed.to_string()
    } else {
        format!("<START>\n{}", trimmed)
    };
    // ASCII lowercase keeps byte offsets identical to `text`.
    let lower = text.to_ascii_lowercase();
    let mut blocks: Vec<&str> = Vec::new();
    let mut starts: Vec<usize> = lower.match_indices("<start>").map(|(i, _)| i).collect();
    starts.push(text.len());
    for w in starts.windows(2) {
        blocks.push(&text[w[0] + "<start>".len()..w[1]]);
    }

    let user_prefix = format!("{}:", user);
    let char_prefix = format!("{}:", char_name);
    let mut result: Vec<Vec<ChatMsg>> = Vec::new();
    for block in blocks {
        let mut msgs: Vec<ChatMsg> = Vec::new();
        for line in block.lines() {
            let l = line.trim_start();
            if let Some(rest) = l.strip_prefix(user_prefix.as_str()) {
                msgs.push(ChatMsg::named(
                    "system",
                    rest.trim().to_string(),
                    "example_user",
                ));
            } else if let Some(rest) = l.strip_prefix(char_prefix.as_str()) {
                msgs.push(ChatMsg::named(
                    "system",
                    rest.trim().to_string(),
                    "example_assistant",
                ));
            } else if let Some(last) = msgs.last_mut() {
                last.content.push('\n');
                last.content.push_str(line.trim_end());
            }
        }
        for m in msgs.iter_mut() {
            m.content = m.content.trim().to_string();
        }
        msgs.retain(|m| !m.content.is_empty());
        if !msgs.is_empty() {
            result.push(msgs);
        }
    }
    result
}

// ----------------------------------------------------------------- builder

pub struct BuildArgs<'a> {
    /// `oai_settings` from the user's settings.json.
    pub oai: &'a Value,
    /// `power_user` from the user's settings.json.
    pub power_user: &'a Value,
    pub ch: &'a Character,
    pub user_name: &'a str,
    pub persona: &'a str,
    pub persona_position: i64,
    /// Chat messages to include (already excluding anything being regenerated).
    pub history: &'a [Value],
    /// Characters per token used for the rough budget estimate.
    pub chars_per_token: f32,
    /// Continue the last (assistant) message instead of starting a new reply.
    pub continue_last: bool,
}

enum Slot {
    Msg(ChatMsg),
    History,
    Examples,
}

fn norm_role(r: &str) -> &'static str {
    match r {
        "user" => "user",
        "assistant" => "assistant",
        _ => "system",
    }
}

pub fn est_tokens(text: &str, cpt: f32) -> usize {
    let cpt = if cpt < 1.0 { 3.2 } else { cpt };
    ((text.chars().count() as f32) / cpt).ceil() as usize
}

fn est_msg(m: &ChatMsg, cpt: f32) -> usize {
    est_tokens(&m.content, cpt) + 4
}

fn sanitize_name(name: &str) -> Option<String> {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn default_prompts() -> Vec<Value> {
    vec![serde_json::json!({
        "identifier": "main",
        "role": "system",
        "content": "Write {{char}}'s next reply in a fictional chat between {{char}} and {{user}}.",
    })]
}

fn default_order() -> Vec<(String, bool)> {
    [
        "main",
        "worldInfoBefore",
        "personaDescription",
        "charDescription",
        "charPersonality",
        "scenario",
        "nsfw",
        "worldInfoAfter",
        "dialogueExamples",
        "chatHistory",
        "jailbreak",
    ]
    .iter()
    .map(|s| (s.to_string(), true))
    .collect()
}

/// Prompt order: the global entry (character_id 100001) like the web UI uses
/// when no per-character override exists.
fn pick_order(o: &Value) -> Vec<(String, bool)> {
    let arr: Vec<Value> = o
        .get("prompt_order")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let chosen = arr
        .iter()
        .find(|e| ji64(e, "character_id", 0) == 100001)
        .or_else(|| arr.iter().find(|e| ji64(e, "character_id", 0) == 100000))
        .or_else(|| arr.first());
    match chosen {
        Some(e) => {
            let list: Vec<(String, bool)> = e
                .get("order")
                .and_then(|x| x.as_array())
                .map(|list| {
                    list.iter()
                        .map(|it| (jstr(it, "identifier"), jbool(it, "enabled", true)))
                        .collect()
                })
                .unwrap_or_default();
            if list.is_empty() {
                default_order()
            } else {
                list
            }
        }
        None => default_order(),
    }
}

pub fn build_chat_completion(a: &BuildArgs) -> Vec<ChatMsg> {
    let o = a.oai;
    let cpt = a.chars_per_token;

    // ---- macro context (card fields are substituted once, up front)
    let mut ctx = Ctx {
        user: a.user_name.to_string(),
        char_name: a.ch.name.clone(),
        ..Ctx::default()
    };
    ctx.description = subst(&a.ch.description, &ctx).trim().to_string();
    ctx.personality = subst(&a.ch.personality, &ctx).trim().to_string();
    ctx.scenario = subst(&a.ch.scenario, &ctx).trim().to_string();
    ctx.persona = subst(a.persona, &ctx).trim().to_string();
    ctx.mes_examples = subst(&a.ch.mes_example, &ctx);
    for m in a.history.iter().rev() {
        if chat::is_system(m) {
            continue;
        }
        let text = chat::mes(m).to_string();
        if ctx.last_message.is_empty() {
            ctx.last_message = text.clone();
        }
        if chat::is_user(m) {
            if ctx.last_user_message.is_empty() {
                ctx.last_user_message = text;
            }
        } else if ctx.last_char_message.is_empty() {
            ctx.last_char_message = text;
        }
    }

    // ---- prompt definitions + order
    let prompts: Vec<Value> = match o.get("prompts").and_then(|p| p.as_array()) {
        Some(p) if !p.is_empty() => p.clone(),
        _ => default_prompts(),
    };
    let order = pick_order(o);
    let prefer_char_prompt = jbool(a.power_user, "prefer_character_prompt", true);
    let prefer_char_jb = jbool(a.power_user, "prefer_character_jailbreak", true);

    let personality_format = {
        let f = jstr(o, "personality_format");
        if f.is_empty() {
            "{{personality}}".to_string()
        } else {
            f
        }
    };
    let scenario_format = {
        let f = jstr(o, "scenario_format");
        if f.is_empty() {
            "{{scenario}}".to_string()
        } else {
            f
        }
    };

    let mut slots: Vec<Slot> = Vec::new();
    // (depth, sequence, message) — inserted into the chat history later.
    let mut injections: Vec<(usize, usize, ChatMsg)> = Vec::new();
    let mut seq = 0usize;

    for (id, enabled) in order.iter() {
        if !*enabled {
            continue;
        }
        match id.as_str() {
            "chatHistory" => slots.push(Slot::History),
            "dialogueExamples" => slots.push(Slot::Examples),
            "charDescription" => {
                if !ctx.description.is_empty() {
                    slots.push(Slot::Msg(ChatMsg::new("system", ctx.description.clone())));
                }
            }
            "charPersonality" => {
                if !ctx.personality.is_empty() {
                    let text = subst(&personality_format, &ctx);
                    if !text.trim().is_empty() {
                        slots.push(Slot::Msg(ChatMsg::new("system", text)));
                    }
                }
            }
            "scenario" => {
                if !ctx.scenario.is_empty() {
                    let text = subst(&scenario_format, &ctx);
                    if !text.trim().is_empty() {
                        slots.push(Slot::Msg(ChatMsg::new("system", text)));
                    }
                }
            }
            "personaDescription" => {
                if (a.persona_position == 0 || a.persona_position == 1) && !ctx.persona.is_empty() {
                    slots.push(Slot::Msg(ChatMsg::new("system", ctx.persona.clone())));
                }
            }
            other => {
                let Some(p) = prompts.iter().find(|p| jstr(p, "identifier") == other) else {
                    continue;
                };
                if jbool(p, "marker", false) {
                    // World info, author's note, summaries, ... (not supported).
                    continue;
                }
                let mut content = jstr(p, "content");
                let forbid = jbool(p, "forbid_overrides", false);
                if other == "main"
                    && prefer_char_prompt
                    && !forbid
                    && !a.ch.system_prompt.trim().is_empty()
                {
                    ctx.original = content.clone();
                    content = a.ch.system_prompt.clone();
                }
                if other == "jailbreak"
                    && prefer_char_jb
                    && !forbid
                    && !a.ch.post_history_instructions.trim().is_empty()
                {
                    ctx.original = content.clone();
                    content = a.ch.post_history_instructions.clone();
                }
                let text = subst(&content, &ctx);
                ctx.original.clear();
                if text.trim().is_empty() {
                    continue;
                }
                let role = norm_role(&jstr(p, "role"));
                if ji64(p, "injection_position", 0) == 1 {
                    let depth = ji64(p, "injection_depth", 4).max(0) as usize;
                    injections.push((depth, seq, ChatMsg::new(role, text)));
                    seq += 1;
                } else {
                    slots.push(Slot::Msg(ChatMsg::new(role, text)));
                }
            }
        }
    }

    // Character "depth prompt" (Character's Note) and persona-at-depth.
    if let Some(dp) = &a.ch.depth_prompt {
        let text = subst(&dp.prompt, &ctx);
        if !text.trim().is_empty() {
            injections.push((dp.depth, seq, ChatMsg::new(&dp.role, text)));
            seq += 1;
        }
    }
    if a.persona_position == 4 && !ctx.persona.is_empty() {
        let depth = ji64(a.power_user, "persona_description_depth", 2).max(0) as usize;
        let role = match ji64(a.power_user, "persona_description_role", 0) {
            1 => "user",
            2 => "assistant",
            _ => "system",
        };
        injections.push((depth, seq, ChatMsg::new(role, ctx.persona.clone())));
    }

    // ---- chat history -> messages
    let names_behavior = ji64(o, "names_behavior", 0);
    let mut hist: Vec<ChatMsg> = Vec::new();
    for m in a.history.iter() {
        let narrator = m
            .get("extra")
            .map(|e| jstr(e, "type") == "narrator")
            .unwrap_or(false);
        if chat::is_system(m) && !narrator {
            continue;
        }
        let text = subst(&chat::mes(m).replace('\r', ""), &ctx);
        if text.trim().is_empty() {
            continue;
        }
        let who = chat::name(m);
        let role = if narrator {
            "system"
        } else if chat::is_user(m) {
            "user"
        } else {
            "assistant"
        };
        let mut msg = ChatMsg::new(role, text);
        match names_behavior {
            2 => {
                if !narrator {
                    msg.content = format!("{}: {}", who, msg.content);
                }
            }
            1 => {
                msg.name = sanitize_name(&who);
            }
            _ => {}
        }
        hist.push(msg);
    }

    // Insert depth injections (depth 0 = after the last message).
    injections.sort_by(|x, y| x.0.cmp(&y.0).then(y.1.cmp(&x.1)));
    let base_len = hist.len();
    for (depth, _, msg) in injections.into_iter() {
        let idx = base_len.saturating_sub(depth).min(hist.len());
        hist.insert(idx, msg);
    }

    // ---- budget
    let max_ctx = ji64(o, "openai_max_context", 8192).max(512) as usize;
    let max_resp = ji64(o, "openai_max_tokens", 300).max(0) as usize;
    let avail = max_ctx.saturating_sub(max_resp);

    let new_chat_text = {
        let t = jstr(o, "new_chat_prompt");
        let t = if t.is_empty() {
            "[Start a new Chat]".to_string()
        } else {
            t
        };
        subst(&t, &ctx)
    };
    let new_chat_msg = if new_chat_text.trim().is_empty() {
        None
    } else {
        Some(ChatMsg::new("system", new_chat_text))
    };

    let mut static_tokens = 0usize;
    for s in slots.iter() {
        if let Slot::Msg(m) = s {
            static_tokens += est_msg(m, cpt);
        }
    }
    if let Some(m) = &new_chat_msg {
        static_tokens += est_msg(m, cpt);
    }
    let hist_budget = avail.saturating_sub(static_tokens);

    // Newest messages first; always keep at least the newest one.
    let mut kept: Vec<ChatMsg> = Vec::new();
    let mut used = 0usize;
    for m in hist.into_iter().rev() {
        let t = est_msg(&m, cpt);
        if !kept.is_empty() && used + t > hist_budget {
            break;
        }
        used += t;
        kept.push(m);
    }
    kept.reverse();

    let send_if_empty = jstr(o, "send_if_empty");
    if a.continue_last {
        // Like the web UI: unless "continue prefill" is on, ask the model to carry on.
        if !jbool(o, "continue_prefill", false) {
            let nudge = {
                let t = jstr(o, "continue_nudge_prompt");
                if t.trim().is_empty() {
                    "[Continue your last message without repeating its original content.]"
                        .to_string()
                } else {
                    t
                }
            };
            let nudge = nudge.replace("{{lastChatMessage}}", &ctx.last_message);
            kept.push(ChatMsg::new("system", subst(&nudge, &ctx)));
        }
    } else if !send_if_empty.trim().is_empty()
        && kept.last().map(|m| m.role == "assistant").unwrap_or(false)
    {
        // Same as the web UI's empty-message replacement: only added when the
        // history ends with a reply (nothing was typed) and it fits the budget.
        let m = ChatMsg::new("user", subst(&send_if_empty, &ctx));
        if used + est_msg(&m, cpt) <= hist_budget {
            kept.push(m);
        }
    }

    // Example dialogue only gets what is left over after the history.
    let mut example_msgs: Vec<ChatMsg> = Vec::new();
    let has_examples_slot = slots.iter().any(|s| matches!(s, Slot::Examples));
    if has_examples_slot {
        let marker_text = {
            let t = jstr(o, "new_example_chat_prompt");
            let t = if t.is_empty() {
                "[Example Chat]".to_string()
            } else {
                t
            };
            subst(&t, &ctx)
        };
        let mut left = hist_budget.saturating_sub(used);
        for block in parse_examples(&ctx.mes_examples, a.user_name, &a.ch.name) {
            let mut cost: usize = block.iter().map(|m| est_msg(m, cpt)).sum();
            if !marker_text.trim().is_empty() {
                cost += est_tokens(&marker_text, cpt) + 4;
            }
            if cost > left {
                break;
            }
            left -= cost;
            if !marker_text.trim().is_empty() {
                example_msgs.push(ChatMsg::new("system", marker_text.clone()));
            }
            example_msgs.extend(block);
        }
    }

    // ---- assemble
    let mut out: Vec<ChatMsg> = Vec::new();
    let mut history_placed = false;
    for slot in slots.into_iter() {
        match slot {
            Slot::Msg(m) => out.push(m),
            Slot::History => {
                if let Some(m) = &new_chat_msg {
                    out.push(m.clone());
                }
                out.append(&mut kept);
                history_placed = true;
            }
            Slot::Examples => out.append(&mut example_msgs),
        }
    }
    if !history_placed {
        out.append(&mut kept);
    }

    if jbool(o, "squash_system_messages", false) {
        let mut squashed: Vec<ChatMsg> = Vec::new();
        for m in out.into_iter() {
            if let Some(last) = squashed.last_mut() {
                if last.role == "system"
                    && m.role == "system"
                    && last.name.is_none()
                    && m.name.is_none()
                {
                    last.content.push('\n');
                    last.content.push_str(&m.content);
                    continue;
                }
            }
            squashed.push(m);
        }
        out = squashed;
    }

    out
}
