//! Ollama (<https://ollama.com>), through its own API: `POST /api/generate`.
//!
//! tarpi builds the whole prompt itself (context template, instruct template,
//! examples, history), so the request is `raw`: Ollama must not wrap it in the
//! model's own chat template a second time. (Its OpenAI-compatible
//! `/v1/completions` route cannot promise that, which is why this kind exists
//! next to the OpenAI-compatible one.)
//!
//! - The reply is streamed as one JSON object per line (not `data:` lines);
//!   the last object has `done: true` and the token counts.
//! - `num_ctx` is sent with every request, taken from *Context size*. Ollama's
//!   own default window is small, and a prompt longer than the window is cut
//!   at the start without a word.
//! - Sampler settings go into the request's `options` object under Ollama's
//!   names. *Extra options* (merged into `options`) and *Extra request fields*
//!   (merged into the request itself) take anything this list does not have
//!   (`num_gpu`, `mirostat`, `think`, ...).
//!
//! ## Token count
//!
//! Ollama has no tokenizer route, so the count cannot be asked for in the
//! background the way llama.cpp's `/tokenize` is: the only other way is to
//! have the model read the whole prompt, which costs as much as a reply's
//! first step on every change of the chat. So, like OpenRouter, this kind
//! turns on the other source: the final object of every reply carries
//! `prompt_eval_count` and `eval_count`, which is the chat's exact size as the
//! backend counted it (see `Provider::usage_in`). Between replies the status
//! row shows the `chars_per_token` estimate (`~`).
//!
//! The model list (`GET /api/tags`) fills the model picker, and `GET /api/ps`
//! (the model that is loaded, with the context length it was loaded with) is
//! what `t` shows and what `c` copies into *Context size*.

use super::{
    json_strings, text_of, value_of, Info, ModelEntry, ModelSource, ParamDef, ParamKind, Piece,
    Preset, Provider, Usage,
};
use serde_json::{json, Map, Value};

pub struct Ollama;

const fn slider(min: f64, max: f64, step: f64, default: f64) -> ParamKind {
    ParamKind::Slider {
        min,
        max,
        step,
        default,
    }
}

const fn p(
    key: &'static str,
    label: &'static str,
    group: &'static str,
    help: &'static str,
    kind: ParamKind,
) -> ParamDef {
    ParamDef {
        key,
        label,
        group,
        help,
        kind,
    }
}

const G_GEN: &str = "Generation";
const G_SAMPLING: &str = "Sampling";
const G_REPEAT: &str = "Repetition";
const G_ADV: &str = "Advanced";

impl Ollama {
    /// A whole-number setting of the connection (or `fallback`).
    fn int(&self, params: &Map<String, Value>, key: &str, fallback: i64) -> i64 {
        self.params()
            .iter()
            .find(|d| d.key == key)
            .and_then(|d| value_of(params, d).as_f64())
            .map(|x| x.round() as i64)
            .unwrap_or(fallback)
    }
}

/// `5m`, `1h30m`, `300ms`: what Go's `time.ParseDuration` reads (and Ollama's
/// `keep_alive` takes as text).
fn is_duration(s: &str) -> bool {
    let s = s
        .strip_prefix('-')
        .or_else(|| s.strip_prefix('+'))
        .unwrap_or(s);
    if s.is_empty() {
        return false;
    }
    let mut rest = s;
    while !rest.is_empty() {
        let num_end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        if num_end == 0 {
            return false;
        }
        let tail = &rest[num_end..];
        let unit_end = tail
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(tail.len());
        let (unit, next) = tail.split_at(unit_end);
        if !["ns", "us", "\u{b5}s", "ms", "s", "m", "h"].contains(&unit) {
            return false;
        }
        rest = next;
    }
    true
}

/// `4.7 GB` / `640 MB`, the way Ollama itself writes sizes.
fn size_label(bytes: u64) -> String {
    let gb = bytes as f64 / 1_000_000_000.0;
    if gb >= 1.0 {
        format!("{:.1} GB", gb)
    } else {
        format!("{:.0} MB", bytes as f64 / 1_000_000.0)
    }
}

impl Provider for Ollama {
    fn kind(&self) -> &'static str {
        "ollama"
    }

    fn label(&self) -> &'static str {
        "Ollama"
    }

    fn default_url(&self) -> &'static str {
        "http://127.0.0.1:11434"
    }

    /// SillyTavern's name for it.
    fn textgen_type(&self) -> &'static str {
        "ollama"
    }

    fn params(&self) -> Vec<ParamDef> {
        vec![
            // ---- Generation
            p("model", "Model", G_GEN,
              "Press Enter to pick one of the models installed in Ollama (type to filter), e.g. llama3.1:8b. Ollama needs one.",
              ParamKind::Text { default: "" }),
            p("num_predict", "Max reply tokens", G_GEN,
              "Longest reply the model may write.",
              slider(1.0, 8192.0, 1.0, 350.0)),
            p("max_context", "Context size", G_GEN,
              "Tokens of chat history sent, and the context window asked of Ollama (num_ctx). A longer prompt is cut at the start by Ollama, so keep it within what the model and your memory can hold.",
              slider(512.0, 262144.0, 256.0, 8192.0)),
            p("stream", "Stream reply", G_GEN,
              "Show the reply as it is written.",
              ParamKind::Toggle { default: true }),
            p("seed", "Seed", G_GEN,
              "-1 = random each time (not sent).",
              ParamKind::Number { default: -1 }),
            p("stop_strings", "Extra stop strings", G_GEN,
              "JSON list, e.g. [\"\\n\\n\", \"OOC:\"]. Added to the stops of the instruct template.",
              ParamKind::Text { default: "[]" }),
            // ---- Sampling
            p("temperature", "Temperature", G_SAMPLING,
              "Higher = more random, lower = more predictable (Ollama's default: 0.8).",
              slider(0.0, 5.0, 0.01, 0.8)),
            p("top_p", "Top P", G_SAMPLING,
              "Keep the smallest set of tokens whose probabilities add up to P. 1 = off (Ollama's default: 0.9).",
              slider(0.0, 1.0, 0.01, 0.9)),
            p("top_k", "Top K", G_SAMPLING,
              "Only the K likeliest tokens are considered (Ollama's default: 40).",
              slider(1.0, 200.0, 1.0, 40.0)),
            p("min_p", "Min P", G_SAMPLING,
              "Drop tokens below P times the likeliest one's probability. 0 = off.",
              slider(0.0, 1.0, 0.01, 0.0)),
            p("typical_p", "Typical P", G_SAMPLING,
              "Locally typical sampling. 1 = off (not sent).",
              slider(0.0, 1.0, 0.01, 1.0)),
            // ---- Repetition
            p("repeat_penalty", "Repeat penalty", G_REPEAT,
              "Above 1 discourages repeating recent tokens (Ollama's default: 1.1).",
              slider(1.0, 2.0, 0.01, 1.1)),
            p("repeat_last_n", "Repeat window", G_REPEAT,
              "How many recent tokens the repeat penalty looks at. 0 = off, -1 = the whole context (Ollama's default: 64).",
              slider(-1.0, 4096.0, 1.0, 64.0)),
            p("frequency_penalty", "Frequency penalty", G_REPEAT,
              "Punishes tokens by how often they were already used. 0 = off.",
              slider(-2.0, 2.0, 0.01, 0.0)),
            p("presence_penalty", "Presence penalty", G_REPEAT,
              "Punishes tokens that were already used at all. 0 = off.",
              slider(-2.0, 2.0, 0.01, 0.0)),
            // ---- Advanced
            p("keep_alive", "Keep model loaded", G_ADV,
              "How long Ollama keeps the model in memory after a reply: 5m, 1h, -1 (for ever), 0 (unload at once). Empty = Ollama's own setting (5m).",
              ParamKind::Text { default: "" }),
            p("extra_options", "Extra options", G_ADV,
              "A JSON object merged into the request's `options`, e.g. {\"num_gpu\": 20, \"mirostat\": 2}. For settings this list does not have.",
              ParamKind::Text { default: "" }),
            p("extra_body", "Extra request fields", G_ADV,
              "A JSON object merged into the request itself, e.g. {\"think\": false}. (`prompt` and `options` cannot be replaced here.)",
              ParamKind::Text { default: "" }),
        ]
    }

    fn presets(&self) -> Vec<Preset> {
        // Same idea as the other kinds: a preset sets every sampler setting
        // (Sampling and Repetition); what it does not name goes back to its
        // default. Generation and Advanced settings are not touched.
        let sampler_groups = [G_SAMPLING, G_REPEAT];
        let make = |name: &'static str, help: &'static str, set: &[(&'static str, f64)]| {
            let values = self
                .params()
                .iter()
                .filter(|d| sampler_groups.contains(&d.group))
                .map(|d| match set.iter().find(|(k, _)| *k == d.key) {
                    Some((_, x)) => (d.key, d.snap(*x)),
                    None => (d.key, d.default_value()),
                })
                .collect();
            Preset { name, help, values }
        };
        vec![
            make(
                "Normal",
                "balanced and reliable",
                &[
                    ("temperature", 0.8),
                    ("top_p", 0.95),
                    ("top_k", 40.0),
                    ("min_p", 0.05),
                    ("repeat_penalty", 1.05),
                ],
            ),
            make(
                "Creative",
                "more varied wording",
                &[
                    ("temperature", 1.15),
                    ("top_p", 0.98),
                    ("top_k", 80.0),
                    ("min_p", 0.04),
                    ("repeat_penalty", 1.05),
                ],
            ),
            make(
                "Crazy",
                "wild and unpredictable (high temperature)",
                &[
                    ("temperature", 1.8),
                    ("top_p", 1.0),
                    ("top_k", 200.0),
                    ("min_p", 0.02),
                    ("repeat_penalty", 1.1),
                ],
            ),
        ]
    }

    fn limits(&self, params: &Map<String, Value>) -> (i64, i64) {
        (
            self.int(params, "max_context", 8192),
            self.int(params, "num_predict", 350),
        )
    }

    fn streaming(&self, params: &Map<String, Value>) -> bool {
        self.params()
            .iter()
            .find(|d| d.key == "stream")
            .map(|d| value_of(params, d).as_bool().unwrap_or(true))
            .unwrap_or(true)
    }

    /// Ollama has no tokenizer route: the count comes from `usage_in`.
    fn tokenizes(&self, _params: &Map<String, Value>) -> bool {
        false
    }

    fn validate_text(&self, key: &str, text: &str) -> Result<(), String> {
        let t = text.trim();
        match key {
            "stop_strings" => {
                if t.is_empty() {
                    return Ok(());
                }
                serde_json::from_str::<Vec<String>>(t)
                    .map(|_| ())
                    .map_err(|_| "must be a JSON list of strings, e.g. [\"a\", \"b\"]".to_string())
            }
            "extra_options" | "extra_body" => {
                if t.is_empty() {
                    return Ok(());
                }
                match serde_json::from_str::<Value>(t) {
                    Ok(Value::Object(_)) => Ok(()),
                    _ => Err("must be a JSON object, e.g. {\"num_gpu\": 20}".to_string()),
                }
            }
            "keep_alive" => {
                let number = matches!(t.parse::<f64>(), Ok(n) if n.is_finite());
                if t.is_empty() || number || is_duration(t) {
                    Ok(())
                } else {
                    Err("a time such as 5m or 1h, a number of seconds, or -1".to_string())
                }
            }
            _ => Ok(()),
        }
    }

    fn generate_path(&self) -> &'static str {
        "/api/generate"
    }

    fn build_body(&self, params: &Map<String, Value>, prompt: &str, stop: &[String]) -> Value {
        let defs = self.params();
        let val = |key: &str| -> Value {
            defs.iter()
                .find(|d| d.key == key)
                .map(|d| value_of(params, d))
                .unwrap_or(Value::Null)
        };
        let num = |key: &str| -> f64 { val(key).as_f64().unwrap_or(0.0) };

        // Everything but the prompt, the model and the stream switch goes
        // into `options`. The context window is always asked for: left out,
        // Ollama uses its own (small) default and cuts a longer prompt.
        let mut o: Map<String, Value> = Map::new();
        o.insert("num_ctx".to_string(), val("max_context"));
        o.insert("num_predict".to_string(), val("num_predict"));
        o.insert("temperature".to_string(), val("temperature"));
        o.insert("top_p".to_string(), val("top_p"));
        o.insert("top_k".to_string(), val("top_k"));
        o.insert("min_p".to_string(), val("min_p"));
        o.insert("repeat_penalty".to_string(), val("repeat_penalty"));
        o.insert("repeat_last_n".to_string(), val("repeat_last_n"));
        o.insert("frequency_penalty".to_string(), val("frequency_penalty"));
        o.insert("presence_penalty".to_string(), val("presence_penalty"));
        if num("typical_p") < 1.0 {
            o.insert("typical_p".to_string(), val("typical_p"));
        }
        if num("seed") >= 0.0 {
            o.insert("seed".to_string(), val("seed"));
        }

        let mut stops: Vec<String> = stop.to_vec();
        for s in json_strings(&text_of(&val("stop_strings"))) {
            if !s.is_empty() && !stops.contains(&s) {
                stops.push(s);
            }
        }
        if !stops.is_empty() {
            o.insert("stop".to_string(), json!(stops));
        }

        // Whatever else the person wants in `options`; it wins over the above.
        if let Ok(Value::Object(extra)) =
            serde_json::from_str::<Value>(text_of(&val("extra_options")).trim())
        {
            for (k, v) in extra {
                o.insert(k, v);
            }
        }

        let model = text_of(&val("model"));
        let mut m: Map<String, Value> = Map::new();
        m.insert("model".to_string(), json!(model.trim()));
        m.insert("prompt".to_string(), json!(prompt));
        // tarpi's own formatting: no template of the model's on top of it.
        m.insert("raw".to_string(), json!(true));
        m.insert("stream".to_string(), val("stream"));

        // A number is seconds (-1: for ever), text is a duration (`5m`).
        let keep = text_of(&val("keep_alive"));
        let keep = keep.trim();
        if !keep.is_empty() {
            match keep.parse::<f64>() {
                Ok(n) if n.is_finite() => m.insert("keep_alive".to_string(), json!(n)),
                _ => m.insert("keep_alive".to_string(), json!(keep)),
            };
        }
        m.insert("options".to_string(), Value::Object(o));

        if let Ok(Value::Object(extra)) =
            serde_json::from_str::<Value>(text_of(&val("extra_body")).trim())
        {
            for (k, v) in extra {
                if k != "prompt" && k != "options" {
                    m.insert(k, v);
                }
            }
        }
        Value::Object(m)
    }

    fn parse_stream_line(&self, line: &str) -> Piece {
        // One JSON object per line; anything else is not of interest.
        let line = line.trim();
        if !line.starts_with('{') {
            return Piece::Skip;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return Piece::Skip,
        };
        if let Some(e) = self.error_in(&v) {
            return Piece::Error(e);
        }
        let text = v.get("response").and_then(|t| t.as_str()).unwrap_or("");
        if !text.is_empty() {
            return Piece::Text(text.to_string());
        }
        // The last object: no more text, `done`, and the token counts.
        if v.get("done").and_then(|d| d.as_bool()).unwrap_or(false) {
            return Piece::Done;
        }
        Piece::Skip
    }

    fn parse_full(&self, body: &Value) -> Option<String> {
        body.get("response")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
    }

    fn error_in(&self, body: &Value) -> Option<String> {
        // {"error": "model \"x\" not found, try pulling it first"}
        match body.get("error")? {
            Value::Null | Value::Bool(false) => None,
            Value::String(s) if s.trim().is_empty() => None,
            Value::String(s) => Some(s.clone()),
            e => Some(
                e.get("message")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| e.to_string()),
            ),
        }
    }

    /// Ollama's own names for the counts, on the last object of a reply:
    /// `prompt_eval_count` is the whole prompt as the model's tokenizer saw
    /// it (the part Ollama took from its cache included in current versions;
    /// an older one leaves the field out when the prompt was fully cached,
    /// and then there is no count, only the estimate).
    fn usage_in(&self, body: &Value) -> Option<Usage> {
        let prompt = body.get("prompt_eval_count").and_then(|n| n.as_u64())? as usize;
        if prompt == 0 {
            return None;
        }
        let completion = body
            .get("eval_count")
            .and_then(|n| n.as_u64())
            .unwrap_or(0) as usize;
        Some(Usage {
            prompt_tokens: prompt,
            completion_tokens: completion,
        })
    }

    fn info_paths(&self) -> &'static [&'static str] {
        // `/api/ps`: the loaded model (older servers do not have it, then the
        // list of installed models says at least that the server is up).
        &["/api/ps", "/api/tags"]
    }

    fn parse_info(&self, body: &Value) -> Info {
        let first = body.pointer("/models/0");
        // Only a model that is loaded (it has an expiry time) is "the model":
        // the first of the installed ones is not.
        let loaded = first.filter(|m| m.get("expires_at").is_some());
        let Some(m) = loaded else {
            return Info::default();
        };
        let model = m
            .get("name")
            .or_else(|| m.get("model"))
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        // The context length it was loaded with: what `c` copies, so that the
        // next reply does not make Ollama load it again.
        let n_ctx = m
            .get("context_length")
            .and_then(|n| n.as_i64())
            .filter(|n| *n > 0);
        Info { model, n_ctx }
    }

    /// Unused while `tokenizes` is false (Ollama has no such route).
    fn tokenize_request(&self, text: &str) -> (&'static str, Value) {
        ("/api/tokenize", json!({ "text": text }))
    }

    fn parse_token_count(&self, _body: &Value) -> Option<usize> {
        None
    }

    fn model_source(&self) -> Option<ModelSource> {
        Some(ModelSource {
            param: "model",
            path: "/api/tags",
            // Sent when a key is set (a proxy in front of Ollama, Ollama's
            // cloud); a local one has none and gets nothing.
            needs_key: true,
        })
    }

    fn parse_models(&self, body: &Value) -> Vec<ModelEntry> {
        let Some(list) = body.get("models").and_then(|d| d.as_array()) else {
            return Vec::new();
        };
        list.iter()
            .filter_map(|m| {
                let id = m
                    .get("name")
                    .or_else(|| m.get("model"))
                    .and_then(|s| s.as_str())?
                    .trim();
                if id.is_empty() {
                    return None;
                }
                let details = m.get("details");
                let text_of_detail = |key: &str| -> String {
                    details
                        .and_then(|d| d.get(key))
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string()
                };
                // `7.6B · Q4_K_M · 4.7 GB`, shown beside the name.
                let mut bits: Vec<String> = Vec::new();
                for key in ["parameter_size", "quantization_level"] {
                    let s = text_of_detail(key);
                    if !s.is_empty() {
                        bits.push(s);
                    }
                }
                if let Some(n) = m.get("size").and_then(|n| n.as_u64()) {
                    if n > 0 {
                        bits.push(size_label(n));
                    }
                }
                Some(ModelEntry {
                    id: id.to_string(),
                    name: text_of_detail("family"),
                    context: None,
                    price: bits.join(" \u{b7} "),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Map<String, Value> {
        Map::new()
    }

    #[test]
    fn body_is_raw_and_always_asks_for_the_context_window() {
        let b = Ollama.build_body(&defaults(), "hello", &["\nYou:".to_string()]);
        assert_eq!(b["prompt"], "hello");
        assert_eq!(b["raw"], true);
        assert_eq!(b["stream"], true);
        assert_eq!(b["model"], "");
        let o = &b["options"];
        assert_eq!(o["num_ctx"], 8192);
        assert_eq!(o["num_predict"], 350);
        assert_eq!(o["temperature"], 0.8);
        assert_eq!(o["top_p"], 0.9);
        assert_eq!(o["top_k"], 40);
        assert_eq!(o["min_p"], 0.0);
        assert_eq!(o["repeat_penalty"], 1.1);
        assert_eq!(o["repeat_last_n"], 64);
        assert_eq!(o["stop"], json!(["\nYou:"]));
        // off values and tarpi-only settings stay out
        for k in ["typical_p", "seed", "max_context", "keep_alive", "stream", "model"] {
            assert!(o.get(k).is_none(), "{} is not an option", k);
        }
        for k in ["keep_alive", "num_ctx", "num_predict", "temperature"] {
            assert!(b.get(k).is_none(), "{} belongs in options or is off", k);
        }
    }

    #[test]
    fn no_stops_means_no_stop_field() {
        let b = Ollama.build_body(&defaults(), "x", &[]);
        assert!(b["options"].get("stop").is_none());
    }

    #[test]
    fn switched_on_fields_are_sent() {
        let mut p = Map::new();
        p.insert("model".into(), json!("  llama3.1:8b "));
        p.insert("max_context".into(), json!(16384));
        p.insert("typical_p".into(), json!(0.9));
        p.insert("seed".into(), json!(42));
        p.insert("stream".into(), json!(false));
        let b = Ollama.build_body(&p, "x", &[]);
        assert_eq!(b["model"], "llama3.1:8b");
        assert_eq!(b["stream"], false);
        assert_eq!(b["options"]["num_ctx"], 16384);
        assert_eq!(b["options"]["typical_p"], 0.9);
        assert_eq!(b["options"]["seed"], 42);
        // seed 0 is a seed, only -1 means random
        p.insert("seed".into(), json!(0));
        assert_eq!(Ollama.build_body(&p, "x", &[]).pointer("/options/seed"), Some(&json!(0)));
    }

    #[test]
    fn stop_strings_are_added_once() {
        let mut p = Map::new();
        p.insert("stop_strings".into(), json!("[\"OOC:\", \"\\nYou:\"]"));
        let b = Ollama.build_body(&p, "x", &["\nYou:".to_string()]);
        assert_eq!(b["options"]["stop"], json!(["\nYou:", "OOC:"]));
    }

    #[test]
    fn extra_options_win_and_extra_fields_cannot_replace_prompt_or_options() {
        let mut p = Map::new();
        p.insert(
            "extra_options".into(),
            json!("{\"num_gpu\": 20, \"temperature\": 2.0}"),
        );
        p.insert(
            "extra_body".into(),
            json!("{\"think\": false, \"prompt\": \"evil\", \"options\": {}}"),
        );
        let b = Ollama.build_body(&p, "real", &[]);
        assert_eq!(b["options"]["num_gpu"], 20);
        assert_eq!(b["options"]["temperature"], 2.0);
        assert_eq!(b["options"]["num_ctx"], 8192);
        assert_eq!(b["think"], false);
        assert_eq!(b["prompt"], "real");
        // not objects: ignored
        p.insert("extra_options".into(), json!("[1]"));
        p.insert("extra_body".into(), json!("nope"));
        let b = Ollama.build_body(&p, "real", &[]);
        assert!(b["options"].get("num_gpu").is_none());
        assert!(b.get("think").is_none());
    }

    #[test]
    fn keep_alive_is_a_number_or_a_duration() {
        let mut p = Map::new();
        p.insert("keep_alive".into(), json!("30m"));
        assert_eq!(Ollama.build_body(&p, "x", &[])["keep_alive"], "30m");
        p.insert("keep_alive".into(), json!("-1"));
        assert_eq!(Ollama.build_body(&p, "x", &[])["keep_alive"], -1.0);
        p.insert("keep_alive".into(), json!("0"));
        assert_eq!(Ollama.build_body(&p, "x", &[])["keep_alive"], 0.0);
        p.insert("keep_alive".into(), json!("  "));
        assert!(Ollama.build_body(&p, "x", &[]).get("keep_alive").is_none());
    }

    #[test]
    fn durations_are_checked() {
        for ok in ["5m", "1h30m", "300ms", "1.5h", "-1m", "10s"] {
            assert!(is_duration(ok), "{}", ok);
        }
        for bad in ["", "m", "5", "5x", "h1", "1h 30m", "-"] {
            assert!(!is_duration(bad), "{}", bad);
        }
        assert!(Ollama.validate_text("keep_alive", "").is_ok());
        assert!(Ollama.validate_text("keep_alive", "-1").is_ok());
        assert!(Ollama.validate_text("keep_alive", "3600").is_ok());
        assert!(Ollama.validate_text("keep_alive", "20m").is_ok());
        assert!(Ollama.validate_text("keep_alive", "soon").is_err());
        assert!(Ollama.validate_text("keep_alive", "nan").is_err());
        assert!(Ollama.validate_text("keep_alive", "inf").is_err());
    }

    #[test]
    fn text_settings_are_checked() {
        assert!(Ollama.validate_text("stop_strings", "").is_ok());
        assert!(Ollama.validate_text("stop_strings", "[\"a\"]").is_ok());
        assert!(Ollama.validate_text("stop_strings", "a").is_err());
        for k in ["extra_options", "extra_body"] {
            assert!(Ollama.validate_text(k, "").is_ok());
            assert!(Ollama.validate_text(k, "{\"a\": 1}").is_ok());
            assert!(Ollama.validate_text(k, "[1]").is_err());
            assert!(Ollama.validate_text(k, "nope").is_err());
        }
        assert!(Ollama.validate_text("model", "anything").is_ok());
    }

    #[test]
    fn token_counts_come_from_the_last_object_of_a_reply() {
        let last = json!({
            "model": "llama3.1", "response": "", "done": true, "done_reason": "stop",
            "prompt_eval_count": 812, "eval_count": 40, "total_duration": 5
        });
        let u = Ollama.usage_in(&last).unwrap();
        assert_eq!((u.prompt_tokens, u.completion_tokens), (812, 40));
        // a whole (not streamed) reply has them too
        assert!(Ollama.usage_in(&json!({"response": "Hi", "done": true, "prompt_eval_count": 5, "eval_count": 2})).is_some());
        // a chunk of text, a missing or empty prompt count: nothing to report
        assert!(Ollama.usage_in(&json!({"response": "Hi", "done": false})).is_none());
        assert!(Ollama.usage_in(&json!({"done": true, "eval_count": 9})).is_none());
        assert!(Ollama.usage_in(&json!({"done": true, "prompt_eval_count": 0})).is_none());
        // the completion count is optional
        assert_eq!(Ollama.usage_in(&json!({"prompt_eval_count": 5})).unwrap().completion_tokens, 0);
        // the OpenAI spelling is not Ollama's
        assert!(Ollama.usage_in(&json!({"usage": {"prompt_tokens": 5}})).is_none());
    }

    #[test]
    fn there_is_no_tokenizer_to_ask() {
        assert!(!Ollama.tokenizes(&defaults()));
        assert_eq!(Ollama.parse_token_count(&json!({"tokens": [1, 2]})), None);
    }

    #[test]
    fn stream_lines() {
        let l = Ollama;
        assert!(matches!(l.parse_stream_line(""), Piece::Skip));
        assert!(matches!(l.parse_stream_line("not json"), Piece::Skip));
        assert!(matches!(l.parse_stream_line("{broken"), Piece::Skip));
        assert!(matches!(
            l.parse_stream_line("{\"model\":\"m\",\"created_at\":\"t\",\"response\":\"Hi\",\"done\":false}"),
            Piece::Text(t) if t == "Hi"
        ));
        // an empty piece of text is nothing
        assert!(matches!(
            l.parse_stream_line("{\"response\":\"\",\"done\":false}"),
            Piece::Skip
        ));
        // text and `done` together: the text counts
        assert!(matches!(
            l.parse_stream_line("{\"response\":\".\",\"done\":true}"),
            Piece::Text(t) if t == "."
        ));
        assert!(matches!(
            l.parse_stream_line("{\"response\":\"\",\"done\":true,\"done_reason\":\"stop\",\"prompt_eval_count\":9,\"eval_count\":3}"),
            Piece::Done
        ));
        assert!(matches!(
            l.parse_stream_line("{\"error\":\"model \\\"x\\\" not found, try pulling it first\"}"),
            Piece::Error(e) if e.contains("not found")
        ));
        // SSE is not what Ollama speaks
        assert!(matches!(l.parse_stream_line("data: {\"response\":\"Hi\"}"), Piece::Skip));
    }

    #[test]
    fn full_reply_and_errors() {
        let v = json!({"response": "Hello there", "done": true});
        assert_eq!(Ollama.parse_full(&v).as_deref(), Some("Hello there"));
        assert_eq!(Ollama.parse_full(&json!({})), None);
        assert_eq!(Ollama.error_in(&v), None);
        assert_eq!(Ollama.error_in(&json!({"error": "nope"})).as_deref(), Some("nope"));
        assert_eq!(
            Ollama.error_in(&json!({"error": {"message": "bad key"}})).as_deref(),
            Some("bad key")
        );
        assert_eq!(Ollama.error_in(&json!({"error": ""})), None);
        assert_eq!(Ollama.error_in(&json!({"error": null})), None);
    }

    #[test]
    fn info_is_the_loaded_model() {
        let ps = json!({"models": [
            {"name": "llama3.1:8b", "model": "llama3.1:8b", "expires_at": "2026-11-11T10:00:00Z",
             "size_vram": 5, "context_length": 8192}
        ]});
        let i = Ollama.parse_info(&ps);
        assert_eq!(i.model, "llama3.1:8b");
        assert_eq!(i.n_ctx, Some(8192));
        // an older server: no context length
        let i = Ollama.parse_info(&json!({"models": [{"name": "m", "expires_at": "x"}]}));
        assert_eq!((i.model.as_str(), i.n_ctx), ("m", None));
        // nothing loaded, or the list of installed models: up, but no model
        let i = Ollama.parse_info(&json!({"models": []}));
        assert_eq!((i.model.as_str(), i.n_ctx), ("", None));
        let tags = json!({"models": [{"name": "a:latest", "modified_at": "x", "size": 1}]});
        let i = Ollama.parse_info(&tags);
        assert_eq!((i.model.as_str(), i.n_ctx), ("", None));
        let i = Ollama.parse_info(&Value::Null);
        assert_eq!((i.model.as_str(), i.n_ctx), ("", None));
    }

    #[test]
    fn model_list() {
        let tags = json!({"models": [
            {"name": "llama3.1:8b", "model": "llama3.1:8b", "size": 4_920_000_000u64,
             "details": {"family": "llama", "parameter_size": "8.0B", "quantization_level": "Q4_K_M"}},
            {"name": "tiny:latest", "size": 640_000_000u64},
            {"name": "  "},
            {"nothing": true}
        ]});
        let list = Ollama.parse_models(&tags);
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].id, "llama3.1:8b");
        assert_eq!(list[0].name, "llama");
        assert_eq!(list[0].price, "8.0B \u{b7} Q4_K_M \u{b7} 4.9 GB");
        assert_eq!(list[0].context, None);
        assert_eq!(list[1].id, "tiny:latest");
        assert_eq!(list[1].price, "640 MB");
        assert!(Ollama.parse_models(&json!({})).is_empty());
        let src = Ollama.model_source().unwrap();
        assert_eq!((src.param, src.path), ("model", "/api/tags"));
        assert!(Ollama.params().iter().any(|d| d.key == src.param));
    }

    #[test]
    fn presets_cover_every_sampler_and_are_on_the_grid() {
        let presets = Ollama.presets();
        let names: Vec<&str> = presets.iter().map(|p| p.name).collect();
        assert_eq!(names, ["Normal", "Creative", "Crazy"]);
        let n = presets[0].values.len();
        assert_eq!(n, 9);
        for p in &presets {
            assert_eq!(p.values.len(), n);
            for (k, v) in &p.values {
                let d = Ollama.params().into_iter().find(|d| d.key == *k).unwrap();
                if let Some(x) = v.as_f64() {
                    assert_eq!(&d.snap(x), v, "{} {}", p.name, k);
                }
            }
        }
        let get = |p: &Preset, k: &str| p.values.iter().find(|(x, _)| *x == k).unwrap().1.clone();
        assert_eq!(get(&presets[1], "temperature"), json!(1.15));
        assert_eq!(get(&presets[2], "top_k"), json!(200));
        assert_eq!(get(&presets[0], "frequency_penalty"), json!(0.0));
        assert_eq!(get(&presets[0], "repeat_last_n"), json!(64));
    }

    #[test]
    fn limits_and_streaming() {
        let mut p = Map::new();
        assert_eq!(Ollama.limits(&p), (8192, 350));
        assert!(Ollama.streaming(&p));
        p.insert("max_context".into(), json!(16384));
        p.insert("num_predict".into(), json!(500));
        p.insert("stream".into(), json!(false));
        assert_eq!(Ollama.limits(&p), (16384, 500));
        assert!(!Ollama.streaming(&p));
    }

    #[test]
    fn a_summary_gets_its_own_reply_length() {
        let c = super::super::Connection::new("ollama", "x").unwrap();
        assert_eq!(c.url, "http://127.0.0.1:11434");
        let s = super::super::for_summary(&c, 777);
        assert_eq!(s.params["num_predict"], 777);
        assert_eq!(s.params["repeat_penalty"], 1.0);
        let b = Ollama.build_body(&s.resolved(), "x", &[]);
        assert_eq!(b["options"]["num_predict"], 777);
        assert_eq!(b["options"]["temperature"], 0.5);
    }

    #[test]
    fn keys_are_unique() {
        let defs = Ollama.params();
        for (i, a) in defs.iter().enumerate() {
            for b in &defs[i + 1..] {
                assert_ne!(a.key, b.key);
            }
        }
    }

    #[test]
    fn defaults_are_valid_for_their_controls() {
        for d in Ollama.params() {
            if let ParamKind::Slider { min, max, default, .. } = d.kind {
                assert!(default >= min && default <= max, "{}", d.key);
            }
        }
    }
}
