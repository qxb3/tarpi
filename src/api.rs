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

    /// Write the whole settings.json back (`/api/settings/save`), which is
    /// what the web UI does when a setting changes.
    pub async fn save_settings(&self, settings: &Value) -> Result<()> {
        self.post_json("/api/settings/save", settings).await?;
        Ok(())
    }

    pub async fn characters(&self) -> Result<Vec<Value>> {
        let v = self.post_json("/api/characters/all", &json!({})).await?;
        match v {
            Value::Array(a) => Ok(a),
            _ => bail!("unexpected response from /api/characters/all"),
        }
    }

    /// Full character card (not the shallow list entry).
    pub async fn character(&self, avatar: &str) -> Result<Value> {
        self.post_json("/api/characters/get", &json!({ "avatar_url": avatar }))
            .await
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

    /// Delete a chat file for good (`/api/chats/delete`, like the web UI's
    /// "delete chat" button). `file` is the name without `.jsonl`.
    pub async fn delete_chat(&self, avatar: &str, file: &str) -> Result<()> {
        let name = if file.ends_with(".jsonl") {
            file.to_string()
        } else {
            format!("{}.jsonl", file)
        };
        let body = json!({ "chatfile": name, "avatar_url": avatar });
        let resp = self
            .post("/api/chats/delete")
            .timeout(Duration::from_secs(30))
            .json(&body)
            .send()
            .await
            .context("POST /api/chats/delete")?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        bail!("delete failed ({}): {}", status, short(&text, 200));
    }

    /// Add a user avatar image (`/api/avatars/upload`, the web UI's persona
    /// "create" button). Returns the file name the server gave it.
    pub async fn upload_avatar(&self, png: Vec<u8>) -> Result<String> {
        let part = reqwest::multipart::Part::bytes(png)
            .file_name("persona.png".to_string())
            .mime_str("image/png")?;
        let form = reqwest::multipart::Form::new().part("avatar", part);
        let resp = self
            .post("/api/avatars/upload")
            .timeout(Duration::from_secs(60))
            .multipart(form)
            .send()
            .await
            .context("POST /api/avatars/upload")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("avatar upload failed ({}): {}", status, short(&text, 200));
        }
        let v: Value = serde_json::from_str(&text).context("invalid JSON from avatar upload")?;
        v.get("path")
            .and_then(|p| p.as_str())
            .map(|p| p.to_string())
            .ok_or_else(|| anyhow!("avatar upload had no file name: {}", short(&text, 200)))
    }

    /// Delete a user avatar image (`/api/avatars/delete`).
    pub async fn delete_avatar(&self, avatar: &str) -> Result<()> {
        self.post_json("/api/avatars/delete", &json!({ "avatar": avatar }))
            .await?;
        Ok(())
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

    /// Upload a card to `/api/characters/import` (the web UI's import button).
    /// `file_type` is `png` or `json`. Returns the new avatar name without ".png".
    pub async fn import_character(
        &self,
        bytes: Vec<u8>,
        file_name: &str,
        file_type: &str,
        mime: &str,
    ) -> Result<String> {
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(file_name.to_string())
            .mime_str(mime)?;
        let form = reqwest::multipart::Form::new()
            .text("file_type", file_type.to_string())
            .part("avatar", part);
        let resp = self
            .post("/api/characters/import")
            .timeout(Duration::from_secs(60))
            .multipart(form)
            .send()
            .await
            .context("POST /api/characters/import")?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!(
                "SillyTavern refused the card ({}): {}",
                status,
                short(&text, 200)
            );
        }
        let v: Value = serde_json::from_str(&text).context("invalid JSON from import")?;
        if v.get("error").and_then(|e| e.as_bool()).unwrap_or(false) {
            bail!("SillyTavern could not read this file as a character card");
        }
        v.get("file_name")
            .and_then(|f| f.as_str())
            .map(|f| f.to_string())
            .ok_or_else(|| anyhow!("import response had no file_name: {}", short(&text, 200)))
    }

    /// Delete a character card. With `delete_chats`, its whole chat folder
    /// goes too (`/api/characters/delete`, as in the web UI).
    pub async fn delete_character(&self, avatar: &str, delete_chats: bool) -> Result<()> {
        let resp = self
            .post("/api/characters/delete")
            .timeout(Duration::from_secs(60))
            .json(&json!({ "avatar_url": avatar, "delete_chats": delete_chats }))
            .send()
            .await
            .context("POST /api/characters/delete")?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        bail!("delete failed ({}): {}", status, short(&text, 200));
    }

    /// Set one field of a character card for good (`/api/characters/edit-attribute`,
    /// which writes both the top-level and the `data.*` copy). SillyTavern only
    /// answers when it worked or the field is unknown; on other failures it
    /// stays silent, hence the timeout.
    pub async fn edit_card_attribute(
        &self,
        avatar: &str,
        ch_name: &str,
        field: &str,
        value: &Value,
    ) -> Result<()> {
        let body = json!({
            "avatar_url": avatar,
            "ch_name": ch_name,
            "field": field,
            "value": value,
        });
        let resp = self
            .post("/api/characters/edit-attribute")
            .timeout(Duration::from_secs(20))
            .json(&body)
            .send()
            .await
            .with_context(|| {
                format!(
                    "SillyTavern did not confirm the change to `{}` (the card may be an \
                     old V1 card without a data block)",
                    field
                )
            })?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        bail!("saving to the card failed ({}): {}", status, short(&text, 200));
    }

    pub async fn save_chat(
        &self,
        avatar: &str,
        ch_name: &str,
        file: &str,
        lines: &[Value],
    ) -> Result<()> {
        let body = json!({
            "ch_name": ch_name,
            "file_name": file,
            "avatar_url": avatar,
            "chat": lines,
            "force": false,
        });
        let resp = self
            .post("/api/chats/save")
            .timeout(Duration::from_secs(30))
            .json(&body)
            .send()
            .await
            .context("POST /api/chats/save")?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        if text.contains("integrity") {
            bail!(
                "save refused: the chat file changed elsewhere (e.g. in the web UI). \
                 Reopen st-cli to reload it."
            );
        }
        bail!("save failed ({}): {}", status, short(&text, 200));
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
