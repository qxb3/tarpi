//! Text Completion support (llama.cpp, KoboldCpp, Ooba, TabbyAPI, vLLM, ...).
//!
//! Mirrors SillyTavern's own pipeline for `main_api = "textgenerationwebui"`:
//! context template (story string), instruct template, example dialogue,
//! stop strings and the sampler parameters from the active preset. All of it
//! is read from the live settings.json, so whatever is selected in the web UI
//! is what gets used here.

use crate::character::Character;
use crate::chat;
use crate::gen::Request;
use crate::jv::*;
use crate::prompt::{subst, Ctx};
use anyhow::{bail, Result};
use serde_json::{json, Map, Value};
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
    pub history: &'a [Value],
    pub chars_per_token: f32,
    /// Text of the reply being continued. `history` then excludes that reply
    /// and the prompt ends with it, right after the character cue.
    pub continue_text: Option<&'a str>,
}

pub struct TgPrompt {
    pub prompt: String,
    pub stop: Vec<String>,
}

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

    // ---- story string
    let persona_in_prompt = a.persona_position == 0;
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
    for k in [
        "wiBefore",
        "wiAfter",
        "loreBefore",
        "loreAfter",
        "anchorBefore",
        "anchorAfter",
    ] {
        vars.insert(k, String::new());
    }
    vars.insert("mesExamples", examples.join(""));
    vars.insert("mesExamplesRaw", examples_raw.join(""));

    let story_pos = ji64(ctx_v, "story_string_position", 0);
    let template = jstr(ctx_v, "story_string");
    let mut story = String::new();
    render_nodes(&parse_template(&template), &vars, &mut story);
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
    if a.persona_position == 4 && !ctx.persona.is_empty() {
        injects.push(Inject {
            depth: ji64(pu, "persona_description_depth", 2).max(0) as usize,
            role: ji64(pu, "persona_description_role", 0).clamp(0, 2) as u8,
            text: ctx.persona.clone(),
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
    for m in formatted.iter().skip(start) {
        prompt.push_str(m);
    }
    prompt.push_str(&tail);
    if !cont_text.is_empty() {
        if !prompt.ends_with(char::is_whitespace) {
            prompt.push(' ');
        }
        prompt.push_str(&cont_text);
    }
    let mut prompt = prompt.replace('\r', "");
    if jbool(pu, "collapse_newlines", false) {
        while prompt.contains("\n\n") {
            prompt = prompt.replace("\n\n", "\n");
        }
    }

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

    TgPrompt { prompt, stop: uniq }
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

// ------------------------------------------------------------------ request

fn put(m: &mut Map<String, Value>, key: &str, v: Option<Value>) {
    if let Some(v) = v {
        if !v.is_null() {
            m.insert(key.to_string(), v);
        }
    }
}

fn parse_breakers(t: &Value) -> Option<Value> {
    let raw = t.get("dry_sequence_breakers")?;
    match raw {
        Value::Array(_) => Some(raw.clone()),
        Value::String(s) => match serde_json::from_str::<Value>(s) {
            Ok(v @ Value::Array(_)) => Some(v),
            _ => Some(json!(s
                .split(',')
                .map(|x| x.to_string())
                .collect::<Vec<String>>())),
        },
        _ => None,
    }
}

/// Banned token ids / strings from the preset (`send_banned_tokens`). Plain
/// text entries would need a tokenizer, so only ids and quoted strings are sent.
fn banned(t: &Value, ctx: &Ctx) -> (String, Vec<String>) {
    if !jbool(t, "send_banned_tokens", false) {
        return (String::new(), Vec::new());
    }
    let mut tokens: Vec<String> = Vec::new();
    let mut strings: Vec<String> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let joined = format!(
        "{}\n{}",
        jstr(t, "banned_tokens"),
        jstr(t, "global_banned_tokens")
    );
    for line in joined.split('\n').filter(|l| !l.is_empty()) {
        if seen.iter().any(|s| s == line) {
            continue;
        }
        seen.push(line.to_string());
        let line = subst(line, ctx);
        if line.starts_with('[') && line.ends_with(']') {
            if let Ok(Value::Array(a)) = serde_json::from_str::<Value>(&line) {
                for x in a {
                    if let Some(i) = x.as_i64() {
                        let s = i.to_string();
                        if !tokens.contains(&s) {
                            tokens.push(s);
                        }
                    }
                }
            }
        } else if line.len() >= 2 && line.starts_with('"') && line.ends_with('"') {
            strings.push(line[1..line.len() - 1].to_string());
        }
    }
    (tokens.join(","), strings)
}

pub fn api_type(settings: &Value) -> String {
    jstr(sub(settings, "textgenerationwebui_settings"), "type")
}

pub fn server_url(settings: &Value) -> String {
    let t = sub(settings, "textgenerationwebui_settings");
    jstr(sub(t, "server_urls"), &jstr(t, "type"))
}

/// The model name SillyTavern has for the selected Text Completion type
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

pub fn build_request(
    settings: &Value,
    tp: &TgPrompt,
    user: &str,
    char_name: &str,
) -> Result<Request> {
    let t = sub(settings, "textgenerationwebui_settings");
    let ty = jstr(t, "type");
    if ty.is_empty() {
        bail!("no Text Completion API type is set in SillyTavern");
    }
    let fixed_server = [
        "openrouter",
        "togetherai",
        "infermaticai",
        "dreamgen",
        "mancer",
        "featherless",
    ];
    if fixed_server.contains(&ty.as_str()) {
        bail!(
            "Text Completion type '{}' (hosted API) is not supported yet; use a Chat Completion source or a local backend",
            ty
        );
    }
    let server = server_url(settings);
    if server.trim().is_empty() {
        bail!(
            "no server URL is set for the '{}' Text Completion API in SillyTavern",
            ty
        );
    }

    let ctx = names_ctx(user, char_name);
    let amount = ji64(settings, "amount_gen", 350);
    let max_ctx = ji64(settings, "max_context", 8192);
    let stream = jbool(t, "streaming", false);
    let s = |k: &str| -> Option<Value> { t.get(k).cloned() };
    let is = |names: &[&str]| names.contains(&ty.as_str());

    let dynatemp = jbool(t, "dynatemp", false)
        && is(&[
            "ooba",
            "mancer",
            "koboldcpp",
            "tabby",
            "llamacpp",
            "aphrodite",
        ]);
    let min_temp = jf64(t, "min_temp", 0.0);
    let max_temp = jf64(t, "max_temp", 0.0);
    let seed = jf64(t, "seed", -1.0);
    let model = model_name(t, &ty);
    let (ban_tokens, ban_strings) = banned(t, &ctx);

    let mut m: Map<String, Value> = Map::new();
    put(&mut m, "prompt", Some(json!(tp.prompt)));
    if !model.is_empty() {
        put(&mut m, "model", Some(json!(model)));
    }
    put(&mut m, "max_new_tokens", Some(json!(amount)));
    put(&mut m, "max_tokens", Some(json!(amount)));
    put(
        &mut m,
        "temperature",
        if dynatemp {
            Some(json!((min_temp + max_temp) / 2.0))
        } else {
            s("temp")
        },
    );
    put(&mut m, "top_p", s("top_p"));
    put(&mut m, "typical_p", s("typical_p"));
    put(&mut m, "typical", s("typical_p"));
    if seed >= 0.0 {
        put(&mut m, "sampler_seed", Some(json!(seed as i64)));
    }
    put(&mut m, "min_p", s("min_p"));
    put(&mut m, "repetition_penalty", s("rep_pen"));
    put(&mut m, "frequency_penalty", s("freq_pen"));
    put(&mut m, "presence_penalty", s("presence_pen"));
    put(&mut m, "top_k", s("top_k"));
    put(&mut m, "skew", s("skew"));
    if is(&["ooba"]) {
        put(&mut m, "min_length", s("min_length"));
        put(&mut m, "num_beams", s("num_beams"));
        put(&mut m, "length_penalty", s("length_penalty"));
        put(&mut m, "early_stopping", s("early_stopping"));
        put(&mut m, "sampler_priority", s("sampler_priority"));
    }
    put(&mut m, "min_tokens", s("min_length"));
    put(&mut m, "add_bos_token", s("add_bos_token"));
    if dynatemp {
        put(&mut m, "dynamic_temperature", Some(json!(true)));
        put(&mut m, "dynatemp_low", Some(json!(min_temp)));
        put(&mut m, "dynatemp_high", Some(json!(max_temp)));
        put(
            &mut m,
            "dynatemp_range",
            Some(json!((max_temp - min_temp) / 2.0)),
        );
        put(&mut m, "dynatemp_exponent", s("dynatemp_exponent"));
    }
    put(&mut m, "smoothing_factor", s("smoothing_factor"));
    put(&mut m, "smoothing_curve", s("smoothing_curve"));
    put(&mut m, "dry_allowed_length", s("dry_allowed_length"));
    put(&mut m, "dry_multiplier", s("dry_multiplier"));
    put(&mut m, "dry_base", s("dry_base"));
    put(&mut m, "dry_sequence_breakers", parse_breakers(t));
    put(&mut m, "dry_penalty_last_n", s("dry_penalty_last_n"));
    put(&mut m, "max_tokens_second", s("max_tokens_second"));
    if is(&["llamacpp"]) {
        put(&mut m, "samplers", s("samplers"));
    }
    put(&mut m, "stopping_strings", Some(json!(tp.stop)));
    put(&mut m, "stop", Some(json!(tp.stop)));
    put(&mut m, "truncation_length", Some(json!(max_ctx)));
    put(&mut m, "ban_eos_token", s("ban_eos_token"));
    put(&mut m, "skip_special_tokens", s("skip_special_tokens"));
    put(&mut m, "include_reasoning", s("include_reasoning"));
    put(&mut m, "top_a", s("top_a"));
    put(&mut m, "tfs", s("tfs"));
    if is(&["ooba", "mancer"]) {
        put(&mut m, "epsilon_cutoff", s("epsilon_cutoff"));
        put(&mut m, "eta_cutoff", s("eta_cutoff"));
    }
    put(&mut m, "mirostat_mode", s("mirostat_mode"));
    put(&mut m, "mirostat_tau", s("mirostat_tau"));
    put(&mut m, "mirostat_eta", s("mirostat_eta"));
    put(&mut m, "custom_token_bans", Some(json!(ban_tokens)));
    put(&mut m, "banned_strings", Some(json!(ban_strings)));
    put(&mut m, "api_type", Some(json!(ty)));
    put(&mut m, "api_server", Some(json!(server)));
    if is(&["koboldcpp"]) {
        put(&mut m, "sampler_order", s("sampler_order"));
    }
    put(&mut m, "xtc_threshold", s("xtc_threshold"));
    put(&mut m, "xtc_probability", s("xtc_probability"));
    put(&mut m, "nsigma", s("nsigma"));
    put(&mut m, "top_n_sigma", s("nsigma"));
    put(&mut m, "min_keep", s("min_keep"));
    put(&mut m, "adaptive_target", s("adaptive_target"));
    put(&mut m, "adaptive_decay", s("adaptive_decay"));

    let grammar = jstr(t, "grammar_string");
    if is(&["vllm", "infermaticai"]) {
        put(&mut m, "n", Some(json!(1)));
        put(&mut m, "ignore_eos", s("ignore_eos_token"));
        put(
            &mut m,
            "spaces_between_special_tokens",
            s("spaces_between_special_tokens"),
        );
        if seed >= 0.0 {
            put(&mut m, "seed", Some(json!(seed as i64)));
        }
    } else {
        put(&mut m, "rep_pen", s("rep_pen"));
        put(&mut m, "rep_pen_range", s("rep_pen_range"));
        if is(&["tabby"]) {
            put(&mut m, "repetition_decay", s("rep_pen_decay"));
            put(&mut m, "speculative_ngram", s("speculative_ngram"));
        }
        put(&mut m, "repetition_penalty_range", s("rep_pen_range"));
        if is(&["ooba"]) {
            put(&mut m, "encoder_repetition_penalty", s("encoder_rep_pen"));
            put(&mut m, "no_repeat_ngram_size", s("no_repeat_ngram_size"));
            put(&mut m, "penalty_alpha", s("penalty_alpha"));
            put(&mut m, "do_sample", s("do_sample"));
        }
        if is(&["ooba", "aphrodite", "tabby"]) {
            put(&mut m, "temperature_last", s("temperature_last"));
        }
        if seed >= 0.0 {
            put(&mut m, "seed", Some(json!(seed as i64)));
        }
        put(
            &mut m,
            "guidance_scale",
            Some(t.get("guidance_scale").cloned().unwrap_or(json!(1))),
        );
        put(
            &mut m,
            "negative_prompt",
            Some(json!(subst(&jstr(t, "negative_prompt"), &ctx))),
        );
        if !grammar.is_empty() {
            put(&mut m, "grammar_string", Some(json!(grammar)));
        }
        if is(&["tabby", "llamacpp"]) {
            if let Some(js) = t.get("json_schema") {
                if js.as_object().map(|o| !o.is_empty()).unwrap_or(false) {
                    put(&mut m, "json_schema", Some(js.clone()));
                }
            }
        }
        put(&mut m, "repeat_penalty", s("rep_pen"));
        put(&mut m, "repeat_last_n", s("rep_pen_range"));
        put(&mut m, "n_predict", Some(json!(amount)));
        put(&mut m, "num_predict", Some(json!(amount)));
        put(&mut m, "num_ctx", Some(json!(max_ctx)));
        put(&mut m, "mirostat", s("mirostat_mode"));
        put(&mut m, "ignore_eos", s("ban_eos_token"));
        put(&mut m, "rep_pen_slope", s("rep_pen_slope"));
    }

    if is(&["koboldcpp"]) {
        if !grammar.is_empty() {
            put(&mut m, "grammar", Some(json!(grammar)));
        }
        put(&mut m, "trim_stop", Some(json!(true)));
    }
    if is(&["tabby", "llamacpp"]) {
        put(&mut m, "n", Some(json!(1)));
    }
    if is(&["llamacpp", "ollama"]) {
        // Token bans become [[id, false], ...] pairs for llama.cpp.
        let bans: Vec<Value> = ban_tokens
            .split(',')
            .filter_map(|x| x.parse::<i64>().ok())
            .map(|id| json!([id, false]))
            .collect();
        put(&mut m, "logit_bias", Some(Value::Array(bans)));
        put(&mut m, "grammar", Some(json!(grammar)));
        put(&mut m, "cache_prompt", Some(json!(true)));
        let empty = m
            .get("dry_sequence_breakers")
            .and_then(|v| v.as_array())
            .map(|a| a.is_empty())
            .unwrap_or(true);
        if empty {
            m.remove("dry_sequence_breakers");
        }
    }
    if is(&["llamacpp"]) && m.contains_key("json_schema") {
        m.remove("grammar");
        m.remove("grammar_string");
    }

    m.insert("stream".to_string(), json!(stream));

    Ok(Request {
        body: Value::Object(m),
        source: "textgen".to_string(),
        stream,
        model,
        endpoint: "/api/backends/text-completions/generate",
    })
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
