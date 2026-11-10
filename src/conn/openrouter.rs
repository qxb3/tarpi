//! OpenRouter (<https://openrouter.ai>): one API key for hundreds of hosted
//! models. It answers the OpenAI *text completions* API, so this kind is the
//! OpenAI-compatible one (`openai.rs`) with the address filled in and a few
//! differences:
//!
//! - *Model name* is picked from OpenRouter's own list (`GET /v1/models`,
//!   public) in a window you can filter by typing; see `Provider::model_source`.
//! - `/v1/key` is used to test the connection, so a wrong API key shows up.
//! - There is no `/tokenize` route and no EOS ban, so those settings are gone.
//!
//! Only models that still offer plain text completion work: a model that is
//! chat-only on OpenRouter answers with an error that tarpi shows as it is.

use super::openai::OpenAi;
use super::{Info, ModelEntry, ModelSource, ParamDef, ParamKind, Piece, Preset, Provider};
use serde_json::{json, Map, Value};

pub struct OpenRouter;

impl Provider for OpenRouter {
    fn kind(&self) -> &'static str {
        "openrouter"
    }

    fn label(&self) -> &'static str {
        "OpenRouter"
    }

    fn default_url(&self) -> &'static str {
        "https://openrouter.ai/api"
    }

    fn textgen_type(&self) -> &'static str {
        "openrouter"
    }

    fn params(&self) -> Vec<ParamDef> {
        OpenAi
            .params()
            .into_iter()
            .filter(|d| d.key != "use_tokenizer" && d.key != "ignore_eos")
            .map(|d| {
                if d.key == "model" {
                    ParamDef {
                        label: "Model",
                        help: "Press Enter to pick from OpenRouter's list (type to filter), e.g. meta-llama/llama-3.3-70b-instruct.",
                        kind: ParamKind::Text { default: "" },
                        ..d
                    }
                } else {
                    d
                }
            })
            .collect()
    }

    fn presets(&self) -> Vec<Preset> {
        OpenAi.presets()
    }

    fn limits(&self, params: &Map<String, Value>) -> (i64, i64) {
        OpenAi.limits(params)
    }

    fn streaming(&self, params: &Map<String, Value>) -> bool {
        OpenAi.streaming(params)
    }

    fn tokenizes(&self, _params: &Map<String, Value>) -> bool {
        false
    }

    fn validate_text(&self, key: &str, text: &str) -> Result<(), String> {
        OpenAi.validate_text(key, text)
    }

    fn generate_path(&self) -> &'static str {
        OpenAi.generate_path()
    }

    fn build_body(&self, params: &Map<String, Value>, prompt: &str, stop: &[String]) -> Value {
        let mut body = OpenAi.build_body(params, prompt, stop);
        if let Some(m) = body.as_object_mut() {
            // llama.cpp's spelling of the repetition penalty: not an OpenRouter field.
            m.remove("repeat_penalty");
        }
        body
    }

    fn parse_stream_line(&self, line: &str) -> Piece {
        // OpenRouter also sends `: OPENROUTER PROCESSING` keep-alive comments;
        // the OpenAI reader skips every line that is not `data:`.
        OpenAi.parse_stream_line(line)
    }

    fn parse_full(&self, body: &Value) -> Option<String> {
        OpenAi.parse_full(body)
    }

    fn error_in(&self, body: &Value) -> Option<String> {
        OpenAi.error_in(body)
    }

    fn info_paths(&self) -> &'static [&'static str] {
        // Answers 401 for a missing or wrong key, unlike the public model list.
        &["/v1/key"]
    }

    fn parse_info(&self, _body: &Value) -> Info {
        Info::default()
    }

    fn tokenize_request(&self, text: &str) -> (&'static str, Value) {
        ("/tokenize", json!({ "prompt": text }))
    }

    fn parse_token_count(&self, _body: &Value) -> Option<usize> {
        None
    }

    fn model_source(&self) -> Option<ModelSource> {
        Some(ModelSource {
            param: "model",
            path: "/v1/models",
            needs_key: false,
        })
    }

    fn parse_models(&self, body: &Value) -> Vec<ModelEntry> {
        let Some(list) = body.get("data").and_then(|d| d.as_array()) else {
            return Vec::new();
        };
        list.iter()
            .filter_map(|m| {
                let id = m.get("id")?.as_str()?.trim();
                if id.is_empty() {
                    return None;
                }
                // Image-only generators and the like cannot complete text.
                if let Some(outs) = m
                    .pointer("/architecture/output_modalities")
                    .and_then(|o| o.as_array())
                {
                    if !outs.iter().any(|x| x.as_str() == Some("text")) {
                        return None;
                    }
                }
                let context = m
                    .get("context_length")
                    .and_then(|n| n.as_f64())
                    .or_else(|| m.pointer("/top_provider/context_length").and_then(|n| n.as_f64()))
                    .map(|n| n as i64)
                    .filter(|n| *n > 0);
                Some(ModelEntry {
                    id: id.to_string(),
                    name: m
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("")
                        .trim()
                        .to_string(),
                    context,
                    price: price_of(m.get("pricing")),
                })
            })
            .collect()
    }
}

/// OpenRouter prices are dollars per token, as strings (`"0.00000015"`).
fn per_million(p: Option<&Value>, key: &str) -> Option<f64> {
    let v = p?.get(key)?;
    let x = match v {
        Value::String(s) => s.trim().parse::<f64>().ok()?,
        Value::Number(n) => n.as_f64()?,
        _ => return None,
    };
    if x < 0.0 {
        return None; // the `auto` router uses -1
    }
    Some(x * 1_000_000.0)
}

fn money(x: f64) -> String {
    let s = if x >= 1.0 {
        format!("{:.2}", x)
    } else {
        format!("{:.3}", x)
    };
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() {
        "0".to_string()
    } else {
        s.to_string()
    }
}

fn price_of(pricing: Option<&Value>) -> String {
    let (Some(i), Some(o)) = (per_million(pricing, "prompt"), per_million(pricing, "completion"))
    else {
        return String::new();
    };
    if i == 0.0 && o == 0.0 {
        "free".to_string()
    } else {
        format!("${}/${} per M", money(i), money(o))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_model_list() {
        let body = json!({ "data": [
            { "id": "a/b", "name": "A: B", "context_length": 131072,
              "pricing": { "prompt": "0.00000015", "completion": "0.0000006" },
              "architecture": { "output_modalities": ["text"] } },
            { "id": "img/only", "name": "Painter",
              "architecture": { "output_modalities": ["image"] } },
            { "id": "free/m:free", "pricing": { "prompt": "0", "completion": "0" },
              "top_provider": { "context_length": 8192 } },
            { "id": "openrouter/auto", "pricing": { "prompt": "-1", "completion": "-1" } },
            { "name": "no id" }
        ]});
        let l = OpenRouter.parse_models(&body);
        let ids: Vec<&str> = l.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, ["a/b", "free/m:free", "openrouter/auto"]);
        assert_eq!(l[0].name, "A: B");
        assert_eq!(l[0].context, Some(131072));
        assert_eq!(l[0].price, "$0.15/$0.6 per M");
        assert_eq!(l[1].price, "free");
        assert_eq!(l[1].context, Some(8192));
        assert_eq!(l[2].price, "");
    }

    #[test]
    fn prices_are_short() {
        assert_eq!(money(15.0), "15");
        assert_eq!(money(0.075), "0.075");
        assert_eq!(money(3.0), "3");
    }

    #[test]
    fn body_has_no_llamacpp_spelling() {
        let mut params = Map::new();
        params.insert("repetition_penalty".to_string(), json!(1.1));
        params.insert("model".to_string(), json!("a/b"));
        let b = OpenRouter.build_body(&params, "hi", &[]);
        assert!(b.get("repeat_penalty").is_none());
        assert_eq!(b["repetition_penalty"], json!(1.1));
        assert_eq!(b["model"], json!("a/b"));
    }
}
