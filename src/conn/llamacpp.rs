//! llama.cpp's own server (`llama-server`): `POST /completion`, `/props`,
//! `/health`, `/tokenize`. Every setting below is sent under its llama.cpp
//! name, so a new llama.cpp sampler only needs a new line in `params`.

use super::{
    json_strings, text_of, value_of, Info, ParamDef, ParamKind, Piece, Preset, Provider,
};
use serde_json::{json, Map, Value};

pub struct LlamaCpp;

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
const G_DRY: &str = "DRY";
const G_XTC: &str = "XTC";
const G_MIRO: &str = "Mirostat";
const G_DYNA: &str = "Dynamic temperature";
const G_ADV: &str = "Advanced";

const DRY_BREAKERS: &str = r#"["\n", ":", "\"", "*"]"#;

impl Provider for LlamaCpp {
    fn kind(&self) -> &'static str {
        "llamacpp"
    }

    fn label(&self) -> &'static str {
        "llama.cpp"
    }

    fn default_url(&self) -> &'static str {
        "http://127.0.0.1:8080"
    }

    fn textgen_type(&self) -> &'static str {
        "llamacpp"
    }

    fn params(&self) -> Vec<ParamDef> {
        vec![
            // ---- Generation
            p("n_predict", "Max reply tokens", G_GEN,
              "Longest reply the model may write.",
              slider(1.0, 8192.0, 1.0, 350.0)),
            p("max_context", "Context size", G_GEN,
              "Tokens of chat history sent. Keep at or below the server's -c (press c to copy it).",
              slider(512.0, 262144.0, 256.0, 8192.0)),
            p("stream", "Stream reply", G_GEN,
              "Show the reply as it is written.",
              ParamKind::Toggle { default: true }),
            p("seed", "Seed", G_GEN,
              "-1 = random each time.",
              ParamKind::Number { default: -1 }),
            p("ignore_eos", "Ban EOS token", G_GEN,
              "Never let the model end its reply by itself (it only stops at the limit or a stop string).",
              ParamKind::Toggle { default: false }),
            p("cache_prompt", "Reuse prompt cache", G_GEN,
              "Skip re-reading the part of the prompt the server already has. Leave on.",
              ParamKind::Toggle { default: true }),
            p("stop_strings", "Extra stop strings", G_GEN,
              "JSON list, e.g. [\"\\n\\n\", \"OOC:\"]. Added to the stops of the instruct template.",
              ParamKind::Text { default: "[]" }),
            // ---- Sampling
            p("temperature", "Temperature", G_SAMPLING,
              "Higher = more random, lower = more predictable.",
              slider(0.0, 5.0, 0.01, 0.8)),
            p("top_k", "Top K", G_SAMPLING,
              "Only the K likeliest tokens are considered. 0 = off.",
              slider(0.0, 200.0, 1.0, 40.0)),
            p("top_p", "Top P", G_SAMPLING,
              "Keep the smallest set of tokens whose probabilities add up to P. 1 = off.",
              slider(0.0, 1.0, 0.01, 0.95)),
            p("min_p", "Min P", G_SAMPLING,
              "Drop tokens below P times the likeliest one's probability. 0 = off.",
              slider(0.0, 1.0, 0.01, 0.05)),
            p("typical_p", "Typical P", G_SAMPLING,
              "Locally typical sampling. 1 = off.",
              slider(0.0, 1.0, 0.01, 1.0)),
            p("top_n_sigma", "Top-nσ", G_SAMPLING,
              "Keep tokens within N standard deviations of the top logit. -1 = off.",
              slider(-1.0, 5.0, 0.1, -1.0)),
            p("min_keep", "Min keep", G_SAMPLING,
              "Samplers never leave fewer than this many tokens.",
              slider(0.0, 100.0, 1.0, 0.0)),
            // ---- Repetition
            p("repeat_penalty", "Repetition penalty", G_REPEAT,
              "Above 1 discourages repeating recent tokens. 1 = off.",
              slider(1.0, 2.0, 0.01, 1.0)),
            p("repeat_last_n", "Penalty window", G_REPEAT,
              "How many recent tokens the penalties look at. 0 = off, -1 = whole context.",
              slider(-1.0, 4096.0, 1.0, 64.0)),
            p("frequency_penalty", "Frequency penalty", G_REPEAT,
              "Punishes tokens by how often they were already used.",
              slider(-2.0, 2.0, 0.01, 0.0)),
            p("presence_penalty", "Presence penalty", G_REPEAT,
              "Punishes tokens that were already used at all.",
              slider(-2.0, 2.0, 0.01, 0.0)),
            // ---- DRY
            p("dry_multiplier", "DRY multiplier", G_DRY,
              "Strength of the anti-repetition-of-phrases sampler. 0 = off.",
              slider(0.0, 5.0, 0.01, 0.0)),
            p("dry_base", "DRY base", G_DRY,
              "How fast the penalty grows with the length of the repeat.",
              slider(1.0, 4.0, 0.01, 1.75)),
            p("dry_allowed_length", "DRY allowed length", G_DRY,
              "Repeats up to this long are not penalised.",
              slider(1.0, 20.0, 1.0, 2.0)),
            p("dry_penalty_last_n", "DRY window", G_DRY,
              "Tokens DRY looks back over. -1 = whole context, 0 = off.",
              slider(-1.0, 8192.0, 1.0, -1.0)),
            p("dry_sequence_breakers", "DRY breakers", G_DRY,
              "JSON list of strings that end a repeated sequence.",
              ParamKind::Text { default: DRY_BREAKERS }),
            // ---- XTC
            p("xtc_probability", "XTC probability", G_XTC,
              "Chance per token that the top choices are cut (more variety). 0 = off.",
              slider(0.0, 1.0, 0.01, 0.0)),
            p("xtc_threshold", "XTC threshold", G_XTC,
              "Tokens above this probability can be cut by XTC.",
              slider(0.0, 1.0, 0.01, 0.1)),
            // ---- Mirostat
            p("mirostat", "Mirostat mode", G_MIRO,
              "0 = off, 1 = Mirostat, 2 = Mirostat 2.0. Replaces top-k / top-p / min-p while on.",
              slider(0.0, 2.0, 1.0, 0.0)),
            p("mirostat_tau", "Mirostat tau", G_MIRO,
              "Target surprise: lower = more focused.",
              slider(0.0, 10.0, 0.1, 5.0)),
            p("mirostat_eta", "Mirostat eta", G_MIRO,
              "How fast Mirostat adapts.",
              slider(0.0, 1.0, 0.01, 0.1)),
            // ---- Dynamic temperature
            p("dynatemp_range", "Range", G_DYNA,
              "Temperature moves within +/- this around the Temperature value. 0 = off.",
              slider(0.0, 5.0, 0.01, 0.0)),
            p("dynatemp_exponent", "Exponent", G_DYNA,
              "Shape of the temperature curve.",
              slider(0.0, 5.0, 0.01, 1.0)),
            // ---- Advanced
            p("samplers", "Sampler order", G_ADV,
              "Names separated by ; in the order to run, e.g. penalties;dry;top_k;typ_p;top_p;min_p;xtc;temperature. Empty = the server's default.",
              ParamKind::Text { default: "" }),
            p("grammar", "Grammar (GBNF)", G_ADV,
              "Force the output to follow a GBNF grammar. Empty = none.",
              ParamKind::Text { default: "" }),
        ]
    }

    fn presets(&self) -> Vec<Preset> {
        // A preset sets every sampler setting (the groups Sampling to Dynamic
        // temperature): what it does not name goes back to its default, so
        // switching between presets never leaves anything behind. Generation
        // and Advanced settings (reply length, context, stops) are not touched.
        let sampler_groups = [G_SAMPLING, G_REPEAT, G_DRY, G_XTC, G_MIRO, G_DYNA];
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
                    ("top_k", 40.0),
                    ("top_p", 0.95),
                    ("min_p", 0.05),
                    ("repeat_penalty", 1.05),
                ],
            ),
            make(
                "Creative",
                "more varied wording, DRY against repeated phrases",
                &[
                    ("temperature", 1.15),
                    ("top_k", 80.0),
                    ("top_p", 0.98),
                    ("min_p", 0.04),
                    ("repeat_penalty", 1.05),
                    ("dry_multiplier", 0.8),
                ],
            ),
            make(
                "Crazy",
                "wild and unpredictable (high temperature, XTC, DRY)",
                &[
                    ("temperature", 1.8),
                    ("top_k", 0.0),
                    ("top_p", 1.0),
                    ("min_p", 0.02),
                    ("repeat_penalty", 1.1),
                    ("dry_multiplier", 1.0),
                    ("xtc_probability", 0.5),
                    ("xtc_threshold", 0.1),
                    ("dynatemp_range", 0.5),
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
        (get("max_context", 8192), get("n_predict", 350))
    }

    fn streaming(&self, params: &Map<String, Value>) -> bool {
        self.params()
            .iter()
            .find(|d| d.key == "stream")
            .map(|d| value_of(params, d).as_bool().unwrap_or(true))
            .unwrap_or(true)
    }

    fn validate_text(&self, key: &str, text: &str) -> Result<(), String> {
        match key {
            "stop_strings" | "dry_sequence_breakers" => {
                let t = text.trim();
                if t.is_empty() {
                    return Ok(());
                }
                serde_json::from_str::<Vec<String>>(t)
                    .map(|_| ())
                    .map_err(|_| "must be a JSON list of strings, e.g. [\"a\", \"b\"]".to_string())
            }
            _ => Ok(()),
        }
    }

    fn generate_path(&self) -> &'static str {
        "/completion"
    }

    fn build_body(&self, params: &Map<String, Value>, prompt: &str, stop: &[String]) -> Value {
        let mut m: Map<String, Value> = Map::new();
        m.insert("prompt".to_string(), json!(prompt));
        let mut stops: Vec<String> = stop.to_vec();
        for d in self.params() {
            let v = value_of(params, &d);
            match d.key {
                // Used by st-cli itself, not a request field.
                "max_context" => {}
                "stop_strings" => {
                    for s in json_strings(&text_of(&v)) {
                        if !s.is_empty() && !stops.contains(&s) {
                            stops.push(s);
                        }
                    }
                }
                "dry_sequence_breakers" => {
                    let list = json_strings(&text_of(&v));
                    if !list.is_empty() {
                        m.insert(d.key.to_string(), json!(list));
                    }
                }
                // Newer llama.cpp builds refuse -1 here ("whole context"), so
                // send the context size instead. While DRY is off the field
                // is left out altogether.
                "dry_penalty_last_n" => {
                    let dry_on = self
                        .params()
                        .iter()
                        .find(|x| x.key == "dry_multiplier")
                        .and_then(|x| value_of(params, x).as_f64())
                        .map(|x| x > 0.0)
                        .unwrap_or(false);
                    if dry_on {
                        let n = v.as_f64().unwrap_or(-1.0).round() as i64;
                        let n = if n < 0 { self.limits(params).0.max(1) } else { n };
                        m.insert(d.key.to_string(), json!(n));
                    }
                }
                "samplers" => {
                    let list: Vec<String> = text_of(&v)
                        .split(|c| c == ';' || c == ',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !list.is_empty() {
                        m.insert(d.key.to_string(), json!(list));
                    }
                }
                "grammar" => {
                    let g = text_of(&v);
                    if !g.trim().is_empty() {
                        m.insert(d.key.to_string(), json!(g));
                    }
                }
                _ => {
                    m.insert(d.key.to_string(), v);
                }
            }
        }
        m.insert("stop".to_string(), json!(stops));
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
            .get("content")
            .and_then(|c| c.as_str())
            .or_else(|| v.pointer("/choices/0/text").and_then(|c| c.as_str()))
            .unwrap_or("");
        if !text.is_empty() {
            return Piece::Text(text.to_string());
        }
        if v.get("stop").and_then(|s| s.as_bool()).unwrap_or(false) {
            return Piece::Done;
        }
        Piece::Skip
    }

    fn parse_full(&self, body: &Value) -> Option<String> {
        body.get("content")
            .and_then(|c| c.as_str())
            .or_else(|| body.pointer("/choices/0/text").and_then(|c| c.as_str()))
            .map(|s| s.to_string())
    }

    fn error_in(&self, body: &Value) -> Option<String> {
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
        &["/props", "/health"]
    }

    fn parse_info(&self, body: &Value) -> Info {
        let model = body
            .get("model_alias")
            .and_then(|m| m.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .or_else(|| {
                body.get("model_path")
                    .and_then(|m| m.as_str())
                    .map(|path| path.rsplit(['/', '\\']).next().unwrap_or(path).to_string())
            })
            .unwrap_or_default();
        let n_ctx = body
            .pointer("/default_generation_settings/n_ctx")
            .and_then(|n| n.as_i64())
            .or_else(|| body.pointer("/default_generation_settings/params/n_ctx").and_then(|n| n.as_i64()));
        Info { model, n_ctx }
    }

    fn tokenize_request(&self, text: &str) -> (&'static str, Value) {
        ("/tokenize", json!({ "content": text }))
    }

    fn parse_token_count(&self, body: &Value) -> Option<usize> {
        body.get("tokens")
            .and_then(|t| t.as_array())
            .map(|a| a.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Map<String, Value> {
        Map::new()
    }

    #[test]
    fn body_has_defaults_and_prompt() {
        let b = LlamaCpp.build_body(&defaults(), "hello", &["\nYou:".to_string()]);
        assert_eq!(b["prompt"], "hello");
        assert_eq!(b["n_predict"], 350);
        assert_eq!(b["top_k"], 40);
        assert_eq!(b["temperature"], 0.8);
        assert_eq!(b["stream"], true);
        assert_eq!(b["stop"], json!(["\nYou:"]));
        // client-side only, empty text settings are left out
        assert!(b.get("max_context").is_none());
        assert!(b.get("grammar").is_none());
        assert!(b.get("samplers").is_none());
        assert_eq!(b["dry_sequence_breakers"], json!(["\n", ":", "\"", "*"]));
        // DRY is off by default: no window is sent (-1 is refused by new servers)
        assert!(b.get("dry_penalty_last_n").is_none());
    }

    #[test]
    fn presets_cover_every_sampler_and_are_on_the_grid() {
        let presets = LlamaCpp.presets();
        let names: Vec<&str> = presets.iter().map(|p| p.name).collect();
        assert_eq!(names, ["Normal", "Creative", "Crazy"]);
        let n = presets[0].values.len();
        assert!(n > 20);
        for p in &presets {
            assert_eq!(p.values.len(), n);
            for (k, v) in &p.values {
                let d = LlamaCpp.params().into_iter().find(|d| d.key == *k).unwrap();
                if let Some(x) = v.as_f64() {
                    assert_eq!(&d.snap(x), v, "{} {}", p.name, k);
                }
            }
        }
        let get = |p: &Preset, k: &str| p.values.iter().find(|(x, _)| *x == k).unwrap().1.clone();
        assert_eq!(get(&presets[1], "temperature"), json!(1.15));
        assert_eq!(get(&presets[2], "top_k"), json!(0));
        assert_eq!(get(&presets[0], "dry_multiplier"), json!(0.0));
    }

    #[test]
    fn dry_window_minus_one_becomes_context_size() {
        let mut p = Map::new();
        p.insert("dry_multiplier".into(), json!(0.8));
        p.insert("max_context".into(), json!(16384));
        let b = LlamaCpp.build_body(&p, "x", &[]);
        assert_eq!(b["dry_penalty_last_n"], 16384);
        p.insert("dry_penalty_last_n".into(), json!(2048));
        let b = LlamaCpp.build_body(&p, "x", &[]);
        assert_eq!(b["dry_penalty_last_n"], 2048);
    }

    #[test]
    fn stored_values_win_and_bad_types_fall_back() {
        let mut p = Map::new();
        p.insert("temperature".into(), json!(1.25));
        p.insert("top_k".into(), json!("lots"));
        p.insert("stop_strings".into(), json!("[\"OOC:\", \"\\nYou:\"]"));
        p.insert("samplers".into(), json!("top_k; min_p;temperature"));
        let b = LlamaCpp.build_body(&p, "x", &["\nYou:".to_string()]);
        assert_eq!(b["temperature"], 1.25);
        assert_eq!(b["top_k"], 40);
        assert_eq!(b["stop"], json!(["\nYou:", "OOC:"]));
        assert_eq!(b["samplers"], json!(["top_k", "min_p", "temperature"]));
    }

    #[test]
    fn limits_and_streaming() {
        let mut p = Map::new();
        assert_eq!(LlamaCpp.limits(&p), (8192, 350));
        p.insert("max_context".into(), json!(16384));
        p.insert("n_predict".into(), json!(500));
        p.insert("stream".into(), json!(false));
        assert_eq!(LlamaCpp.limits(&p), (16384, 500));
        assert!(!LlamaCpp.streaming(&p));
    }

    #[test]
    fn stream_lines() {
        let l = LlamaCpp;
        assert!(matches!(l.parse_stream_line(""), Piece::Skip));
        assert!(matches!(l.parse_stream_line("data: {\"content\":\"Hi\",\"stop\":false}"),
            Piece::Text(t) if t == "Hi"));
        assert!(matches!(l.parse_stream_line("data: {\"content\":\"\",\"stop\":true}"), Piece::Done));
        assert!(matches!(l.parse_stream_line("data: {\"error\":{\"message\":\"boom\"}}"),
            Piece::Error(e) if e == "boom"));
        assert!(matches!(l.parse_stream_line("data: [DONE]"), Piece::Done));
        assert!(matches!(l.parse_stream_line("event: x"), Piece::Skip));
    }

    #[test]
    fn info_from_props() {
        let v = json!({"model_path": "/models/Foo-7B.Q4.gguf",
                       "default_generation_settings": {"n_ctx": 4096}});
        let i = LlamaCpp.parse_info(&v);
        assert_eq!(i.model, "Foo-7B.Q4.gguf");
        assert_eq!(i.n_ctx, Some(4096));
        assert_eq!(LlamaCpp.parse_info(&json!({"status": "ok"})).model, "");
    }

    #[test]
    fn slider_snapping() {
        let defs = LlamaCpp.params();
        let t = defs.iter().find(|d| d.key == "temperature").unwrap();
        assert_eq!(t.snap(0.8049), json!(0.8));
        assert_eq!(t.snap(9.0), json!(5.0));
        assert_eq!(t.format(&json!(0.8)), "0.80");
        let k = defs.iter().find(|d| d.key == "top_k").unwrap();
        assert_eq!(k.snap(41.4), json!(41));
        assert!(k.is_int());
        let c = defs.iter().find(|d| d.key == "max_context").unwrap();
        assert_eq!(c.snap(1000.0), json!(1024));
    }

    #[test]
    fn keys_are_unique() {
        let defs = LlamaCpp.params();
        for (i, a) in defs.iter().enumerate() {
            for b in &defs[i + 1..] {
                assert_ne!(a.key, b.key);
            }
        }
    }
}
