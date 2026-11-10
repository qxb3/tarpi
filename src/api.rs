use anyhow::{anyhow, bail, Context, Result};
use reqwest::{Client, RequestBuilder};
use serde_json::{json, Value};
use std::time::Duration;

/// Thin client for the SillyTavern HTTP API.
///
/// SillyTavern protects its API with a session cookie plus a CSRF token
/// (`GET /csrf-token`, sent back as the `x-csrf-token` header).
#[derive(Clone)]
pub struct Api {
    http: Client,
    base: String,
    csrf: String,
}

impl Api {
    pub async fn connect(base: &str) -> Result<Api> {
        let http = Client::builder()
            .cookie_store(true)
            .connect_timeout(Duration::from_secs(10))
            .build()
            .context("building HTTP client")?;
        let mut api = Api {
            http,
            base: base.trim_end_matches('/').to_string(),
            csrf: String::new(),
        };
        api.refresh_csrf().await?;
        Ok(api)
    }

    async fn refresh_csrf(&mut self) -> Result<()> {
        let resp = self
            .http
            .get(format!("{}/csrf-token", self.base))
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .with_context(|| format!("could not reach SillyTavern at {}", self.base))?;
        let status = resp.status();
        if !status.is_success() {
            bail!("GET /csrf-token returned {}", status);
        }
        let v: Value = resp.json().await.context("parsing /csrf-token response")?;
        self.csrf = v
            .get("token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| anyhow!("/csrf-token response had no token"))?
            .to_string();
        Ok(())
    }

    /// Start a POST request with the CSRF header attached.
    pub fn post(&self, path: &str) -> RequestBuilder {
        self.http
            .post(format!("{}{}", self.base, path))
            .header("x-csrf-token", self.csrf.as_str())
    }

    /// The card picture (`GET /characters/<avatar>`: the card PNG itself).
    pub async fn character_image(&self, avatar: &str) -> Result<Vec<u8>> {
        let mut url = reqwest::Url::parse(&self.base).context("bad server address")?;
        url.path_segments_mut()
            .map_err(|_| anyhow!("bad server address"))?
            .pop_if_empty()
            .push("characters")
            .push(avatar);
        let resp = self
            .http
            .get(url)
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .context("GET /characters/<avatar>")?;
        let status = resp.status();
        if !status.is_success() {
            bail!("the server has no picture for this character ({})", status);
        }
        Ok(resp.bytes().await.context("reading the picture")?.to_vec())
    }

    async fn post_json(&self, path: &str, body: &Value) -> Result<Value> {
        let resp = self
            .post(path)
            .timeout(Duration::from_secs(30))
            .json(body)
            .send()
            .await
            .with_context(|| format!("POST {}", path))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("{} returned {}: {}", path, status, short(&text, 300));
        }
        serde_json::from_str(&text).with_context(|| format!("invalid JSON from {}", path))
    }

    /// The user's settings.json (as served by `/api/settings/get`), parsed.
    pub async fn settings(&self) -> Result<Value> {
        let v = self.post_json("/api/settings/get", &json!({})).await?;
        let raw = v
            .get("settings")
            .and_then(|s| s.as_str())
            .ok_or_else(|| anyhow!("settings response had no `settings` string"))?;
        serde_json::from_str(raw).context("parsing SillyTavern settings.json")
    }

    pub async fn characters(&self) -> Result<Vec<Value>> {
        let v = self.post_json("/api/characters/all", &json!({})).await?;
        match v {
            Value::Array(a) => Ok(a),
            _ => bail!("unexpected response from /api/characters/all"),
        }
    }

    /// Chat files for a character. Each entry has `file_name` ("x.jsonl"),
    /// `file_id` and `last_mes` (file mtime in ms).
    pub async fn chats(&self, avatar: &str) -> Result<Vec<Value>> {
        let v = self
            .post_json("/api/characters/chats", &json!({ "avatar_url": avatar }))
            .await?;
        match v {
            Value::Array(a) => Ok(a),
            _ => Ok(Vec::new()), // `{error: true}` when the character has no chat folder yet
        }
    }

    /// Raw chat lines: first element is the header, the rest are messages.
    pub async fn load_chat(&self, avatar: &str, ch_name: &str, file: &str) -> Result<Vec<Value>> {
        let v = self
            .post_json(
                "/api/chats/get",
                &json!({ "ch_name": ch_name, "file_name": file, "avatar_url": avatar }),
            )
            .await?;
        match v {
            Value::Array(a) => Ok(a),
            _ => Ok(Vec::new()), // `{}` when the file does not exist
        }
    }

    /// Ask the server to download a card from a URL (chub.ai, ...), like the
    /// web UI's "Import content from URL".
    pub async fn import_url(&self, url: &str) -> Result<Download> {
        let resp = self
            .post("/api/content/importURL")
            .timeout(Duration::from_secs(90))
            .json(&json!({ "url": url }))
            .send()
            .await
            .context("POST /api/content/importURL")?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 404 {
                bail!("SillyTavern could not fetch {} (not found or not supported)", url);
            }
            bail!("download failed ({}): {}", status, short(&text, 200));
        }
        let header = |name: &str| -> String {
            resp.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string()
        };
        let kind = header("x-custom-content-type");
        let file_name = crate::import::filename_from_disposition(&header("content-disposition"));
        let bytes = resp.bytes().await.context("reading download")?.to_vec();
        Ok(Download {
            bytes,
            file_name,
            kind,
        })
    }
}

/// A file the server downloaded for us (`/api/content/importURL`).
pub struct Download {
    pub bytes: Vec<u8>,
    pub file_name: String,
    /// `character` or `lorebook` (the `X-Custom-Content-Type` header).
    pub kind: String,
}

pub fn short(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{}…", cut)
    }
}
