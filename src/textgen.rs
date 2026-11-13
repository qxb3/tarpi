//! Text Completion support (llama.cpp, KoboldCpp, Ooba, TabbyAPI, vLLM, ...).
//!
//! Mirrors SillyTavern's own pipeline for `main_api = "textgenerationwebui"`:
//! context template (story string), instruct template, example dialogue,
//! stop strings and the sampler parameters from the active preset. All of it
//! is read from the in-memory settings (`config::base_settings`) with tarpi's
//! own persona, system prompt, `formatting.yaml` and API connection written
//! over them.

use crate::character::Character;
use crate::chat;
use crate::jv::*;
use crate::note::{self, Note};
use crate::prompt::{subst, Ctx};
use serde_json::Value;
use std::collections::HashMap;

static NULL: Value = Value::Null;

fn sub<'a>(v: &'a Value, key: &str) -> &'a Value {
    v.get(key).unwrap_or(&NULL)
}

// ------------------------------------------------------------ small helpers

pub fn replace_ci(s: &str, pat: &str, rep: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let pat_l = pat.to_ascii_lowercase();
    let mut out = String::with_capacity(s.len());
    let mut last = 0usize;
    for (i, _) in lower.match_indices(&pat_l) {
        out.push_str(&s[last..i]);
        out.push_str(rep);
        last = i + pat.len();
    }
    out.push_str(&s[last..]);
    out
}

fn names_ctx(user: &str, char_name: &str) -> Ctx {
    Ctx {
        user: user.to_string(),
        char_name: char_name.to_string(),
        ..Ctx::default()
    }
}

fn chop_one_newline(s: &mut String) {
    if s.ends_with('\n') {
        s.pop();
    }
}

// ------------------------------------------------ story string (handlebars)

enum Node {
    Text(String),
    Var(String),
    If {
        name: String,
        negate: bool,
        then: Vec<Node>,
        other: Vec<Node>,
    },
}

fn tokenize(tpl: &str) -> Vec<(bool, String)> {
    let mut toks: Vec<(bool, String)> = Vec::new();
    let mut rest = tpl;
    while let Some(s) = rest.find("{{") {
        if s > 0 {
            toks.push((false, rest[..s].to_string()));
        }
        let after = &rest[s + 2..];
        match after.find("}}") {
            Some(e) => {
                toks.push((true, after[..e].trim().to_string()));
                rest = &after[e + 2..];
            }
            None => {
                toks.push((false, rest[s..].to_string()));
                rest = "";
                break;
            }
        }
    }
    if !rest.is_empty() {
        toks.push((false, rest.to_string()));
    }
    toks
}

/// Names a story string can test with `{{#if name}}` (the ones `build_prompt`
/// fills in).
pub const STORY_FIELDS: &[&str] = &[
    "system",
    "description",
    "personality",
    "scenario",
    "persona",
    "char",
    "user",
    "wiBefore",
    "wiAfter",
    "loreBefore",
    "loreAfter",
    "anchorBefore",
    "anchorAfter",
    "mesExamples",
    "mesExamplesRaw",
];

/// What is wrong with a story string, in words for the `/formatting` editor.
/// The renderer is forgiving (an unclosed `{{#if}}` runs to the end, a stray
/// `{{/if}}` is printed as it is, a name that is never set is always empty),
/// so a typo gives a strange prompt and no error: this says so up front.
pub fn check_template(tpl: &str) -> Vec<String> {
    let mut problems: Vec<String> = Vec::new();
    let mut open = 0usize;
    for (is_tag, c) in tokenize(tpl) {
        if !is_tag {
            continue;
        }
        if let Some(n) = c.strip_prefix("#if ").or_else(|| c.strip_prefix("#unless ")) {
            let n = n.trim();
            if !STORY_FIELDS.contains(&n) {
                problems.push(format!("{{{{#if {}}}}} tests a name that is never filled in", n));
            }
            open += 1;
        } else if c == "else" {
            if open == 0 {
                problems.push("{{else}} is outside of any {{#if}}".to_string());
            }
        } else if c.starts_with('/') {
            if open == 0 {
                problems.push(format!("{{{{{}}}}} closes nothing", c));
            } else {
                open -= 1;
            }
        }
    }
    if open > 0 {
        problems.push(format!(
            "{} {{{{#if}}}} {} never closed with {{{{/if}}}}",
            open,
            if open == 1 { "is" } else { "are" }
        ));
    }
    problems
}

fn parse_block(toks: &[(bool, String)], pos: &mut usize) -> Vec<Node> {
    let mut out = Vec::new();
    while *pos < toks.len() {
        let (is_tag, c) = &toks[*pos];
        if !*is_tag {
            out.push(Node::Text(c.clone()));
            *pos += 1;
            continue;
        }
        if c == "else" || c.starts_with('/') {
            break;
        }
        let head = if let Some(n) = c.strip_prefix("#if ") {
            Some((n.trim().to_string(), false))
        } else {
            c.strip_prefix("#unless ")
                .map(|n| (n.trim().to_string(), true))
        };
        if let Some((name, negate)) = head {
            *pos += 1;
            let then = parse_block(toks, pos);
            let mut other = Vec::new();
            if *pos < toks.len() && toks[*pos].0 && toks[*pos].1 == "else" {
                *pos += 1;
                other = parse_block(toks, pos);
            }
            if *pos < toks.len() && toks[*pos].0 && toks[*pos].1.starts_with('/') {
                *pos += 1;
            }
            out.push(Node::If {
                name,
                negate,
                then,
                other,
            });
            continue;
        }
        out.push(Node::Var(c.clone()));
        *pos += 1;
    }
    out
}

fn parse_template(tpl: &str) -> Vec<Node> {
    let toks = tokenize(tpl);
    let mut pos = 0usize;
    let mut nodes = Vec::new();
    while pos < toks.len() {
        nodes.extend(parse_block(&toks, &mut pos));
        if pos < toks.len() {
            // Stray `else` / closing tag: keep it as literal text.
            nodes.push(Node::Text(format!("{{{{{}}}}}", toks[pos].1)));
            pos += 1;
        }
    }
    nodes
}

fn render_nodes(nodes: &[Node], vars: &HashMap<&str, String>, out: &mut String) {
    for n in nodes {
        match n {
            Node::Text(t) => out.push_str(t),
            Node::Var(name) => match vars.get(name.as_str()) {
                Some(v) => out.push_str(v),
                None => {
                    // Not a story-string field (e.g. {{trim}}): leave for macro substitution.
                    out.push_str("{{");
                    out.push_str(name);
                    out.push_str("}}");
                }
            },
            Node::If {
                name,
                negate,
                then,
                other,
            } => {
                let truthy = vars
                    .get(name.as_str())
                    .map(|v| !v.is_empty())
                    .unwrap_or(false);
                if truthy != *negate {
                    render_nodes(then, vars, out);
                } else {
                    render_nodes(other, vars, out);
                }
            }
        }
    }
}

// ------------------------------------------------------------ instruct mode

#[derive(Default)]
struct Instruct {
    enabled: bool,
    input_sequence: String,
    output_sequence: String,
    first_output_sequence: String,
    last_output_sequence: String,
    system_sequence: String,
    last_system_sequence: String,
    first_input_sequence: String,
    last_input_sequence: String,
    input_suffix: String,
    output_suffix: String,
    system_suffix: String,
    stop_sequence: String,
    wrap: bool,
    macro_on: bool,
    names_behavior: String,
    system_same_as_user: bool,
    skip_examples: bool,
    sequences_as_stop_strings: bool,
    story_string_prefix: String,
    story_string_suffix: String,
}

impl Instruct {
    fn from_value(v: &Value) -> Instruct {
        let nb = jstr(v, "names_behavior");
        Instruct {
            enabled: jbool(v, "enabled", false),
            input_sequence: jstr(v, "input_sequence"),
            output_sequence: jstr(v, "output_sequence"),
            first_output_sequence: jstr(v, "first_output_sequence"),
            last_output_sequence: jstr(v, "last_output_sequence"),
            system_sequence: jstr(v, "system_sequence"),
            last_system_sequence: jstr(v, "last_system_sequence"),
            first_input_sequence: jstr(v, "first_input_sequence"),
            last_input_sequence: jstr(v, "last_input_sequence"),
            input_suffix: jstr(v, "input_suffix"),
            output_suffix: jstr(v, "output_suffix"),
            system_suffix: jstr(v, "system_suffix"),
            stop_sequence: jstr(v, "stop_sequence"),
            wrap: jbool(v, "wrap", true),
            macro_on: jbool(v, "macro", true),
            names_behavior: if nb.is_empty() {
                "force".to_string()
            } else {
                nb
            },
            system_same_as_user: jbool(v, "system_same_as_user", false),
            skip_examples: jbool(v, "skip_examples", false),
            sequences_as_stop_strings: jbool(v, "sequences_as_stop_strings", false),
            story_string_prefix: jstr(v, "story_string_prefix"),
            story_string_suffix: jstr(v, "story_string_suffix"),
        }
    }

    fn sep(&self) -> &'static str {
        if self.wrap {
            "\n"
        } else {
            ""
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Force {
    None,
    First,
    Last,
}

/// `{{macros}}` + `{{name}}` expansion used for instruct sequences.
fn expand_seq(ins: &Instruct, text: &str, user: &str, ch: &str, name: &str) -> String {
    if !ins.macro_on {
        return text.to_string();
    }
    let c = names_ctx(user, ch);
    let s = subst(text, &c);
    replace_ci(
        &s,
        "{{name}}",
        if name.is_empty() { "System" } else { name },
    )
}

fn format_instruct_chat(
    ins: &Instruct,
    user: &str,
    ch: &str,
    name: &str,
    mes: &str,
    is_user: bool,
    narrator: bool,
    force: Force,
) -> String {
    let include_names = !narrator && ins.names_behavior == "always";

    let prefix_raw: &str = if narrator {
        if ins.system_same_as_user {
            &ins.input_sequence
        } else {
            &ins.system_sequence
        }
    } else if is_user {
        match force {
            Force::First if !ins.first_input_sequence.is_empty() => &ins.first_input_sequence,
            Force::Last if !ins.last_input_sequence.is_empty() => &ins.last_input_sequence,
            _ => &ins.input_sequence,
        }
    } else {
        match force {
            Force::First if !ins.first_output_sequence.is_empty() => &ins.first_output_sequence,
            Force::Last if !ins.last_output_sequence.is_empty() => &ins.last_output_sequence,
            _ => &ins.output_sequence,
        }
    };
    let suffix_raw: &str = if narrator {
        if ins.system_same_as_user {
            &ins.input_suffix
        } else {
            &ins.system_suffix
        }
    } else if is_user {
        &ins.input_suffix
    } else {
        &ins.output_suffix
    };

    let prefix = expand_seq(ins, prefix_raw, user, ch, name);
    let mut suffix = expand_seq(ins, suffix_raw, user, ch, name);
    if suffix.is_empty() && ins.wrap {
        suffix = "\n".to_string();
    }

    let body = if include_names && !name.is_empty() {
        format!("{}: {}{}", name, mes, suffix)
    } else {
        format!("{}{}", mes, suffix)
    };
    [prefix, body]
        .into_iter()
        .filter(|x| !x.is_empty())
        .collect::<Vec<String>>()
        .join(ins.sep())
}

/// The line appended after the chat that cues the character to speak.
fn format_instruct_prompt(ins: &Instruct, user: &str, ch: &str) -> String {
    let include_names = !ch.is_empty() && ins.names_behavior == "always";
    let sequence_raw: &str = if !ins.last_output_sequence.is_empty() {
        &ins.last_output_sequence
    } else {
        &ins.output_sequence
    };

    let mut name_filler = String::new();
    if include_names
        && !ins.last_output_sequence.is_empty()
        && !ins.output_sequence.is_empty()
        && sequence_raw == ins.last_output_sequence
        && ins.output_sequence.ends_with(char::is_whitespace)
        && !ins.last_output_sequence.ends_with(char::is_whitespace)
    {
        if let Some(c) = ins.output_sequence.chars().last() {
            name_filler = c.to_string();
        }
    }

    let sequence = expand_seq(ins, sequence_raw, user, ch, ch);
    let sep = ins.sep();
    let text = if include_names {
        format!("{}{}{}{}{}:", sep, sequence, sep, name_filler, ch)
    } else {
        format!("{}{}", sep, sequence)
    };
    let text = if ins.wrap {
        text.trim_end().to_string()
    } else {
        text
    };
    if include_names {
        text
    } else {
        format!("{}{}", text, sep)
    }
}

/// Stop strings derived from instruct sequences and the context template.
fn instruct_stop_strings(ins: &Instruct, ctx_v: &Value, user: &str, ch: &str) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    if ins.enabled {
        let mut combined: Vec<String> = vec![ins.stop_sequence.clone()];
        if ins.sequences_as_stop_strings {
            let in_seq = replace_ci(&ins.input_sequence, "{{name}}", user);
            let out_seq = replace_ci(&ins.output_sequence, "{{name}}", ch);
            let first_out = replace_ci(&ins.first_output_sequence, "{{name}}", ch);
            let last_out = replace_ci(&ins.last_output_sequence, "{{name}}", ch);
            let sys = replace_ci(&ins.system_sequence, "{{name}}", "System");
            let last_sys = replace_ci(&ins.last_system_sequence, "{{name}}", "System");
            combined.extend([in_seq, out_seq, first_out, last_out, sys, last_sys]);
        }
        let mut seen: Vec<String> = Vec::new();
        for line in combined.join("\n").split('\n') {
            if seen.iter().any(|s| s == line) {
                continue;
            }
            seen.push(line.to_string());
            if line.trim().is_empty() {
                continue;
            }
            let wrapped = if ins.wrap {
                format!("\n{}", line)
            } else {
                line.to_string()
            };
            result.push(if ins.macro_on {
                subst(&wrapped, &names_ctx(user, ch))
            } else {
                wrapped
            });
        }
    }
    if jbool(ctx_v, "use_stop_strings", false) {
        let c = names_ctx(user, ch);
        for key in ["chat_start", "example_separator"] {
            let s = jstr(ctx_v, key);
            if !s.is_empty() {
                result.push(format!("\n{}", subst(&s, &c)));
            }
        }
    }
    result
}

// ---------------------------------------------------------------- examples

/// Port of `parseExampleIntoIndividual` (used by instruct example formatting).
fn split_example_block(text: &str, name1: &str, name2: &str) -> Vec<(bool, String)> {
    let mut result: Vec<(bool, String)> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut in_user = false;
    let mut in_bot = false;
    let user_prefix = format!("{}:", name1);
    let bot_prefix = format!("{}:", name2);

    let flush = |cur: &mut Vec<&str>, is_user: bool, result: &mut Vec<(bool, String)>| {
        let name_prefix = if is_user { &user_prefix } else { &bot_prefix };
        let joined = cur.join("\n").replacen(name_prefix.as_str(), "", 1);
        result.push((is_user, joined.trim().to_string()));
        cur.clear();
    };

    // The first line is always the "{Example Dialogue:}" heading.
    for line in text.split('\n').skip(1) {
        if line.starts_with(&user_prefix) {
            in_user = true;
            if in_bot {
                flush(&mut cur, false, &mut result);
            }
            in_bot = false;
        } else if line.starts_with(&bot_prefix) {
            in_bot = true;
            if in_user {
                flush(&mut cur, true, &mut result);
            }
            in_user = false;
        }
        cur.push(line);
    }
    if in_user {
        flush(&mut cur, true, &mut result);
    } else if in_bot {
        flush(&mut cur, false, &mut result);
    }
    result
}

/// Case-insensitive replace of the first occurrence only.
fn replace_first_ci(s: &str, pat: &str, rep: &str) -> String {
    let lower = s.to_ascii_lowercase();
    match lower.find(&pat.to_ascii_lowercase()) {
        Some(i) => format!("{}{}{}", &s[..i], rep, &s[i + pat.len()..]),
        None => s.to_string(),
    }
}

fn parse_mes_examples(raw: &str, is_instruct: bool, example_sep: &str) -> Vec<String> {
    let raw = raw.to_string();
    if raw.is_empty() || raw == "<START>" {
        return Vec::new();
    }
    let text = if raw.starts_with("<START>") {
        raw
    } else {
        format!("<START>\n{}", raw.trim())
    };
    let heading = if is_instruct {
        "<START>\n".to_string()
    } else if example_sep.is_empty() {
        String::new()
    } else {
        format!("{}\n", example_sep)
    };
    let lower = text.to_ascii_lowercase();
    let mut starts: Vec<usize> = lower.match_indices("<start>").map(|(i, _)| i).collect();
    starts.push(text.len());
    starts
        .windows(2)
        .map(|w| format!("{}{}\n", heading, text[w[0] + 7..w[1]].trim()))
        .collect()
}

fn format_instruct_examples(
    ins: &Instruct,
    items: &[String],
    example_sep: &str,
    user: &str,
    ch: &str,
) -> Vec<String> {
    let heading = if example_sep.is_empty() {
        String::new()
    } else {
        format!("{}\n", example_sep)
    };
    let fallback = |items: &[String]| -> Vec<String> {
        items
            .iter()
            .map(|x| replace_first_ci(x, "<START>\n", &heading))
            .collect()
    };
    if ins.skip_examples {
        return fallback(items);
    }

    let include_names = ins.names_behavior == "always";
    let mut in_p = expand_seq(ins, &ins.input_sequence, user, ch, user);
    let mut out_p = expand_seq(ins, &ins.output_sequence, user, ch, ch);
    let mut in_s = expand_seq(ins, &ins.input_suffix, user, ch, user);
    let mut out_s = expand_seq(ins, &ins.output_suffix, user, ch, ch);
    if ins.macro_on {
        if in_s.is_empty() && ins.wrap {
            in_s = "\n".to_string();
        }
        if out_s.is_empty() && ins.wrap {
            out_s = "\n".to_string();
        }
    }
    if !ins.macro_on {
        in_p = ins.input_sequence.clone();
        out_p = ins.output_sequence.clone();
        in_s = ins.input_suffix.clone();
        out_s = ins.output_suffix.clone();
    }

    let sep = ins.sep();
    let mut formatted: Vec<String> = Vec::new();
    for item in items {
        let cleaned = replace_first_ci(item, "<START>", "{Example Dialogue:}").replace('\r', "");
        let block = split_example_block(&cleaned, user, ch);
        if block.is_empty() {
            continue;
        }
        if !heading.is_empty() {
            formatted.push(heading.clone());
        }
        for (is_user, content) in block {
            let this_names = include_names || (ins.names_behavior == "force" && is_user);
            let (prefix, suffix, name) = if is_user {
                (&in_p, &in_s, user)
            } else {
                (&out_p, &out_s, ch)
            };
            let content = if this_names {
                format!("{}: {}", name, content)
            } else {
                content
            };
            let msg = [prefix.clone(), format!("{}{}", content, suffix)]
                .into_iter()
                .filter(|x| !x.is_empty())
                .collect::<Vec<String>>()
                .join(sep);
            formatted.push(msg);
        }
    }
    if formatted.is_empty() {
        return fallback(items);
    }
    formatted
}

// ------------------------------------------------------------ chat building

struct HMsg {
    name: Option<String>,
    is_user: bool,
    narrator: bool,
    mes: String,
}

struct Inject {
    depth: usize,
    role: u8, // 0 system, 1 user, 2 assistant
    text: String,
}

/// Port of `doChatInject`: insert depth-based prompts into the history.
fn inject_into_chat(msgs: &mut Vec<HMsg>, injects: &[Inject], user: &str, ch: &str) {
    if injects.is_empty() {
        return;
    }
    let max_depth = injects.iter().map(|i| i.depth).max().unwrap_or(0);
    msgs.reverse(); // newest first
    let mut total_inserted = 0usize;
    for depth in 0..=max_depth {
        let mut role_msgs: Vec<HMsg> = Vec::new();
        for role in 0u8..3 {
            let text: String = injects
                .iter()
                .filter(|i| i.depth == depth && i.role == role)
                .map(|i| i.text.as_str())
                .collect::<Vec<&str>>()
                .join("\n");
            let text = text.trim_start().to_string();
            if text.is_empty() {
                continue;
            }
            role_msgs.push(HMsg {
                name: match role {
                    1 => Some(user.to_string()),
                    2 => Some(ch.to_string()),
                    _ => None,
                },
                is_user: role == 1,
                narrator: role == 0,
                mes: text,
            });
        }
        if role_msgs.is_empty() {
            continue;
        }
        let idx = (depth + total_inserted).min(msgs.len());
        total_inserted += role_msgs.len();
        for (k, m) in role_msgs.into_iter().enumerate() {
            msgs.insert(idx + k, m);
        }
    }
    msgs.reverse();
}

fn role_num(s: &str) -> u8 {
    match s {
        "user" => 1,
        "assistant" => 2,
        _ => 0,
    }
}

// ------------------------------------------------------------ prompt output

pub struct TgArgs<'a> {
    /// The user's full settings.json.
    pub settings: &'a Value,
    pub ch: &'a Character,
    pub user_name: &'a str,
    pub persona: &'a str,
    pub persona_position: i64,
    /// The Author's Note of this chat (`/note`).
    pub note: &'a Note,
    pub history: &'a [Value],
    pub chars_per_token: f32,
    /// Text of the reply being continued. `history` then excludes that reply
    /// and the prompt ends with it, right after the character cue.
    pub continue_text: Option<&'a str>,
    /// `/regen <nudge>`: a direction for this reply only. It goes in as the
    /// very last system message, right before the character cue, and is never
    /// saved to the chat.
    pub nudge: Option<&'a str>,
}

pub struct TgPrompt {
    pub prompt: String,
    pub stop: Vec<String>,
    /// Byte offset in `prompt` where its last [`POST_MESSAGES`] messages
    /// start (the `/prompt` window's second page); `0` when not worked out.
    pub split: usize,
}

/// How many messages from the end of the prompt make its "post prompt".
pub const POST_MESSAGES: usize = 5;

pub fn build_prompt(a: &TgArgs) -> TgPrompt {
    let pu = sub(a.settings, "power_user");
    let ctx_v = sub(pu, "context");
    let ins = Instruct::from_value(sub(pu, "instruct"));
    let sysp = sub(pu, "sysprompt");
    let user = a.user_name;
    let chn = a.ch.name.as_str();
    let is_instruct = ins.enabled;

    // ---- macro context
    let mut ctx = names_ctx(user, chn);
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

    // ---- examples
    let example_sep = {
        let s = jstr(ctx_v, "example_separator");
        if s.is_empty() {
            String::new()
        } else {
            subst(&s, &ctx)
        }
    };
    let mut examples = parse_mes_examples(&ctx.mes_examples, is_instruct, &example_sep);
    let examples_raw = examples.clone();
    if is_instruct {
        examples = format_instruct_examples(&ins, &examples, &example_sep, user, chn);
    }

    // ---- system prompt (power_user.sysprompt)
    let prefer_char_prompt = jbool(pu, "prefer_character_prompt", true);
    let prefer_char_jb = jbool(pu, "prefer_character_jailbreak", true);
    let sys_enabled = jbool(sysp, "enabled", false);
    let mut system = String::new();
    let mut post_history = String::new();
    if sys_enabled {
        let content = jstr(sysp, "content");
        let mut c = ctx_clone(&ctx);
        c.original = content.clone();
        system = if prefer_char_prompt && !a.ch.system_prompt.trim().is_empty() {
            subst(&a.ch.system_prompt, &c)
        } else {
            subst(&content, &ctx)
        };
        let ph = jstr(sysp, "post_history");
        c.original = ph.clone();
        post_history = if prefer_char_jb && !a.ch.post_history_instructions.trim().is_empty() {
            subst(&a.ch.post_history_instructions, &c)
        } else {
            subst(&ph, &ctx)
        };
    }

    // ---- author's note (plus the persona, when its position is in the note)
    let user_msgs = a
        .history
        .iter()
        .filter(|m| !chat::is_system(m) && chat::is_user(m))
        .count();
    // No text, nothing sent from the note itself.
    let persona_position = a.persona_position;
    let note_text = if a.note.due(user_msgs) {
        subst(&a.note.text, &ctx).trim().to_string()
    } else {
        String::new()
    };
    let note_full = note::compose(&note_text, &ctx.persona, persona_position);
    let mut anchor_before = String::new();
    let mut anchor_after = String::new();
    match a.note.position {
        note::BEFORE_PROMPT => anchor_before = note_full.clone(),
        note::IN_PROMPT => anchor_after = note_full.clone(),
        _ => {}
    }

    // ---- story string
    let persona_in_prompt = persona_position == 0;
    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("description", ctx.description.clone());
    vars.insert("personality", ctx.personality.clone());
    vars.insert(
        "persona",
        if persona_in_prompt {
            ctx.persona.clone()
        } else {
            String::new()
        },
    );
    vars.insert("scenario", ctx.scenario.clone());
    vars.insert("system", system);
    vars.insert("char", chn.to_string());
    vars.insert("user", user.to_string());
    for k in ["wiBefore", "wiAfter", "loreBefore", "loreAfter"] {
        vars.insert(k, String::new());
    }
    vars.insert("anchorBefore", anchor_before.clone());
    vars.insert("anchorAfter", anchor_after.clone());
    vars.insert("mesExamples", examples.join(""));
    vars.insert("mesExamplesRaw", examples_raw.join(""));

    let story_pos = ji64(ctx_v, "story_string_position", 0);
    let template = jstr(ctx_v, "story_string");
    let mut story = String::new();
    render_nodes(&parse_template(&template), &vars, &mut story);
    // A template that has no {{anchorBefore}} / {{anchorAfter}} of its own
    // still gets the note: before the story string, or after it.
    if !anchor_before.is_empty() && !template.contains("anchorBefore") {
        story = format!("{}\n{}", anchor_before, story);
    }
    if !anchor_after.is_empty() && !template.contains("anchorAfter") {
        if !story.is_empty() && !story.ends_with('\n') {
            story.push('\n');
        }
        story.push_str(&anchor_after);
        story.push('\n');
    }
    let mut story = subst(&story, &ctx);
    story = story.trim_start_matches('\n').to_string();
    if !story.is_empty()
        && !story.ends_with('\n')
        && story_pos != 1
        && (!ins.enabled || (ins.wrap && ins.story_string_suffix.is_empty()))
    {
        story.push('\n');
    }
    let mut combined_story = story;
    if is_instruct && !combined_story.is_empty() && story_pos != 1 {
        if !ins.story_string_prefix.is_empty() {
            let p = replace_ci(&subst(&ins.story_string_prefix, &ctx), "{{name}}", "System");
            combined_story = format!("{}{}{}", p, ins.sep(), combined_story);
        }
        if !ins.story_string_suffix.is_empty() {
            combined_story.push_str(&subst(&ins.story_string_suffix, &ctx));
        }
    }

    // ---- depth injections
    let mut injects: Vec<Inject> = Vec::new();
    if story_pos == 1 && !combined_story.is_empty() {
        injects.push(Inject {
            depth: ji64(ctx_v, "story_string_depth", 1).max(0) as usize,
            role: ji64(ctx_v, "story_string_role", 0).clamp(0, 2) as u8,
            text: std::mem::take(&mut combined_story),
        });
    }
    if let Some(dp) = &a.ch.depth_prompt {
        let text = subst(&dp.prompt, &ctx);
        if !text.trim().is_empty() {
            injects.push(Inject {
                depth: dp.depth,
                role: role_num(&dp.role),
                text,
            });
        }
    }
    if persona_position == 4 && !ctx.persona.is_empty() {
        injects.push(Inject {
            depth: ji64(pu, "persona_description_depth", 2).max(0) as usize,
            role: ji64(pu, "persona_description_role", 0).clamp(0, 2) as u8,
            text: ctx.persona.clone(),
        });
    }
    if a.note.position == note::IN_CHAT && !note_full.is_empty() {
        injects.push(Inject {
            depth: a.note.depth,
            role: a.note.role,
            text: note_full.clone(),
        });
    }

    // ---- history
    let mut msgs: Vec<HMsg> = Vec::new();
    for m in a.history.iter() {
        if chat::is_system(m) {
            continue;
        }
        let is_user = chat::is_user(m);
        let nm = chat::name(m);
        let name = if nm.is_empty() {
            if is_user {
                None
            } else {
                Some(chn.to_string())
            }
        } else {
            Some(nm)
        };
        msgs.push(HMsg {
            name,
            is_user,
            narrator: false,
            mes: subst(chat::mes(m), &ctx),
        });
    }
    inject_into_chat(&mut msgs, &injects, user, chn);
    if !post_history.trim().is_empty() {
        msgs.push(HMsg {
            name: None,
            is_user: true,
            narrator: false,
            mes: post_history,
        });
    }
    if let Some(n) = a.nudge {
        let n = subst(n, &ctx).trim().to_string();
        if !n.is_empty() {
            msgs.push(HMsg {
                name: None,
                is_user: false,
                narrator: true,
                mes: format!("[Direction for {}'s next reply: {}]", chn, n),
            });
        }
    }

    let last_user_idx = msgs.iter().rposition(|m| m.is_user);
    let mut formatted: Vec<String> = Vec::with_capacity(msgs.len());
    for (i, m) in msgs.iter().enumerate() {
        let name = m.name.clone().unwrap_or_default();
        let text = if is_instruct {
            let mut force = Force::None;
            if i == 0 {
                force = Force::First;
            }
            if Some(i) == last_user_idx {
                force = Force::Last;
            }
            format_instruct_chat(&ins, user, chn, &name, &m.mes, m.is_user, m.narrator, force)
        } else if !name.is_empty() && !m.narrator {
            format!("{}: {}\n", name, m.mes)
        } else {
            format!("{}\n", m.mes)
        };
        formatted.push(text.replace('\r', ""));
    }

    // Last line: cue for the character.
    if let Some(last) = formatted.last_mut() {
        if !is_instruct || ins.wrap {
            chop_one_newline(last);
        }
    }
    let mut tail = String::new();
    if is_instruct {
        tail.push_str(&format_instruct_prompt(&ins, user, chn));
    } else if jbool(
        ctx_v,
        "always_force_name2",
        jbool(pu, "always_force_name2", true),
    ) {
        tail = if formatted.last().map(|l| l.ends_with('\n')).unwrap_or(true) {
            String::new()
        } else {
            "\n".to_string()
        };
        tail.push_str(&format!("{}:", chn));
    }

    // ---- budget + assembly
    let max_ctx = ji64(a.settings, "max_context", 8192).max(256) as usize;
    let amount = ji64(a.settings, "amount_gen", 350).max(1) as usize;
    let budget_chars_per_tok = if a.chars_per_token < 1.0 {
        3.2
    } else {
        a.chars_per_token
    };
    let max_prompt = max_ctx.saturating_sub(amount);

    let chat_start = {
        let s = jstr(ctx_v, "chat_start");
        if s.is_empty() {
            String::new()
        } else {
            subst(&format!("{}\n", s), &ctx)
        }
    };
    let pin_examples = jbool(pu, "pin_examples", false);
    let strip_examples = jbool(pu, "strip_examples", false);
    if strip_examples {
        examples.clear();
    }

    let clen = |s: &str| s.chars().count();
    let cont_text: String = a
        .continue_text
        .map(|t| subst(t, &ctx).trim_end().to_string())
        .unwrap_or_default();
    let fixed_chars =
        clen(&combined_story) + clen(&chat_start) + clen(&tail) + clen(&cont_text) + 1;
    let ex_chars: Vec<usize> = examples.iter().map(|e| clen(e)).collect();
    let msg_chars: Vec<usize> = formatted.iter().map(|m| clen(m)).collect();
    let mut ex_n = examples.len();
    let mut start = 0usize;
    let mut ex_sum: usize = ex_chars.iter().sum();
    let mut msg_sum: usize = msg_chars.iter().sum();
    loop {
        let total = fixed_chars + ex_sum + msg_sum;
        let tokens = ((total as f32) / budget_chars_per_tok).ceil() as usize;
        if tokens <= max_prompt {
            break;
        }
        if !pin_examples && ex_n > 0 {
            ex_n -= 1;
            ex_sum -= ex_chars[ex_n];
            continue;
        }
        if start + 1 < formatted.len() {
            msg_sum -= msg_chars[start];
            start += 1;
            continue;
        }
        break;
    }

    let mut prompt = String::new();
    prompt.push_str(&combined_story);
    for e in examples.iter().take(ex_n) {
        prompt.push_str(e);
    }
    prompt.push_str(&chat_start);
    // Where the last `POST_MESSAGES` messages of the prompt begin.
    let post_from = start.max(formatted.len().saturating_sub(POST_MESSAGES));
    let mut pre_len: Option<usize> = None;
    for (i, m) in formatted.iter().enumerate().skip(start) {
        if i == post_from {
            pre_len = Some(prompt.len());
        }
        prompt.push_str(m);
    }
    let pre_len = pre_len.unwrap_or(prompt.len());
    let pre_raw = prompt[..pre_len].to_string();
    prompt.push_str(&tail);
    if !cont_text.is_empty() {
        if !prompt.ends_with(char::is_whitespace) {
            prompt.push(' ');
        }
        prompt.push_str(&cont_text);
    }
    let collapse = jbool(pu, "collapse_newlines", false);
    let tidy = |t: &str| -> String {
        let mut t = t.replace('\r', "");
        if collapse {
            while t.contains("\n\n") {
                t = t.replace("\n\n", "\n");
            }
        }
        t
    };
    let prompt = tidy(&prompt);
    // The same cleaning of the first part gives where the second begins (the
    // text both share, in case collapsing changed the seam).
    let pre = tidy(&pre_raw);
    let split = prompt
        .char_indices()
        .zip(pre.chars())
        .find(|((_, a), b)| a != b)
        .map(|((i, _), _)| i)
        .unwrap_or_else(|| pre.len().min(prompt.len()));

    // ---- stop strings (getStoppingStrings for Text Completion)
    let mut stop: Vec<String> = Vec::new();
    if jbool(ctx_v, "names_as_stop_strings", false) {
        stop.push(format!("\n{}:", user));
        stop.push(format!("\n{}:", user));
    }
    stop.extend(instruct_stop_strings(&ins, ctx_v, user, chn));
    stop.extend(custom_stop_strings(pu, &ctx));
    if jbool(pu, "single_line", false) {
        stop.insert(0, "\n".to_string());
    }
    let mut uniq: Vec<String> = Vec::new();
    for s in stop {
        if !s.is_empty() && !uniq.contains(&s) {
            uniq.push(s);
        }
    }

    TgPrompt {
        prompt,
        stop: uniq,
        split,
    }
}

/// The prompt of `/summarize`: `body` (the instructions and the text to
/// digest) as one turn from the user, answered from the character's side of
/// the instruct template. The character card, the system prompt and the
/// persona are left out on purpose: they would only pull the reply towards
/// role-play and take room from the chat that is being summarized.
pub fn build_summary_prompt(settings: &Value, user: &str, ch: &str, body: &str) -> TgPrompt {
    let pu = sub(settings, "power_user");
    let ctx_v = sub(pu, "context");
    let ins = Instruct::from_value(sub(pu, "instruct"));
    let mut prompt = String::new();
    let mut stop: Vec<String> = Vec::new();
    if ins.enabled {
        let mut turn = format_instruct_chat(&ins, user, ch, "", body, true, false, Force::Last);
        // The cue below starts with a line break of its own.
        if ins.wrap {
            chop_one_newline(&mut turn);
        }
        prompt.push_str(&turn);
        prompt.push_str(&format_instruct_prompt(&ins, user, ch));
        stop = instruct_stop_strings(&ins, ctx_v, user, ch);
    } else {
        prompt.push_str(body);
        prompt.push_str("\n\nSUMMARY:\n");
        stop.push(format!("\n{}:", user));
        stop.push(format!("\n{}:", ch));
    }
    let mut uniq: Vec<String> = Vec::new();
    for s in stop {
        if !s.is_empty() && !uniq.contains(&s) {
            uniq.push(s);
        }
    }
    TgPrompt {
        prompt: prompt.replace('\r', ""),
        stop: uniq,
        split: 0,
    }
}

fn ctx_clone(c: &Ctx) -> Ctx {
    Ctx {
        user: c.user.clone(),
        char_name: c.char_name.clone(),
        description: c.description.clone(),
        personality: c.personality.clone(),
        scenario: c.scenario.clone(),
        persona: c.persona.clone(),
        mes_examples: c.mes_examples.clone(),
        last_message: c.last_message.clone(),
        last_user_message: c.last_user_message.clone(),
        last_char_message: c.last_char_message.clone(),
        original: c.original.clone(),
    }
}

fn custom_stop_strings(pu: &Value, ctx: &Ctx) -> Vec<String> {
    let raw = jstr(pu, "custom_stopping_strings");
    if raw.trim().is_empty() {
        return Vec::new();
    }
    let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(&raw) else {
        return Vec::new();
    };
    let macros = jbool(pu, "custom_stopping_strings_macro", true);
    arr.iter()
        .filter_map(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| if macros { subst(s, ctx) } else { s.to_string() })
        .collect()
}

// ------------------------------------------------------------------ backend

pub fn api_type(settings: &Value) -> String {
    jstr(sub(settings, "textgenerationwebui_settings"), "type")
}

pub fn server_url(settings: &Value) -> String {
    let t = sub(settings, "textgenerationwebui_settings");
    jstr(sub(t, "server_urls"), &jstr(t, "type"))
}

/// The model name the in-memory settings hold for the selected Text Completion type
/// (empty for types that have none).
pub fn current_model(settings: &Value) -> String {
    let t = sub(settings, "textgenerationwebui_settings");
    model_name(t, &jstr(t, "type"))
}

fn model_name(t: &Value, ty: &str) -> String {
    let key = match ty {
        "ooba" => "custom_model",
        "generic" => "generic_model",
        "vllm" => "vllm_model",
        "aphrodite" => "aphrodite_model",
        "ollama" => "ollama_model",
        "tabby" => "tabby_model",
        "llamacpp" => "llamacpp_model",
        _ => "",
    };
    if key.is_empty() {
        String::new()
    } else {
        jstr(t, key)
    }
}

// -------------------------------------------------------------- post-process

/// Port of SillyTavern's `trimToEndSentence`.
pub fn trim_to_end_sentence(input: &str) -> String {
    const PUNCT: &[char] = &[
        '.', '!', '?', '*', '"', ')', '}', '`', ']', '$', '。', '！', '？', '”', '）', '】', '’',
        '」', '_',
    ];
    let chars: Vec<char> = input.chars().collect();
    let mut last: Option<usize> = None;
    for i in (0..chars.len()).rev() {
        let c = chars[i];
        let emoji = !c.is_ascii()
            && !c.is_alphanumeric()
            && !PUNCT.contains(&c)
            && !c.is_whitespace()
            && (c as u32) >= 0x1F000;
        if PUNCT.contains(&c) || emoji {
            if !emoji && i > 0 && chars[i - 1].is_whitespace() {
                last = Some(i - 1);
            } else {
                last = Some(i);
            }
            break;
        }
    }
    match last {
        None => input.trim_end().to_string(),
        Some(i) => chars[..=i]
            .iter()
            .collect::<String>()
            .trim_end()
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::posedit::{BOTTOM_NOTE, TOP_NOTE};
    use serde_json::json;

    fn settings(story: &str) -> Value {
        json!({
            "main_api": "textgenerationwebui",
            "max_context": 100000,
            "amount_gen": 100,
            "power_user": { "context": { "story_string": story } },
        })
    }

    fn card() -> Character {
        Character {
            name: "Sofia".into(),
            description: "A baker.".into(),
            ..Character::default()
        }
    }

    fn history() -> Vec<Value> {
        vec![
            chat::user_message("Bob", "Hi"),
            chat::assistant_message("Sofia", "Hello", "m", "t", "t"),
            chat::user_message("Bob", "Go on"),
        ]
    }

    fn note(text: &str, position: i64, depth: usize) -> Note {
        Note {
            text: text.into(),
            position,
            depth,
            role: 0,
            interval: 1,
        }
    }

    fn build(
        story: &str,
        note: &Note,
        persona: &str,
        persona_position: i64,
        history: &[Value],
    ) -> String {
        let settings = settings(story);
        let ch = card();
        build_prompt(&TgArgs {
            settings: &settings,
            ch: &ch,
            user_name: "Bob",
            persona,
            persona_position,
            note,
            history,
            chars_per_token: 3.2,
            continue_text: None,
            nudge: None,
        })
        .prompt
    }

    #[test]
    fn the_split_leaves_the_last_five_messages_for_the_post_prompt() {
        let mut h = Vec::new();
        for i in 0..7 {
            if i % 2 == 0 {
                h.push(chat::user_message("Bob", &format!("m{}", i)));
            } else {
                h.push(chat::assistant_message("Sofia", &format!("m{}", i), "m", "t", "t"));
            }
        }
        let settings = settings("{{description}}");
        let ch = card();
        let tp = build_prompt(&TgArgs {
            settings: &settings,
            ch: &ch,
            user_name: "Bob",
            persona: "",
            persona_position: 0,
            note: &Note::default(),
            history: &h,
            chars_per_token: 3.2,
            continue_text: None,
            nudge: None,
        });
        let (pre, post) = tp.prompt.split_at(tp.split);
        assert!(pre.contains("A baker.") && pre.contains("m0") && pre.contains("m1"), "{}", pre);
        assert!(!pre.contains("m2"), "{}", pre);
        assert!(post.starts_with("Bob: m2\n"), "{}", post);
        assert!(post.ends_with("Bob: m6\nSofia:"), "{}", post);
        // With fewer than five messages they are all in the post-prompt.
        let tp = build_prompt(&TgArgs {
            settings: &settings,
            ch: &ch,
            user_name: "Bob",
            persona: "",
            persona_position: 0,
            note: &Note::default(),
            history: &history(),
            chars_per_token: 3.2,
            continue_text: None,
            nudge: None,
        });
        let (pre, post) = tp.prompt.split_at(tp.split);
        assert_eq!(pre, "A baker.\n");
        assert!(post.starts_with("Bob: Hi\n"), "{}", post);
    }

    #[test]
    fn no_note_adds_nothing() {
        let p = build("{{description}}", &Note::default(), "", 0, &history());
        assert!(p.starts_with("A baker.\n"));
        assert!(p.ends_with("Bob: Go on\nSofia:"));
    }

    #[test]
    fn the_note_goes_into_the_chat_at_its_depth() {
        let n = note("Keep it slow.", note::IN_CHAT, 1);
        let p = build("{{description}}", &n, "", 0, &history());
        assert!(p.contains("Sofia: Hello\nKeep it slow.\nBob: Go on\nSofia:"), "{}", p);
        let n = note("Keep it slow.", note::IN_CHAT, 0);
        let p = build("{{description}}", &n, "", 0, &history());
        assert!(p.ends_with("Bob: Go on\nKeep it slow.\nSofia:"), "{}", p);
    }

    #[test]
    fn the_note_goes_before_or_after_the_story_string() {
        let n = note("Keep it slow.", note::BEFORE_PROMPT, 4);
        let p = build("{{description}}", &n, "", 0, &history());
        assert!(p.starts_with("Keep it slow.\nA baker."), "{}", p);
        let n = note("Keep it slow.", note::IN_PROMPT, 4);
        let p = build("{{description}}", &n, "", 0, &history());
        assert!(p.starts_with("A baker.\nKeep it slow.\n"), "{}", p);
    }

    #[test]
    fn a_template_with_its_own_anchor_gets_the_note_once() {
        let n = note("Keep it slow.", note::IN_PROMPT, 4);
        let p = build("{{description}}\n{{anchorAfter}}", &n, "", 0, &history());
        assert_eq!(p.matches("Keep it slow.").count(), 1, "{}", p);
        let n = note("Keep it slow.", note::BEFORE_PROMPT, 4);
        let p = build("{{anchorBefore}}\n{{description}}", &n, "", 0, &history());
        assert_eq!(p.matches("Keep it slow.").count(), 1, "{}", p);
        assert!(p.starts_with("Keep it slow.\nA baker."), "{}", p);
    }

    #[test]
    fn the_interval_counts_your_messages() {
        let mut n = note("Keep it slow.", note::IN_CHAT, 1);
        n.interval = 2;
        // Two messages of yours: due.
        let p = build("{{description}}", &n, "", 0, &history());
        assert!(p.contains("Keep it slow."), "{}", p);
        // One message of yours: not due.
        let h = history();
        let p = build("{{description}}", &n, "", 0, &h[..1]);
        assert!(!p.contains("Keep it slow."), "{}", p);
        // Interval 0: never.
        n.interval = 0;
        let p = build("{{description}}", &n, "", 0, &history());
        assert!(!p.contains("Keep it slow."), "{}", p);
    }

    #[test]
    fn the_persona_joins_the_note_above_or_below_its_text() {
        let story = "{{description}}{{#if persona}}\n{{persona}}{{/if}}";
        let n = note("Keep it slow.", note::IN_CHAT, 1);
        let p = build(story, &n, "Bob is tall.", TOP_NOTE, &history());
        assert!(p.contains("Hello\nBob is tall.\nKeep it slow.\nBob: Go on"), "{}", p);
        assert_eq!(p.matches("Bob is tall.").count(), 1, "{}", p);
        let p = build(story, &n, "Bob is tall.", BOTTOM_NOTE, &history());
        assert!(p.contains("Hello\nKeep it slow.\nBob is tall.\nBob: Go on"), "{}", p);
        assert_eq!(p.matches("Bob is tall.").count(), 1, "{}", p);
    }

    #[test]
    fn a_persona_in_the_note_is_sent_even_without_note_text() {
        let n = note("", note::IN_CHAT, 1);
        let p = build("{{description}}", &n, "Bob is tall.", TOP_NOTE, &history());
        assert!(p.contains("Hello\nBob is tall.\nBob: Go on"), "{}", p);
    }
}
