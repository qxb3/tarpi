//! Sends a chat-completion request through SillyTavern's backend proxy
//! (`/api/backends/chat-completions/generate`) and streams the reply back.
//! SillyTavern holds the API keys and talks to the actual provider.

use crate::api::{short, Api};
use crate::jv::*;
use crate::prompt::{subst_names, ChatMsg};
use anyhow::{bail, Context, Result};
use reqwest::StatusCode;
use serde_json::{json, Map, Value};
use tokio::sync::mpsc::UnboundedSender;

pub enum GenEvent {
    Chunk(String),
    Done,
    Error(String),
}

/// `(generation id, event)` so stale events from an aborted request can be ignored.
pub type GenMsg = (u64, GenEvent);

pub struct Request {
    pub body: Value,
    pub source: String,
    pub stream: bool,
    pub model: String,
    /// Server path the request is POSTed to.
    pub endpoint: &'static str,
}

/// Which settings key holds the model name for each chat-completion source.
pub fn model_for(o: &Value, source: &str) -> String {
    let key = match source {
        "openai" => "openai_model",
        "claude" => "claude_model",
        "openrouter" => "openrouter_model",
        "ai21" => "ai21_model",
        "makersuite" => "google_model",
        "vertexai" => "vertexai_model",
        "mistralai" => "mistralai_model",
        "custom" => "custom_model",
        "cohere" => "cohere_model",
        "perplexity" => "perplexity_model",
        "groq" => "groq_model",
        "electronhub" => "electronhub_model",
        "chutes" => "chutes_model",
        "nanogpt" => "nanogpt_model",
        "deepseek" => "deepseek_model",
        "aimlapi" => "aimlapi_model",
        "xai" => "xai_model",
        "pollinations" => "pollinations_model",
        "moonshot" => "moonshot_model",
        "fireworks" => "fireworks_model",
        "cometapi" => "cometapi_model",
        "azure_openai" => "azure_openai_model",
        "zai" => "zai_model",
        "siliconflow" => "siliconflow_model",
        "workers_ai" => "workers_ai_model",
        "minimax" => "minimax_model",
        _ => "",
    };
    if key.is_empty() {
        String::new()
    } else {
        jstr(o, key)
    }
}

fn copy(m: &mut Map<String, Value>, o: &Value, from: &str, to: &str) {
    if let Some(v) = o.get(from) {
        m.insert(to.to_string(), v.clone());
    }
}

/// Build the request body the same way the web UI does (`createGenerationParameters`).
pub fn build_request(o: &Value, msgs: &[ChatMsg], user: &str, char_name: &str) -> Result<Request> {
    let source = {
        let s = jstr(o, "chat_completion_source");
        if s.is_empty() {
            "openai".to_string()
        } else {
            s
        }
    };
    let model = model_for(o, &source);
    let stream = jbool(o, "stream_openai", true);

    let mut body = json!({
        "type": "normal",
        "messages": msgs,
        "model": model,
        "temperature": jf64(o, "temp_openai", 1.0),
        "frequency_penalty": jf64(o, "freq_pen_openai", 0.0),
        "presence_penalty": jf64(o, "pres_pen_openai", 0.0),
        "top_p": jf64(o, "top_p_openai", 1.0),
        "max_tokens": ji64(o, "openai_max_tokens", 300),
        "stream": stream,
        "chat_completion_source": source,
        "user_name": user,
        "char_name": char_name,
        "group_names": [],
        "include_reasoning": false,
        "enable_web_search": jbool(o, "enable_web_search", false),
        "request_images": false,
        "custom_prompt_post_processing": jstr(o, "custom_prompt_post_processing"),
    });

    {
        let m = body.as_object_mut().expect("body is an object");

        for key in ["reasoning_effort", "verbosity"] {
            let v = jstr(o, key);
            if !v.is_empty() && v != "auto" {
                m.insert(key.to_string(), json!(v));
            }
        }

        match source.as_str() {
            "claude" => {
                m.insert("top_k".into(), json!(jf64(o, "top_k_openai", 0.0)));
                m.insert(
                    "use_sysprompt".into(),
                    json!(jbool(o, "use_sysprompt", false)),
                );
                let prefill = subst_names(&jstr(o, "assistant_prefill"), user, char_name);
                m.insert("assistant_prefill".into(), json!(prefill));
            }
            "openrouter" => {
                m.insert("top_k".into(), json!(jf64(o, "top_k_openai", 0.0)));
                m.insert("min_p".into(), json!(jf64(o, "min_p_openai", 0.0)));
                m.insert(
                    "repetition_penalty".into(),
                    json!(jf64(o, "repetition_penalty_openai", 1.0)),
                );
                m.insert("top_a".into(), json!(jf64(o, "top_a_openai", 0.0)));
                copy(m, o, "openrouter_use_fallback", "use_fallback");
                copy(m, o, "openrouter_providers", "provider");
                copy(m, o, "openrouter_quantizations", "quantizations");
                copy(m, o, "openrouter_allow_fallbacks", "allow_fallbacks");
                copy(m, o, "openrouter_middleout", "middleout");
            }
            "makersuite" | "vertexai" => {
                m.insert("top_k".into(), json!(jf64(o, "top_k_openai", 0.0)));
                m.insert(
                    "use_sysprompt".into(),
                    json!(jbool(o, "use_sysprompt", false)),
                );
                if source == "vertexai" {
                    copy(m, o, "vertexai_auth_mode", "vertexai_auth_mode");
                    copy(m, o, "vertexai_region", "vertexai_region");
                    copy(
                        m,
                        o,
                        "vertexai_express_project_id",
                        "vertexai_express_project_id",
                    );
                }
            }
            "custom" => {
                for key in [
                    "custom_url",
                    "custom_include_body",
                    "custom_exclude_body",
                    "custom_include_headers",
                ] {
                    let v = subst_names(&jstr(o, key), user, char_name);
                    m.insert(key.to_string(), json!(v));
                }
            }
            "azure_openai" => {
                copy(m, o, "azure_base_url", "azure_base_url");
                copy(m, o, "azure_deployment_name", "azure_deployment_name");
                copy(m, o, "azure_api_version", "azure_api_version");
            }
            "cohere" | "electronhub" | "perplexity" | "chutes" => {
                m.insert("top_k".into(), json!(jf64(o, "top_k_openai", 0.0)));
            }
            _ => {}
        }

        let proxy_sources = [
            "claude",
            "openai",
            "mistralai",
            "makersuite",
            "vertexai",
            "deepseek",
            "xai",
            "zai",
            "moonshot",
        ];
        let proxy = jstr(o, "reverse_proxy");
        if !proxy.trim().is_empty() && proxy_sources.contains(&source.as_str()) {
            m.insert("reverse_proxy".into(), json!(proxy));
            m.insert("proxy_password".into(), json!(jstr(o, "proxy_password")));
        }
    }

    if model.is_empty() && source != "openrouter" {
        bail!(
            "no model is selected for the '{}' chat completion source — pick one in the SillyTavern web UI",
            source
        );
    }

    Ok(Request {
        body,
        source,
        stream,
        model,
        endpoint: "/api/backends/chat-completions/generate",
    })
}

// ------------------------------------------------------------------ running

pub async fn run(api: Api, req: Request, id: u64, tx: UnboundedSender<GenMsg>) {
    let res = run_inner(&api, &req, id, &tx).await;
    let ev = match res {
        Ok(()) => GenEvent::Done,
        Err(e) => GenEvent::Error(format!("{:#}", e)),
    };
    let _ = tx.send((id, ev));
}

async fn run_inner(api: &Api, req: &Request, id: u64, tx: &UnboundedSender<GenMsg>) -> Result<()> {
    let mut resp = api
        .post(req.endpoint)
        .json(&req.body)
        .send()
        .await
        .context("sending generation request to SillyTavern")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        bail!("{}", describe_error(status, &text));
    }

    if !req.stream {
        let text = resp.text().await.context("reading response")?;
        let v: Value = serde_json::from_str(&text)
            .with_context(|| format!("unexpected response: {}", short(&text, 200)))?;
        if let Some(err) = error_message(&v) {
            bail!("{}", err);
        }
        let out = extract_full(&v);
        if out.is_empty() {
            bail!("the model returned an empty response");
        }
        let _ = tx.send((id, GenEvent::Chunk(out)));
        return Ok(());
    }

    let mut buf: Vec<u8> = Vec::new();
    let mut done = false;
    while !done {
        let chunk = resp.chunk().await.context("stream interrupted")?;
        let Some(chunk) = chunk else { break };
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line_bytes: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line_bytes).trim().to_string();
            match handle_line(&req.source, &line)? {
                LineResult::Text(t) => {
                    let _ = tx.send((id, GenEvent::Chunk(t)));
                }
                LineResult::Done => {
                    done = true;
                    break;
                }
                LineResult::Skip => {}
            }
        }
    }
    if !done && !buf.is_empty() {
        let line = String::from_utf8_lossy(&buf).trim().to_string();
        if let LineResult::Text(t) = handle_line(&req.source, &line)? {
            let _ = tx.send((id, GenEvent::Chunk(t)));
        }
    }
    Ok(())
}

enum LineResult {
    Text(String),
    Done,
    Skip,
}

fn handle_line(source: &str, line: &str) -> Result<LineResult> {
    let Some(payload) = line.strip_prefix("data:") else {
        // Not an SSE data line. Surface raw JSON errors, ignore the rest
        // (`event:` lines, keep-alive comments).
        if line.starts_with('{') {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                if let Some(e) = error_message(&v) {
                    bail!("{}", e);
                }
            }
        }
        return Ok(LineResult::Skip);
    };
    let payload = payload.trim();
    if payload.is_empty() {
        return Ok(LineResult::Skip);
    }
    if payload == "[DONE]" {
        return Ok(LineResult::Done);
    }
    let v: Value = match serde_json::from_str(payload) {
        Ok(v) => v,
        Err(_) => return Ok(LineResult::Skip),
    };
    if let Some(e) = error_message(&v) {
        bail!("{}", e);
    }
    let t = extract_delta(source, &v);
    if t.is_empty() {
        Ok(LineResult::Skip)
    } else {
        Ok(LineResult::Text(t))
    }
}

fn str_at(v: &Value, pointer: &str) -> Option<String> {
    v.pointer(pointer)
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
}

fn google_text(v: &Value) -> String {
    v.pointer("/candidates/0/content/parts")
        .and_then(|p| p.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter(|p| !jbool(p, "thought", false))
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<&str>>()
                .join("")
        })
        .unwrap_or_default()
}

fn extract_delta(source: &str, v: &Value) -> String {
    match source {
        "claude" => str_at(v, "/delta/text").unwrap_or_default(),
        "makersuite" | "vertexai" => google_text(v),
        "cohere" => str_at(v, "/delta/message/content/text").unwrap_or_default(),
        "textgen" => {
            // Extra choices / llama.cpp slots (swipes) are ignored.
            let idx = v
                .pointer("/choices/0/index")
                .and_then(|x| x.as_i64())
                .or_else(|| v.get("index").and_then(|x| x.as_i64()))
                .unwrap_or(0);
            if idx > 0 {
                return String::new();
            }
            str_at(v, "/choices/0/text")
                .or_else(|| str_at(v, "/content"))
                .unwrap_or_default()
        }
        _ => str_at(v, "/choices/0/delta/content")
            .or_else(|| str_at(v, "/choices/0/text"))
            .unwrap_or_default(),
    }
}

fn join_text_parts(arr: &[Value]) -> String {
    arr.iter()
        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
        .collect::<Vec<&str>>()
        .join("")
}

fn extract_full(v: &Value) -> String {
    if let Some(s) = str_at(v, "/choices/0/message/content") {
        return s;
    }
    if let Some(arr) = v
        .pointer("/choices/0/message/content")
        .and_then(|x| x.as_array())
    {
        return join_text_parts(arr);
    }
    if let Some(s) = str_at(v, "/choices/0/text") {
        return s;
    }
    if let Some(arr) = v.get("content").and_then(|c| c.as_array()) {
        return join_text_parts(arr);
    }
    // llama.cpp / KoboldCpp style: { "content": "..." }
    if let Some(s) = v.get("content").and_then(|c| c.as_str()) {
        return s.to_string();
    }
    let g = google_text(v);
    if !g.is_empty() {
        return g;
    }
    if let Some(arr) = v.pointer("/message/content").and_then(|c| c.as_array()) {
        return join_text_parts(arr);
    }
    String::new()
}

/// Pull a human-readable message out of the various error shapes.
fn error_message(v: &Value) -> Option<String> {
    let err = v.get("error")?;
    match err {
        Value::Null => None,
        Value::String(s) => Some(s.clone()),
        Value::Bool(false) => None,
        Value::Bool(true) => {
            // Text Completion proxy shape: { error: true, status, response }
            let status = v.get("status").map(|s| match s {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            });
            let resp = v
                .get("response")
                .map(|r| match r {
                    Value::String(s) => short(s, 300),
                    other => short(&other.to_string(), 300),
                })
                .filter(|r| !r.is_empty());
            let msg = v.get("message").and_then(|m| m.as_str());
            Some(match (resp, status, msg) {
                (Some(r), Some(st), _) => {
                    format!("the backend returned an error ({}): {}", st, r)
                }
                (Some(r), None, _) => r,
                (None, _, Some(m)) => m.to_string(),
                _ => "SillyTavern reported an error (check its console)".to_string(),
            })
        }
        Value::Object(_) => Some(
            err.get("message")
                .and_then(|m| m.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| short(&err.to_string(), 300)),
        ),
        other => Some(short(&other.to_string(), 300)),
    }
}

fn describe_error(status: StatusCode, text: &str) -> String {
    let detail = match serde_json::from_str::<Value>(text) {
        Ok(v) => error_message(&v).unwrap_or_else(|| short(text, 300)),
        Err(_) => short(text, 300),
    };
    let detail = if detail.is_empty() {
        "(no details — check the SillyTavern console)".to_string()
    } else {
        detail
    };
    match status.as_u16() {
        401 | 403 => format!(
            "{} — check the API key / login in SillyTavern ({})",
            detail, status
        ),
        _ => format!("{} ({})", detail, status),
    }
}
