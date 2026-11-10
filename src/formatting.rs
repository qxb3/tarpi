//! Formatting: the context template (story string), the instruct template and
//! a few reply-cleanup switches.
//!
//! These are tarpi's own. They are kept in
//! `~/.config/tarpi/formatting.yaml` (all settings are written out, hand
//! edits are fine) and edited with `/formatting` in a chat. [`normalize`]
//! writes them into the in-memory settings the prompt builders read
//! (`power_user.context`, `power_user.instruct`, `power_user.trim_sentences`,
//! ...), so those keep reading the place they always did while the values
//! come from here.
//!
//! Until the file exists, the built-in defaults of each setting are used, and
//! `/formatting` starts from them.

use crate::config;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// Which section of the formatting a setting belongs to (and where it sits
/// in the in-memory settings, under `power_user`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sec {
    /// `power_user.context`
    Context,
    /// `power_user.instruct`
    Instruct,
    /// Keys directly in `power_user`.
    Reply,
}

impl Sec {
    pub fn title(self) -> &'static str {
        match self {
            Sec::Context => "Context template",
            Sec::Instruct => "Instruct template",
            Sec::Reply => "Reply cleanup",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Text,
    Toggle,
    Choice(&'static [&'static str]),
}

pub struct Field {
    pub sec: Sec,
    pub key: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    /// `"true"` / `"false"` for toggles.
    pub default: &'static str,
    pub help: &'static str,
}

const fn fld(
    sec: Sec,
    key: &'static str,
    label: &'static str,
    kind: Kind,
    default: &'static str,
    help: &'static str,
) -> Field {
    Field { sec, key, label, kind, default, help }
}

const NAMES: &[&str] = &["none", "force", "always"];

const STORY: &str = "{{#if system}}{{system}}\n{{/if}}{{#if wiBefore}}{{wiBefore}}\n{{/if}}\
{{#if description}}{{description}}\n{{/if}}{{#if personality}}{{char}}'s personality: {{personality}}\n{{/if}}\
{{#if scenario}}Scenario: {{scenario}}\n{{/if}}{{#if wiAfter}}{{wiAfter}}\n{{/if}}{{#if persona}}{{persona}}\n{{/if}}";

/// Every setting, in the order `/formatting` shows them (grouped by `sec`).
pub static FIELDS: &[Field] = &[
    fld(Sec::Context, "story_string", "Story string", Kind::Text, STORY,
        "Top of the prompt. Fields: {{system}} {{description}} {{personality}} {{scenario}} {{persona}} {{char}} {{user}}, and {{#if x}}...{{/if}}."),
    fld(Sec::Context, "example_separator", "Example separator", Kind::Text, "***",
        "Line put before each block of example dialogue."),
    fld(Sec::Context, "chat_start", "Chat start", Kind::Text, "***",
        "Line put before the chat history."),
    fld(Sec::Context, "use_stop_strings", "Stop on separators", Kind::Toggle, "false",
        "Also stop the reply at the example separator and chat start."),
    fld(Sec::Context, "names_as_stop_strings", "Stop on names", Kind::Toggle, "true",
        "Stop the reply when it starts to speak as you (or anyone else in the chat)."),
    fld(Sec::Instruct, "enabled", "Instruct mode", Kind::Toggle, "false",
        "Wrap every message in the sequences below. Off: plain \"Name: text\" lines."),
    fld(Sec::Instruct, "names_behavior", "Names", Kind::Choice(NAMES), "force",
        "none: no names. force: name only before the reply. always: name in every message."),
    fld(Sec::Instruct, "wrap", "Wrap with newlines", Kind::Toggle, "true",
        "Put a line break between a sequence and its text."),
    fld(Sec::Instruct, "macro", "Expand macros", Kind::Toggle, "true",
        "Replace {{char}}, {{user}} and {{name}} inside the sequences."),
    fld(Sec::Instruct, "input_sequence", "User prefix", Kind::Text, "",
        "Goes before each of your messages."),
    fld(Sec::Instruct, "input_suffix", "User suffix", Kind::Text, "",
        "Goes after each of your messages."),
    fld(Sec::Instruct, "output_sequence", "Assistant prefix", Kind::Text, "",
        "Goes before each reply; the last one cues the character to speak."),
    fld(Sec::Instruct, "output_suffix", "Assistant suffix", Kind::Text, "",
        "Goes after each reply."),
    fld(Sec::Instruct, "system_sequence", "System prefix", Kind::Text, "",
        "Goes before system messages."),
    fld(Sec::Instruct, "system_suffix", "System suffix", Kind::Text, "",
        "Goes after system messages."),
    fld(Sec::Instruct, "first_output_sequence", "First assistant prefix", Kind::Text, "",
        "Used instead of the assistant prefix for the first reply. Empty = same."),
    fld(Sec::Instruct, "last_output_sequence", "Last assistant prefix", Kind::Text, "",
        "Used instead of the assistant prefix for the reply being written. Empty = same."),
    fld(Sec::Instruct, "stop_sequence", "Stop sequence", Kind::Text, "",
        "Ends the reply (several: one per line, type \\n between them)."),
    fld(Sec::Instruct, "sequences_as_stop_strings", "Sequences as stops", Kind::Toggle, "false",
        "Also stop at the user, assistant and system prefixes."),
    fld(Sec::Instruct, "story_string_prefix", "Story string prefix", Kind::Text, "",
        "Goes before the story string."),
    fld(Sec::Instruct, "story_string_suffix", "Story string suffix", Kind::Text, "",
        "Goes after the story string."),
    fld(Sec::Reply, "trim_sentences", "Trim unfinished sentences", Kind::Toggle, "false",
        "Cut a reply back to its last complete sentence."),
    fld(Sec::Reply, "collapse_newlines", "Collapse blank lines", Kind::Toggle, "false",
        "Turn runs of blank lines in the prompt into one line break."),
    fld(Sec::Reply, "single_line", "Single line", Kind::Toggle, "false",
        "Stop the reply at its first line break."),
];

impl Field {
    pub fn default_value(&self) -> Value {
        match self.kind {
            Kind::Toggle => json!(self.default == "true"),
            _ => json!(self.default),
        }
    }

    /// `v` if it is the right kind of value for this setting, else the default.
    fn coerce(&self, v: &Value) -> Value {
        match (self.kind, v) {
            (Kind::Toggle, Value::Bool(_)) | (Kind::Text, Value::String(_)) => v.clone(),
            (Kind::Choice(opts), Value::String(s)) if opts.contains(&s.as_str()) => v.clone(),
            _ => self.default_value(),
        }
    }
}

// ---- presets ----------------------------------------------------------------

pub struct Preset {
    /// Which template it fills: `Context` or `Instruct`.
    pub sec: Sec,
    pub name: &'static str,
    pub help: &'static str,
    /// `(key, value)`; toggles as `"true"` / `"false"`. Every other setting of
    /// the template goes back to its default first (a context preset keeps the
    /// stop switches).
    pub values: &'static [(&'static str, &'static str)],
}

/// The presets of one template, in the order they are offered.
pub fn presets(sec: Sec) -> Vec<&'static Preset> {
    PRESETS.iter().filter(|p| p.sec == sec).collect()
}

const fn ins(
    name: &'static str,
    help: &'static str,
    values: &'static [(&'static str, &'static str)],
) -> Preset {
    Preset { sec: Sec::Instruct, name, help, values }
}

const fn ctx(
    name: &'static str,
    help: &'static str,
    values: &'static [(&'static str, &'static str)],
) -> Preset {
    Preset { sec: Sec::Context, name, help, values }
}

const SHORT: &str = "{{#if system}}{{system}}\n{{/if}}{{#if description}}{{description}}\n{{/if}}\
{{#if scenario}}{{scenario}}\n{{/if}}{{#if persona}}{{persona}}\n{{/if}}";

const LABELED: &str = "{{#if system}}{{system}}\n\n{{/if}}{{#if description}}Character: {{char}}\n{{description}}\n\n{{/if}}\
{{#if personality}}Personality: {{personality}}\n\n{{/if}}{{#if scenario}}Scenario: {{scenario}}\n\n{{/if}}\
{{#if persona}}About {{user}}: {{persona}}\n\n{{/if}}";

const PYGMALION: &str = "{{#if system}}{{system}}\n{{/if}}{{char}}'s Persona: {{description}}\n\
{{#if personality}}Personality: {{personality}}\n{{/if}}{{#if scenario}}Scenario: {{scenario}}\n{{/if}}\
{{#if persona}}{{persona}}\n{{/if}}";

/// Starting points for the templates, in the spirit of the chat templates
/// llama.cpp knows. Instruct presets come first (`Off` is the first of all).
pub static PRESETS: &[Preset] = &[
    ins("Off", "plain text, no instruct sequences", &[]),
    ins("ChatML", "<|im_start|>role ... <|im_end|>", &[
        ("enabled", "true"),
        ("input_sequence", "<|im_start|>user"),
        ("output_sequence", "<|im_start|>assistant"),
        ("system_sequence", "<|im_start|>system"),
        ("input_suffix", "<|im_end|>"),
        ("output_suffix", "<|im_end|>"),
        ("system_suffix", "<|im_end|>"),
        ("stop_sequence", "<|im_end|>"),
        ("sequences_as_stop_strings", "true"),
        ("story_string_prefix", "<|im_start|>system"),
        ("story_string_suffix", "<|im_end|>"),
    ]),
    ins("Llama 3", "<|start_header_id|>role<|end_header_id|> ... <|eot_id|>", &[
        ("enabled", "true"),
        ("wrap", "false"),
        ("input_sequence", "<|start_header_id|>user<|end_header_id|>\n\n"),
        ("output_sequence", "<|start_header_id|>assistant<|end_header_id|>\n\n"),
        ("system_sequence", "<|start_header_id|>system<|end_header_id|>\n\n"),
        ("input_suffix", "<|eot_id|>"),
        ("output_suffix", "<|eot_id|>"),
        ("system_suffix", "<|eot_id|>"),
        ("stop_sequence", "<|eot_id|>"),
        ("story_string_prefix", "<|start_header_id|>system<|end_header_id|>\n\n"),
        ("story_string_suffix", "<|eot_id|>"),
    ]),
    ins("Llama 2", "[INST] user [/INST] reply</s>", &[
        ("enabled", "true"),
        ("wrap", "false"),
        ("names_behavior", "none"),
        ("input_sequence", "[INST] "),
        ("input_suffix", " [/INST]"),
        ("output_suffix", "</s>"),
        ("stop_sequence", "</s>"),
    ]),
    ins("Mistral", "[INST] user [/INST]reply</s> (v1, v3)", &[
        ("enabled", "true"),
        ("wrap", "false"),
        ("names_behavior", "none"),
        ("input_sequence", "[INST] "),
        ("input_suffix", " [/INST]"),
        ("output_suffix", "</s>"),
        ("stop_sequence", "</s>"),
    ]),
    ins("Mistral V7", "[SYSTEM_PROMPT]..[/SYSTEM_PROMPT][INST]..[/INST]", &[
        ("enabled", "true"),
        ("wrap", "false"),
        ("names_behavior", "none"),
        ("input_sequence", "[INST]"),
        ("input_suffix", "[/INST]"),
        ("output_suffix", "</s>"),
        ("system_sequence", "[SYSTEM_PROMPT]"),
        ("system_suffix", "[/SYSTEM_PROMPT]"),
        ("stop_sequence", "</s>"),
        ("story_string_prefix", "[SYSTEM_PROMPT]"),
        ("story_string_suffix", "[/SYSTEM_PROMPT]"),
    ]),
    ins("Phi-3", "<|user|> ... <|end|> <|assistant|>", &[
        ("enabled", "true"),
        ("names_behavior", "none"),
        ("input_sequence", "<|user|>"),
        ("output_sequence", "<|assistant|>"),
        ("system_sequence", "<|system|>"),
        ("input_suffix", "<|end|>"),
        ("output_suffix", "<|end|>"),
        ("system_suffix", "<|end|>"),
        ("stop_sequence", "<|end|>"),
        ("story_string_prefix", "<|system|>"),
        ("story_string_suffix", "<|end|>"),
    ]),
    ins("Phi-4", "<|im_start|>role<|im_sep|> ... <|im_end|>", &[
        ("enabled", "true"),
        ("wrap", "false"),
        ("names_behavior", "none"),
        ("input_sequence", "<|im_start|>user<|im_sep|>"),
        ("output_sequence", "<|im_start|>assistant<|im_sep|>"),
        ("system_sequence", "<|im_start|>system<|im_sep|>"),
        ("input_suffix", "<|im_end|>"),
        ("output_suffix", "<|im_end|>"),
        ("system_suffix", "<|im_end|>"),
        ("stop_sequence", "<|im_end|>"),
        ("story_string_prefix", "<|im_start|>system<|im_sep|>"),
        ("story_string_suffix", "<|im_end|>"),
    ]),
    ins("Gemma", "<start_of_turn>user/model ... <end_of_turn>", &[
        ("enabled", "true"),
        ("names_behavior", "none"),
        ("input_sequence", "<start_of_turn>user"),
        ("output_sequence", "<start_of_turn>model"),
        ("system_sequence", "<start_of_turn>user"),
        ("input_suffix", "<end_of_turn>"),
        ("output_suffix", "<end_of_turn>"),
        ("system_suffix", "<end_of_turn>"),
        ("stop_sequence", "<end_of_turn>"),
        ("story_string_prefix", "<start_of_turn>user"),
        ("story_string_suffix", "<end_of_turn>"),
    ]),
    ins("Zephyr", "<|user|> ... </s> <|assistant|>", &[
        ("enabled", "true"),
        ("names_behavior", "none"),
        ("input_sequence", "<|user|>"),
        ("output_sequence", "<|assistant|>"),
        ("system_sequence", "<|system|>"),
        ("input_suffix", "</s>"),
        ("output_suffix", "</s>"),
        ("system_suffix", "</s>"),
        ("stop_sequence", "</s>"),
        ("story_string_prefix", "<|system|>"),
        ("story_string_suffix", "</s>"),
    ]),
    ins("Command R", "<|START_OF_TURN_TOKEN|><|USER_TOKEN|> ...", &[
        ("enabled", "true"),
        ("wrap", "false"),
        ("names_behavior", "none"),
        ("input_sequence", "<|START_OF_TURN_TOKEN|><|USER_TOKEN|>"),
        ("output_sequence", "<|START_OF_TURN_TOKEN|><|CHATBOT_TOKEN|>"),
        ("system_sequence", "<|START_OF_TURN_TOKEN|><|SYSTEM_TOKEN|>"),
        ("input_suffix", "<|END_OF_TURN_TOKEN|>"),
        ("output_suffix", "<|END_OF_TURN_TOKEN|>"),
        ("system_suffix", "<|END_OF_TURN_TOKEN|>"),
        ("stop_sequence", "<|END_OF_TURN_TOKEN|>"),
        ("story_string_prefix", "<|START_OF_TURN_TOKEN|><|SYSTEM_TOKEN|>"),
        ("story_string_suffix", "<|END_OF_TURN_TOKEN|>"),
    ]),
    ins("DeepSeek V3", "<｜User｜> ... <｜Assistant｜> ... <｜end▁of▁sentence｜>", &[
        ("enabled", "true"),
        ("wrap", "false"),
        ("names_behavior", "none"),
        ("input_sequence", "<｜User｜>"),
        ("output_sequence", "<｜Assistant｜>"),
        ("output_suffix", "<｜end▁of▁sentence｜>"),
        ("stop_sequence", "<｜end▁of▁sentence｜>"),
    ]),
    ins("OpenChat", "GPT4 Correct User: ... <|end_of_turn|>", &[
        ("enabled", "true"),
        ("wrap", "false"),
        ("names_behavior", "none"),
        ("input_sequence", "GPT4 Correct User: "),
        ("output_sequence", "GPT4 Correct Assistant: "),
        ("input_suffix", "<|end_of_turn|>"),
        ("output_suffix", "<|end_of_turn|>"),
        ("stop_sequence", "<|end_of_turn|>"),
    ]),
    ins("Granite", "<|start_of_role|>user<|end_of_role|> ... <|end_of_text|>", &[
        ("enabled", "true"),
        ("names_behavior", "none"),
        ("input_sequence", "<|start_of_role|>user<|end_of_role|>"),
        ("output_sequence", "<|start_of_role|>assistant<|end_of_role|>"),
        ("system_sequence", "<|start_of_role|>system<|end_of_role|>"),
        ("input_suffix", "<|end_of_text|>"),
        ("output_suffix", "<|end_of_text|>"),
        ("system_suffix", "<|end_of_text|>"),
        ("stop_sequence", "<|end_of_text|>"),
        ("story_string_prefix", "<|start_of_role|>system<|end_of_role|>"),
        ("story_string_suffix", "<|end_of_text|>"),
    ]),
    ins("Vicuna", "USER: ... ASSISTANT: ...</s>", &[
        ("enabled", "true"),
        ("wrap", "false"),
        ("names_behavior", "none"),
        ("input_sequence", "USER: "),
        ("input_suffix", "\n"),
        ("output_sequence", "ASSISTANT: "),
        ("output_suffix", "</s>\n"),
    ]),
    ins("Alpaca", "### Instruction: / ### Response:", &[
        ("enabled", "true"),
        ("names_behavior", "none"),
        ("input_sequence", "### Instruction:"),
        ("output_sequence", "### Response:"),
        ("system_sequence", "### Input:"),
    ]),
    ins("Metharme", "<|system|> <|user|> <|model|> (Pygmalion)", &[
        ("enabled", "true"),
        ("wrap", "false"),
        ("names_behavior", "none"),
        ("input_sequence", "<|user|>"),
        ("output_sequence", "<|model|>"),
        ("system_sequence", "<|system|>"),
    ]),
    ctx("Default", "system, description, personality, scenario, persona", &[
        ("story_string", STORY),
        ("example_separator", "***"),
        ("chat_start", "***"),
    ]),
    ctx("Short", "system, description, scenario, persona (no labels)", &[
        ("story_string", SHORT),
        ("example_separator", "***"),
        ("chat_start", "***"),
    ]),
    ctx("Labeled", "Character: / Personality: / Scenario: / About you:", &[
        ("story_string", LABELED),
        ("example_separator", "***"),
        ("chat_start", "***"),
    ]),
    ctx("Pygmalion", "{{char}}'s Persona: ..., <START> separators", &[
        ("story_string", PYGMALION),
        ("example_separator", "<START>"),
        ("chat_start", "<START>"),
    ]),
    ctx("No separators", "the default story string, nothing between parts", &[
        ("story_string", STORY),
        ("example_separator", ""),
        ("chat_start", ""),
    ]),
];

// ---- the settings -------------------------------------------------------------

/// What `formatting.yaml` holds: one map per section, keyed by setting.
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Formatting {
    pub context: Map<String, Value>,
    pub instruct: Map<String, Value>,
    pub reply: Map<String, Value>,
}

impl Formatting {
    fn map(&self, s: Sec) -> &Map<String, Value> {
        match s {
            Sec::Context => &self.context,
            Sec::Instruct => &self.instruct,
            Sec::Reply => &self.reply,
        }
    }

    fn map_mut(&mut self, s: Sec) -> &mut Map<String, Value> {
        match s {
            Sec::Context => &mut self.context,
            Sec::Instruct => &mut self.instruct,
            Sec::Reply => &mut self.reply,
        }
    }

    /// The value of a setting (its default when not set).
    pub fn get(&self, fd: &Field) -> Value {
        match self.map(fd.sec).get(fd.key) {
            Some(v) => fd.coerce(v),
            None => fd.default_value(),
        }
    }

    pub fn set(&mut self, fd: &Field, v: Value) {
        self.map_mut(fd.sec).insert(fd.key.to_string(), v);
    }

    pub fn reset(&mut self, fd: &Field) {
        self.map_mut(fd.sec).remove(fd.key);
    }

    /// Replace the template a preset belongs to (a context preset keeps the
    /// stop switches).
    pub fn apply_preset(&mut self, p: &Preset) {
        for fd in FIELDS.iter().filter(|f| f.sec == p.sec) {
            if !(p.sec == Sec::Context && fd.kind == Kind::Toggle) {
                self.reset(fd);
            }
        }
        for (key, text) in p.values {
            if let Some(fd) = FIELDS.iter().find(|f| f.key == *key) {
                let v = match fd.kind {
                    Kind::Toggle => json!(*text == "true"),
                    _ => json!(*text),
                };
                self.set(fd, v);
            }
        }
    }

    /// Every setting of section `s` with its value, as a JSON object.
    fn section(&self, s: Sec) -> Map<String, Value> {
        let mut out = Map::new();
        for fd in FIELDS.iter().filter(|f| f.sec == s) {
            out.insert(fd.key.to_string(), self.get(fd));
        }
        out
    }

    /// Write every setting into the in-memory `settings` the prompt builders read.
    pub fn apply(&self, settings: &mut Value) {
        if !settings.get("power_user").map(|v| v.is_object()).unwrap_or(false) {
            settings["power_user"] = json!({});
        }
        let pu = &mut settings["power_user"];
        pu["context"] = Value::Object(self.section(Sec::Context));
        pu["instruct"] = Value::Object(self.section(Sec::Instruct));
        for (k, v) in self.section(Sec::Reply) {
            pu[k] = v;
        }
        // Other keys the prompt builders read: fixed here, so nothing a
        // server might have set for them ever applies.
        pu["always_force_name2"] = json!(true);
        pu["pin_examples"] = json!(false);
        pu["strip_examples"] = json!(false);
        pu["custom_stopping_strings"] = json!("");
        pu["custom_stopping_strings_macro"] = json!(true);
    }

    // ---- the file

    /// `None` when there is no file yet.
    pub fn load() -> Result<Option<Formatting>> {
        let Some(path) = config::formatting_path() else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        if text.trim().is_empty() {
            return Ok(None);
        }
        let f = serde_yaml::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))?;
        Ok(Some(f))
    }

    pub fn save(&self) -> Result<()> {
        let path = config::formatting_path()
            .ok_or_else(|| anyhow!("no config directory on this system"))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("creating {}", dir.display()))?;
        }
        // Write every setting, so the file shows what there is to edit.
        let full = Formatting {
            context: self.section(Sec::Context),
            instruct: self.section(Sec::Instruct),
            reply: self.section(Sec::Reply),
        };
        let mut text = String::from(
            "# tarpi formatting. Edited with /formatting in a chat; hand edits are fine too.\n",
        );
        text.push_str(&serde_yaml::to_string(&full).context("writing formatting.yaml")?);
        let tmp = path.with_extension("yaml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }
}

/// Put the formatting into the in-memory settings: `formatting.yaml` when it
/// exists, otherwise the built-in defaults (also when the file cannot be
/// read). Nothing is written to disk.
pub fn normalize(settings: &mut Value) {
    let f = Formatting::load().ok().flatten().unwrap_or_default();
    f.apply(settings);
}

// ---- one-line editing --------------------------------------------------------

/// Text as shown in a one-line editor: a line break is `\n`, a backslash `\\`.
pub fn esc(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out
}

/// The reverse of [`esc`]; a backslash before anything else stays as it is.
pub fn unesc(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.peek() {
                Some('n') => {
                    it.next();
                    out.push('\n');
                }
                Some('\\') => {
                    it.next();
                    out.push('\\');
                }
                _ => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(key: &str) -> &'static Field {
        FIELDS.iter().find(|f| f.key == key).unwrap()
    }

    #[test]
    fn keys_are_unique() {
        for (i, a) in FIELDS.iter().enumerate() {
            for b in &FIELDS[i + 1..] {
                assert_ne!(a.key, b.key);
            }
        }
    }

    #[test]
    fn fields_are_grouped_by_section() {
        let mut seen: Vec<Sec> = Vec::new();
        for f in FIELDS {
            if seen.last() != Some(&f.sec) {
                assert!(!seen.contains(&f.sec), "{} is split", f.key);
                seen.push(f.sec);
            }
        }
    }

    #[test]
    fn escaping_round_trips() {
        for s in ["", "plain", "a\nb\n\nc", "back\\slash", "\\n", "x\\\ny"] {
            assert_eq!(unesc(&esc(s)), s);
        }
        assert_eq!(unesc("a\\nb"), "a\nb");
        assert_eq!(unesc("a\\qb"), "a\\qb");
    }

    #[test]
    fn apply_fills_what_the_prompt_builder_reads() {
        let mut f = Formatting::default();
        f.set(field("story_string"), json!("{{description}}"));
        f.set(field("enabled"), json!(true));
        f.set(field("trim_sentences"), json!(true));
        let mut s = json!({"power_user": {"context": {"stale": 1}, "other": 5}});
        f.apply(&mut s);
        assert_eq!(s["power_user"]["context"]["story_string"], "{{description}}");
        assert!(s["power_user"]["context"].get("stale").is_none());
        assert_eq!(s["power_user"]["instruct"]["enabled"], true);
        assert_eq!(s["power_user"]["instruct"]["names_behavior"], "force");
        assert_eq!(s["power_user"]["trim_sentences"], true);
        assert_eq!(s["power_user"]["other"], 5);
    }

    #[test]
    fn a_preset_replaces_the_whole_instruct_template() {
        let mut f = Formatting::default();
        f.set(field("last_output_sequence"), json!("old"));
        f.set(field("chat_start"), json!("keep"));
        f.apply_preset(PRESETS.iter().find(|p| p.name == "ChatML").unwrap());
        assert_eq!(f.get(field("enabled")), json!(true));
        assert_eq!(f.get(field("input_sequence")), json!("<|im_start|>user"));
        assert_eq!(f.get(field("last_output_sequence")), json!(""));
        assert_eq!(f.get(field("chat_start")), json!("keep"));
        f.apply_preset(&PRESETS[0]);
        assert_eq!(f.get(field("enabled")), json!(false));
    }

    #[test]
    fn a_context_preset_keeps_the_instruct_template_and_the_switches() {
        let mut f = Formatting::default();
        f.apply_preset(PRESETS.iter().find(|p| p.name == "ChatML").unwrap());
        f.set(field("use_stop_strings"), json!(true));
        f.apply_preset(PRESETS.iter().find(|p| p.name == "Pygmalion").unwrap());
        assert_eq!(f.get(field("chat_start")), json!("<START>"));
        assert!(f.get(field("story_string")).as_str().unwrap().contains("Persona:"));
        assert_eq!(f.get(field("use_stop_strings")), json!(true));
        assert_eq!(f.get(field("enabled")), json!(true));
    }

    #[test]
    fn every_preset_only_names_settings_of_its_own_template() {
        for p in PRESETS {
            assert!(matches!(p.sec, Sec::Context | Sec::Instruct), "{}", p.name);
            for (key, _) in p.values {
                let fd = FIELDS.iter().find(|f| f.key == *key);
                assert!(fd.is_some(), "{}: unknown setting {}", p.name, key);
                assert_eq!(fd.unwrap().sec, p.sec, "{}: {}", p.name, key);
            }
        }
        assert!(presets(Sec::Context).len() > 1 && presets(Sec::Instruct).len() > 1);
        assert!(presets(Sec::Reply).is_empty());
    }

    #[test]
    fn preset_names_are_unique_within_a_template() {
        for sec in [Sec::Context, Sec::Instruct] {
            let list = presets(sec);
            for (i, a) in list.iter().enumerate() {
                for b in &list[i + 1..] {
                    assert_ne!(a.name, b.name);
                }
            }
        }
    }

    #[test]
    fn the_file_format_round_trips() {
        let mut f = Formatting::default();
        f.set(field("story_string"), json!("{{#if description}}{{description}}\n{{/if}}"));
        f.set(field("wrap"), json!(false));
        let full = Formatting {
            context: f.section(Sec::Context),
            instruct: f.section(Sec::Instruct),
            reply: f.section(Sec::Reply),
        };
        let text = serde_yaml::to_string(&full).unwrap();
        let back: Formatting = serde_yaml::from_str(&text).unwrap();
        for fd in FIELDS {
            assert_eq!(back.get(fd), f.get(fd), "{}", fd.key);
        }
    }

    #[test]
    fn wrong_types_fall_back_to_the_default() {
        let mut f = Formatting::default();
        f.set(field("wrap"), json!("yes"));
        f.set(field("names_behavior"), json!("sometimes"));
        assert_eq!(f.get(field("wrap")), json!(true));
        assert_eq!(f.get(field("names_behavior")), json!("force"));
    }
}
