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

    /// llama.cpp's own `POST /tokenize` (`{"content": text}` -> `{"tokens": [...]}`),
    /// asked directly at the backend address SillyTavern has for it.
    async fn count_tokens_llamacpp(&self, text: &str, server: &str) -> Result<usize> {
        let base = server.trim_end_matches('/');
        let base = base.strip_suffix("/v1").unwrap_or(base);
        let path = format!("{}/tokenize", base);
        let resp = self
            .http
            .post(&path)
            .timeout(Duration::from_secs(60))
            .json(&json!({ "content": text }))
            .send()
            .await
            .with_context(|| format!("POST {}", path))?;
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            bail!("{} returned {}: {}", path, status, short(&body, 200));
        }
        let v: Value = serde_json::from_str(&body).with_context(|| format!("invalid JSON from {}", path))?;
        v.get("tokens")
            .and_then(|t| t.as_array())
            .map(|a| a.len())
            .ok_or_else(|| anyhow!("{} gave no \"tokens\" list", path))
    }

    /// Exact token count of `text` from the text-completion backend. For
    /// llama.cpp the backend is asked directly first; otherwise (or when
    /// that does not work, e.g. st-cli runs on another machine than the
    /// backend) through SillyTavern's tokenizer endpoint, the call the web
    /// UI makes.
    ///
    /// The backend address is the one SillyTavern uses, so it is relative to
    /// the SillyTavern machine. When SillyTavern is remote (a phone talking
    /// to a PC) and that address is a loopback one (`127.0.0.1`,
    /// `localhost`), it points at the backend on the SillyTavern machine,
    /// not at this device: asking it directly would hit the wrong host, so
    /// the direct attempt is skipped and SillyTavern does the counting.
    pub async fn count_tokens(
        &self,
        text: &str,
        api_type: &str,
        server: &str,
        model: &str,
    ) -> Result<usize> {
        let mut direct_err = None;
        let st_remote = !host_is_loopback(&self.base);
        let direct_ok = api_type == "llamacpp" && !(st_remote && host_is_loopback(server));
        if direct_ok {
            match self.count_tokens_llamacpp(text, server).await {
                Ok(n) => return Ok(n),
                Err(e) => direct_err = Some(format!("{:#}", e)),
            }
        }
        match self.count_tokens_via_st(text, api_type, server, model).await {
            Ok(n) => Ok(n),
            Err(e) => match direct_err {
                Some(d) => bail!("{}; via SillyTavern: {:#}", d, e),
                None => Err(e),
            },
        }
    }

    /// SillyTavern's `POST /api/tokenizers/remote/textgenerationwebui/encode`.
    /// The server reads the backend address from `url` (not `api_server`) and,
    /// for vLLM / Aphrodite, the model from `vllm_model` / `aphrodite_model`.
    /// It answers `{ "error": true }` when it cannot reach the backend.
    async fn count_tokens_via_st(
        &self,
        text: &str,
        api_type: &str,
        server: &str,
        model: &str,
    ) -> Result<usize> {
        let v = self
            .post_json(
                "/api/tokenizers/remote/textgenerationwebui/encode",
                &json!({
                    "text": text,
                    "api_type": api_type,
                    "url": server,
                    "vllm_model": model,
                    "aphrodite_model": model,
                }),
            )
            .await?;
        if v.get("error").and_then(|e| e.as_bool()).unwrap_or(false) {
            bail!(
                "SillyTavern could not get a token count from the {} backend at {} (see the SillyTavern console)",
                api_type,
                server
            );
        }
        v.get("count")
            .and_then(|c| c.as_u64())
            .or_else(|| v.get("ids").and_then(|i| i.as_array()).map(|a| a.len() as u64))
            .map(|n| n as usize)
            .ok_or_else(|| anyhow!("tokenizer response had no count"))
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

/// Whether the host of `url` is this machine's own loopback address
/// (`localhost`, `127.0.0.1`, `::1`, `0.0.0.0`), with or without a scheme,
/// port, path or `user@`.
fn host_is_loopback(url: &str) -> bool {
    let u = url.trim();
    let u = u.split_once("://").map(|(_, rest)| rest).unwrap_or(u);
    let authority = u
        .split(|c: char| c == '/' || c == '?' || c == '#')
        .next()
        .unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    let host = match authority.strip_prefix('[') {
        Some(rest) => rest.split(']').next().unwrap_or(""),
        None => authority.split(':').next().unwrap_or(""),
    };
    let host = host.to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|ip| ip.is_loopback() || ip.is_unspecified())
        .unwrap_or(false)
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

#[cfg(test)]
mod tests {
    use super::host_is_loopback;

    #[test]
    fn loopback_hosts() {
        for u in [
            "http://127.0.0.1:8080",
            "http://localhost:8080/",
            "127.0.0.1:5001",
            "http://[::1]:8080",
            "http://0.0.0.0:8080/v1",
            "http://user@localhost:1234",
            "HTTP://LOCALHOST",
        ] {
            assert!(host_is_loopback(u), "{}", u);
        }
    }

    #[test]
    fn remote_hosts() {
        for u in [
            "http://192.168.1.20:8080",
            "http://my-pc.local:8080",
            "https://example.com/localhost",
            "http://127.example.com",
            "",
        ] {
            assert!(!host_is_loopback(u), "{}", u);
        }
    }
}
