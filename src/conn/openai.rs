//! Any server that speaks the OpenAI *text completions* API: `POST
//! /v1/completions`, with `GET /v1/models` to see whether it is up. vLLM,
//! LM Studio, KoboldCpp, TabbyAPI, text-generation-webui, llama.cpp's
//! `/v1` routes, OpenRouter and the like all answer it.
//!
//! tarpi builds the whole prompt itself (context template, instruct template,
//! examples, history), so it is sent as one text `prompt`, not as chat
//! `messages`: the chat completions endpoint is not used.
//!
//! The standard settings (temperature, top P, penalties, ...) are always
//! sent. The ones only some servers know (top K, min P, typical P, the
//! repetition penalty, seed, ignore EOS) are left out while they hold their
//! "off" value, so a strict server never sees a field it would refuse. Anything
//! else a server takes can be added with *Extra request fields*.

use super::{
    json_strings, text_of, value_of, Info, ParamDef, ParamKind, Piece, Preset, Provider,
};
use serde_json::{json, Map, Value};

pub struct OpenAi;

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

impl Provider for OpenAi {
    fn kind(&self) -> &'static str {
        "openai"
    }

    fn label(&self) -> &'static str {
        "OpenAI-compatible"
    }

    fn default_url(&self) -> &'static str {
        "http://127.0.0.1:8000"
    }

    /// SillyTavern's name for "Generic (OpenAI-compatible)".
    fn textgen_type(&self) -> &'static str {
        "generic"
    }

    fn params(&self) -> Vec<ParamDef> {
        vec![
            // ---- Generation
            p("max_tokens", "Max reply tokens", G_GEN,
              "Longest reply the model may write.",
              slider(1.0, 8192.0, 1.0, 350.0)),
            p("max_context", "Context size", G_GEN,
              "Tokens of chat history sent. Keep at or below the model's context length.",
              slider(512.0, 262144.0, 256.0, 8192.0)),
            p("stream", "Stream reply", G_GEN,
              "Show the reply as it is written.",
              ParamKind::Toggle { default: true }),
            p("seed", "Seed", G_GEN,
              "-1 = random each time (not sent).",
              ParamKind::Number { default: -1 }),
            p("ignore_eos", "Ban EOS token", G_GEN,
              "Never let the model end its reply by itself. Sent only when on, and only some servers know it.",
              ParamKind::Toggle { default: false }),
            p("stop_strings", "Extra stop strings", G_GEN,
              "JSON list, e.g. [\"\\n\\n\", \"OOC:\"]. Added to the stops of the instruct template.",
              ParamKind::Text { default: "[]" }),
            // ---- Sampling
            p("temperature", "Temperature", G_SAMPLING,
              "Higher = more random, lower = more predictable.",
              slider(0.0, 5.0, 0.01, 0.8)),
            p("top_p", "Top P", G_SAMPLING,
              "Keep the smallest set of tokens whose probabilities add up to P. 1 = off.",
              slider(0.0, 1.0, 0.01, 1.0)),
            p("top_k", "Top K", G_SAMPLING,
              "Only the K likeliest tokens are considered. 0 = off (not sent).",
              slider(0.0, 200.0, 1.0, 0.0)),
            p("min_p", "Min P", G_SAMPLING,
              "Drop tokens below P times the likeliest one's probability. 0 = off (not sent).",
              slider(0.0, 1.0, 0.01, 0.0)),
            p("typical_p", "Typical P", G_SAMPLING,
              "Locally typical sampling. 1 = off (not sent).",
              slider(0.0, 1.0, 0.01, 1.0)),
            // ---- Repetition
            p("repetition_penalty", "Repetition penalty", G_REPEAT,
              "Above 1 discourages repeating recent tokens. 1 = off (not sent). Sent as repetition_penalty and repeat_penalty.",
              slider(1.0, 2.0, 0.01, 1.0)),
            p("frequency_penalty", "Frequency penalty", G_REPEAT,
              "Punishes tokens by how often they were already used. 0 = off (not sent).",
              slider(-2.0, 2.0, 0.01, 0.0)),
            p("presence_penalty", "Presence penalty", G_REPEAT,
              "Punishes tokens that were already used at all. 0 = off (not sent).",
              slider(-2.0, 2.0, 0.01, 0.0)),
            // ---- Advanced
            p("model", "Model name", G_ADV,
              "Sent as `model` with every request. OpenAI, vLLM, OpenRouter and LM Studio want it; many local servers ignore it. Empty = not sent.",
              ParamKind::Text { default: "" }),
            p("extra_body", "Extra request fields", G_ADV,
              "A JSON object merged into every request, e.g. {\"xtc_probability\": 0.5}. For settings this list does not have.",
              ParamKind::Text { default: "" }),
            p("use_tokenizer", "Count tokens on the server", G_ADV,
              "Ask POST /tokenize for exact token counts (vLLM and llama.cpp have it; OpenAI does not). Off = estimated from the text length.",
              ParamKind::Toggle { default: false }),
        ]
    }

    fn presets(&self) -> Vec<Preset> {
        // A preset sets every sampler setting (Sampling and Repetition): what
        // it does not name goes back to its default, so switching between
        // presets never leaves anything behind. Generation and Advanced
        // settings are not touched.
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
                    ("repetition_penalty", 1.05),
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
                    ("repetition_penalty", 1.05),
                ],
            ),
            make(
                "Crazy",
                "wild and unpredictable (high temperature)",
                &[
                    ("temperature", 1.8),
                    ("top_p", 1.0),
                    ("top_k", 0.0),
                    ("min_p", 0.02),
                    ("repetition_penalty", 1.1),
                ],
            ),
        ]
    }

    fn limits(&self, params: &Map<String, Value>) -> (i64, i64) {
        let defs = self.params();
        let get = |key: &str, fallback: i64| -> i64 {
            defs.iter()
                .find(|d| d.key == key)
                .and_then(|d| value_of(params, d).as_f64())
                .map(|x| x.round() as i64)
                .unwrap_or(fallback)
        };
        (get("max_context", 8192), get("max_tokens", 350))
    }

    fn streaming(&self, params: &Map<String, Value>) -> bool {
        self.params()
            .iter()
            .find(|d| d.key == "stream")
            .map(|d| value_of(params, d).as_bool().unwrap_or(true))
            .unwrap_or(true)
    }

    fn tokenizes(&self, params: &Map<String, Value>) -> bool {
        self.params()
            .iter()
            .find(|d| d.key == "use_tokenizer")
            .map(|d| value_of(params, d).as_bool().unwrap_or(false))
            .unwrap_or(false)
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
            "extra_body" => {
                if t.is_empty() {
                    return Ok(());
                }
                match serde_json::from_str::<Value>(t) {
                    Ok(Value::Object(_)) => Ok(()),
                    _ => Err("must be a JSON object, e.g. {\"top_a\": 0.1}".to_string()),
                }
            }
            _ => Ok(()),
        }
    }

    fn generate_path(&self) -> &'static str {
        "/v1/completions"
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

        let mut m: Map<String, Value> = Map::new();
        m.insert("prompt".to_string(), json!(prompt));
        let model = text_of(&val("model"));
        if !model.trim().is_empty() {
            m.insert("model".to_string(), json!(model.trim()));
        }
        // Standard fields: always sent.
        m.insert("max_tokens".to_string(), val("max_tokens"));
        m.insert("stream".to_string(), val("stream"));
        m.insert("temperature".to_string(), val("temperature"));
        m.insert("top_p".to_string(), val("top_p"));
        // Fields only some servers know: sent once they are switched on.
        if num("top_k") > 0.0 {
            m.insert("top_k".to_string(), val("top_k"));
        }
        if num("min_p") > 0.0 {
            m.insert("min_p".to_string(), val("min_p"));
        }
        if num("typical_p") < 1.0 {
            m.insert("typical_p".to_string(), val("typical_p"));
        }
        let rep = num("repetition_penalty");
        if (rep - 1.0).abs() > 1e-9 {
            m.insert("repetition_penalty".to_string(), val("repetition_penalty"));
            // llama.cpp's own name for it.
            m.insert("repeat_penalty".to_string(), val("repetition_penalty"));
        }
        if num("frequency_penalty") != 0.0 {
            m.insert("frequency_penalty".to_string(), val("frequency_penalty"));
        }
        if num("presence_penalty") != 0.0 {
            m.insert("presence_penalty".to_string(), val("presence_penalty"));
        }
        if num("seed") >= 0.0 {
            m.insert("seed".to_string(), val("seed"));
        }
        if val("ignore_eos").as_bool().unwrap_or(false) {
            m.insert("ignore_eos".to_string(), json!(true));
        }

        let mut stops: Vec<String> = stop.to_vec();
        for s in json_strings(&text_of(&val("stop_strings"))) {
            if !s.is_empty() && !stops.contains(&s) {
                stops.push(s);
            }
        }
        if !stops.is_empty() {
            m.insert("stop".to_string(), json!(stops));
        }

        // Whatever else the person wants sent; it wins over the fields above.
        if let Ok(Value::Object(extra)) =
            serde_json::from_str::<Value>(text_of(&val("extra_body")).trim())
        {
            for (k, v) in extra {
                if k != "prompt" {
                    m.insert(k, v);
                }
            }
        }
        Value::Object(m)
    }

    fn parse_stream_line(&self, line: &str) -> Piece {
        let payload = match line.strip_prefix("data:") {
            Some(p) => p.trim(),
            // Not an SSE data line: only a bare JSON error is of interest.
            None => {
                if line.starts_with('{') {
                    if let Ok(v) = serde_json::from_str::<Value>(line) {
                        if let Some(e) = self.error_in(&v) {
                            return Piece::Error(e);
                        }
                    }
                }
                return Piece::Skip;
            }
        };
        if payload.is_empty() {
            return Piece::Skip;
        }
        if payload == "[DONE]" {
            return Piece::Done;
        }
        let v: Value = match serde_json::from_str(payload) {
            Ok(v) => v,
            Err(_) => return Piece::Skip,
        };
        if let Some(e) = self.error_in(&v) {
            return Piece::Error(e);
        }
        let text = v
            .pointer("/choices/0/text")
            .and_then(|c| c.as_str())
            .unwrap_or("");
        if !text.is_empty() {
            return Piece::Text(text.to_string());
        }
        // The last chunk: no more text, a reason for stopping.
        let finished = v
            .pointer("/choices/0/finish_reason")
            .map(|r| !r.is_null())
            .unwrap_or(false);
        if finished {
            return Piece::Done;
        }
        Piece::Skip
    }

    fn parse_full(&self, body: &Value) -> Option<String> {
        body.pointer("/choices/0/text")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string())
    }

    fn error_in(&self, body: &Value) -> Option<String> {
        // vLLM and others: {"object": "error", "message": "..."}; FastAPI
        // servers: {"detail": "..."}.
        if body.get("object").and_then(|o| o.as_str()) == Some("error") {
            if let Some(m) = body.get("message").and_then(|m| m.as_str()) {
                return Some(m.to_string());
            }
        }
        if let Some(d) = body.get("detail").and_then(|d| d.as_str()) {
            return Some(d.to_string());
        }
        match body.get("error")? {
            Value::Null | Value::Bool(false) => None,
            Value::String(s) => Some(s.clone()),
            e => Some(
                e.get("message")
                    .and_then(|m| m.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| e.to_string()),
            ),
        }
    }

    fn info_paths(&self) -> &'static [&'static str] {
        &["/v1/models", "/health"]
    }

    fn parse_info(&self, body: &Value) -> Info {
        let first = body.pointer("/data/0");
        let model = first
            .and_then(|m| m.get("id"))
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string();
        // vLLM reports the context length it serves.
        let n_ctx = first
            .and_then(|m| m.get("max_model_len"))
            .and_then(|n| n.as_i64());
        Info { model, n_ctx }
    }

    fn tokenize_request(&self, text: &str) -> (&'static str, Value) {
        // llama.cpp reads `content`, vLLM reads `prompt`.
        ("/tokenize", json!({ "content": text, "prompt": text }))
    }

    fn parse_token_count(&self, body: &Value) -> Option<usize> {
        body.get("count")
            .and_then(|n| n.as_u64())
            .map(|n| n as usize)
            .or_else(|| {
                body.get("tokens")
                    .and_then(|t| t.as_array())
                    .map(|a| a.len())
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Map<String, Value> {
        Map::new()
    }

    #[test]
    fn body_sends_the_standard_fields_and_leaves_the_rest_out() {
        let b = OpenAi.build_body(&defaults(), "hello", &["\nYou:".to_string()]);
        assert_eq!(b["prompt"], "hello");
        assert_eq!(b["max_tokens"], 350);
        assert_eq!(b["temperature"], 0.8);
        assert_eq!(b["top_p"], 1.0);
        assert_eq!(b["stream"], true);
        assert_eq!(b["stop"], json!(["\nYou:"]));
        for k in [
            "model", "top_k", "min_p", "typical_p", "repetition_penalty", "repeat_penalty",
            "frequency_penalty", "presence_penalty", "seed", "ignore_eos", "max_context",
            "extra_body", "use_tokenizer",
        ] {
            assert!(b.get(k).is_none(), "{} should not be sent", k);
        }
    }

    #[test]
    fn no_stops_means_no_stop_field() {
        let b = OpenAi.build_body(&defaults(), "x", &[]);
        assert!(b.get("stop").is_none());
    }

    #[test]
    fn switched_on_fields_are_sent() {
        let mut p = Map::new();
        p.insert("model".into(), json!("  my-model "));
        p.insert("top_k".into(), json!(40));
        p.insert("min_p".into(), json!(0.05));
        p.insert("typical_p".into(), json!(0.9));
        p.insert("repetition_penalty".into(), json!(1.1));
        p.insert("frequency_penalty".into(), json!(0.3));
        p.insert("presence_penalty".into(), json!(-0.2));
        p.insert("seed".into(), json!(42));
        p.insert("ignore_eos".into(), json!(true));
        let b = OpenAi.build_body(&p, "x", &[]);
        assert_eq!(b["model"], "my-model");
        assert_eq!(b["top_k"], 40);
        assert_eq!(b["min_p"], 0.05);
        assert_eq!(b["typical_p"], 0.9);
        assert_eq!(b["repetition_penalty"], 1.1);
        assert_eq!(b["repeat_penalty"], 1.1);
        assert_eq!(b["frequency_penalty"], 0.3);
        assert_eq!(b["presence_penalty"], -0.2);
        assert_eq!(b["seed"], 42);
        assert_eq!(b["ignore_eos"], true);
    }

    #[test]
    fn stop_strings_are_added_once() {
        let mut p = Map::new();
        p.insert("stop_strings".into(), json!("[\"OOC:\", \"\\nYou:\"]"));
        let b = OpenAi.build_body(&p, "x", &["\nYou:".to_string()]);
        assert_eq!(b["stop"], json!(["\nYou:", "OOC:"]));
    }

    #[test]
    fn extra_fields_are_merged_and_win_but_not_over_the_prompt() {
        let mut p = Map::new();
        p.insert(
            "extra_body".into(),
            json!("{\"xtc_probability\": 0.5, \"temperature\": 2.0, \"prompt\": \"evil\"}"),
        );
        let b = OpenAi.build_body(&p, "real", &[]);
        assert_eq!(b["xtc_probability"], 0.5);
        assert_eq!(b["temperature"], 2.0);
        assert_eq!(b["prompt"], "real");
        // Not an object: ignored.
        p.insert("extra_body".into(), json!("[1, 2]"));
        let b = OpenAi.build_body(&p, "real", &[]);
        assert!(b.get("xtc_probability").is_none());
    }

    #[test]
    fn text_settings_are_checked() {
        assert!(OpenAi.validate_text("stop_strings", "").is_ok());
        assert!(OpenAi.validate_text("stop_strings", "[\"a\"]").is_ok());
        assert!(OpenAi.validate_text("stop_strings", "a").is_err());
        assert!(OpenAi.validate_text("extra_body", "").is_ok());
        assert!(OpenAi.validate_text("extra_body", "{\"a\": 1}").is_ok());
        assert!(OpenAi.validate_text("extra_body", "[1]").is_err());
        assert!(OpenAi.validate_text("extra_body", "nope").is_err());
        assert!(OpenAi.validate_text("model", "anything").is_ok());
    }

    #[test]
    fn limits_streaming_and_tokenizer_switch() {
        let mut p = Map::new();
        assert_eq!(OpenAi.limits(&p), (8192, 350));
        assert!(OpenAi.streaming(&p));
        assert!(!OpenAi.tokenizes(&p));
        p.insert("max_context".into(), json!(16384));
        p.insert("max_tokens".into(), json!(500));
        p.insert("stream".into(), json!(false));
        p.insert("use_tokenizer".into(), json!(true));
        assert_eq!(OpenAi.limits(&p), (16384, 500));
        assert!(!OpenAi.streaming(&p));
        assert!(OpenAi.tokenizes(&p));
    }

    #[test]
    fn stream_lines() {
        let l = OpenAi;
        assert!(matches!(l.parse_stream_line(""), Piece::Skip));
        assert!(matches!(l.parse_stream_line(": keep-alive"), Piece::Skip));
        assert!(matches!(
            l.parse_stream_line("data: {\"choices\":[{\"text\":\"Hi\",\"finish_reason\":null}]}"),
            Piece::Text(t) if t == "Hi"
        ));
        // text and the reason to stop in the same chunk: the text counts
        assert!(matches!(
            l.parse_stream_line("data: {\"choices\":[{\"text\":\".\",\"finish_reason\":\"stop\"}]}"),
            Piece::Text(t) if t == "."
        ));
        assert!(matches!(
            l.parse_stream_line("data: {\"choices\":[{\"text\":\"\",\"finish_reason\":\"length\"}]}"),
            Piece::Done
        ));
        // a usage-only chunk has no choices
        assert!(matches!(l.parse_stream_line("data: {\"choices\":[],\"usage\":{}}"), Piece::Skip));
        assert!(matches!(
            l.parse_stream_line("data: {\"error\":{\"message\":\"boom\"}}"),
            Piece::Error(e) if e == "boom"
        ));
        assert!(matches!(l.parse_stream_line("data: [DONE]"), Piece::Done));
        assert!(matches!(l.parse_stream_line("event: x"), Piece::Skip));
        assert!(matches!(
            l.parse_stream_line("{\"object\":\"error\",\"message\":\"bad\",\"code\":400}"),
            Piece::Error(e) if e == "bad"
        ));
    }

    #[test]
    fn full_reply_and_errors() {
        let v = json!({"choices": [{"text": "Hello there", "finish_reason": "stop"}]});
        assert_eq!(OpenAi.parse_full(&v).as_deref(), Some("Hello there"));
        assert_eq!(OpenAi.parse_full(&json!({})), None);
        assert_eq!(OpenAi.error_in(&v), None);
        assert_eq!(OpenAi.error_in(&json!({"error": "nope"})).as_deref(), Some("nope"));
        assert_eq!(
            OpenAi.error_in(&json!({"error": {"message": "bad key", "type": "x"}})).as_deref(),
            Some("bad key")
        );
        assert_eq!(OpenAi.error_in(&json!({"detail": "Not Found"})).as_deref(), Some("Not Found"));
        assert_eq!(OpenAi.error_in(&json!({"error": null})), None);
    }

    #[test]
    fn info_from_models() {
        let v = json!({"object": "list", "data": [{"id": "foo-7b", "max_model_len": 4096}]});
        let i = OpenAi.parse_info(&v);
        assert_eq!(i.model, "foo-7b");
        assert_eq!(i.n_ctx, Some(4096));
        let i = OpenAi.parse_info(&json!({"data": [{"id": "bar"}]}));
        assert_eq!(i.model, "bar");
        assert_eq!(i.n_ctx, None);
        // /health answers with nothing useful
        let i = OpenAi.parse_info(&Value::Null);
        assert_eq!(i.model, "");
        assert_eq!(i.n_ctx, None);
    }

    #[test]
    fn token_count_from_either_shape() {
        assert_eq!(OpenAi.parse_token_count(&json!({"count": 12, "tokens": [1, 2]})), Some(12));
        assert_eq!(OpenAi.parse_token_count(&json!({"tokens": [1, 2, 3]})), Some(3));
        assert_eq!(OpenAi.parse_token_count(&json!({"x": 1})), None);
    }

    #[test]
    fn presets_cover_every_sampler_and_are_on_the_grid() {
        let presets = OpenAi.presets();
        let names: Vec<&str> = presets.iter().map(|p| p.name).collect();
        assert_eq!(names, ["Normal", "Creative", "Crazy"]);
        let n = presets[0].values.len();
        assert_eq!(n, 8);
        for p in &presets {
            assert_eq!(p.values.len(), n);
            for (k, v) in &p.values {
                let d = OpenAi.params().into_iter().find(|d| d.key == *k).unwrap();
                if let Some(x) = v.as_f64() {
                    assert_eq!(&d.snap(x), v, "{} {}", p.name, k);
                }
            }
        }
        let get = |p: &Preset, k: &str| p.values.iter().find(|(x, _)| *x == k).unwrap().1.clone();
        assert_eq!(get(&presets[1], "temperature"), json!(1.15));
        assert_eq!(get(&presets[2], "top_k"), json!(0));
        assert_eq!(get(&presets[0], "frequency_penalty"), json!(0.0));
    }

    #[test]
    fn a_summary_gets_its_own_reply_length() {
        let c = super::super::Connection::new("openai", "x").unwrap();
        let s = super::super::for_summary(&c, 777);
        assert_eq!(s.params["max_tokens"], 777);
        assert_eq!(s.params["repetition_penalty"], 1.0);
        let b = OpenAi.build_body(&s.resolved(), "x", &[]);
        assert_eq!(b["max_tokens"], 777);
        assert_eq!(b["temperature"], 0.5);
    }

    #[test]
    fn keys_are_unique() {
        let defs = OpenAi.params();
        for (i, a) in defs.iter().enumerate() {
            for b in &defs[i + 1..] {
                assert_ne!(a.key, b.key);
            }
        }
    }

    #[test]
    fn defaults_are_valid_for_their_controls() {
        for d in OpenAi.params() {
            if let ParamKind::Slider { min, max, default, .. } = d.kind {
                assert!(default >= min && default <= max, "{}", d.key);
            }
        }
    }
}
