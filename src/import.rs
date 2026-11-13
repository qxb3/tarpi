//! `/import <file | link | json>`: add a character card to tarpi's own
//! characters folder (`~/.config/tarpi/characters/`, see `charstore`) from
//! inside the chat screen (or the character menu shown before it).
//!
//! The card is stored as it came: a PNG with an embedded card stays a PNG, a
//! JSON card (V1, V2, V3, Pygmalion) stays JSON. chub.ai / characterhub.org and
//! pygmalion.chat links are downloaded directly from those sites. This module
//! checks the input first so that a wrong file fails with a clear message.
//!
//! The card is written into the folder `config::characters_dir` names, which
//! under `tarpi connect` is the local copy of the server's data: the running
//! sync uploads the new card from there, so nothing here knows about it.
//! Nothing in this module prints: it runs while the chat screen is open.

use crate::charstore;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::path::PathBuf;

/// What `/import` with nothing after it says.
pub const USAGE: &str = "usage: /import <card .png or .json file, or a chub.ai / pygmalion.chat link>";

/// What the user gave us.
#[derive(Debug, PartialEq)]
enum Source {
    File(PathBuf),
    Inline(String),
    Url(String),
}

/// Sites a card can be downloaded from.
#[derive(Debug, PartialEq)]
enum Site {
    Chub,
    Pygmalion,
    /// Any other http(s) URL: taken to point straight at a card `.json` / `.png`.
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Png,
    Json,
}

impl Kind {
    fn file_type(self) -> &'static str {
        match self {
            Kind::Png => "png",
            Kind::Json => "json",
        }
    }
}

/// Add the card `arg` names to the characters folder: a card file (`.png` /
/// `.json`), a chub.ai / pygmalion.chat character link, a direct link to a
/// card file, or the card's JSON text. The card is checked first and a name
/// that is taken gets `-2`, `-3`, ... (see `charstore::save`).
pub async fn import(arg: &str) -> Result<charstore::Saved> {
    let (bytes, file_name) = fetch(arg).await?;
    charstore::save(&bytes, &file_name, None, false)
}

/// The card's bytes and a file name for it, checked to be a usable card.
async fn fetch(arg: &str) -> Result<(Vec<u8>, String)> {
    match classify(arg)? {
        Source::File(path) => {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            validate(&bytes)
                .with_context(|| format!("{} is not a usable character card", path.display()))?;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "card".to_string());
            Ok((bytes, name))
        }
        Source::Inline(text) => {
            let bytes = text.into_bytes();
            validate(&bytes).context("that JSON is not a usable character card")?;
            Ok((bytes, "card.json".to_string()))
        }
        Source::Url(u) => {
            let site = check_url(&u)?;
            let (bytes, name) = match site {
                Site::Chub => download_chub(&u).await?,
                Site::Pygmalion => download_pygmalion(&u).await?,
                Site::Direct => download_direct(&u).await?,
            };
            let kind = validate(&bytes).context("the downloaded file is not a character card")?;
            let name = if name.is_empty() {
                format!("card.{}", kind.file_type())
            } else {
                name
            };
            Ok((bytes, name))
        }
    }
}

/// A path as typed or pasted into the chat box: it may be in quotes, have
/// `\ ` for its spaces (how terminals paste a dragged file), be a `file://`
/// link or start with `~` (the shell does none of that here).
pub(crate) fn expand_path(t: &str) -> PathBuf {
    let mut s = t.trim().to_string();
    let quoted = s.len() >= 2
        && ((s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')));
    if quoted {
        s = s[1..s.len() - 1].to_string();
    }
    if let Some(rest) = s.strip_prefix("file://") {
        s = percent_decode(rest);
    } else {
        s = s.replace("\\ ", " ");
    }
    if s == "~" || s.starts_with("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(s[1..].trim_start_matches('/'));
        }
    }
    PathBuf::from(s)
}

/// Decide what `arg` is: JSON text, a URL, or a file.
fn classify(arg: &str) -> Result<Source> {
    let t = arg.trim();
    if t.starts_with('{') {
        return Ok(Source::Inline(t.to_string()));
    }
    let lower = t.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Ok(Source::Url(t.to_string()));
    }
    let path = expand_path(t);
    if path.is_file() {
        return Ok(Source::File(path));
    }
    // A URL typed without the scheme.
    if host_of(&format!("https://{}", t))
        .is_some_and(|h| is_chub_host(&h) || is_pygmalion_host(&h))
        && t.contains('/')
    {
        return Ok(Source::Url(format!("https://{}", t)));
    }
    if path.is_dir() {
        bail!("{} is a directory; give a card file", t);
    }
    bail!(
        "'{}' is not an existing file, a chub.ai / pygmalion.chat link, a link to a card \
         file or JSON text",
        t
    )
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    if host.is_empty() {
        return None;
    }
    Some(host.to_lowercase())
}

fn is_chub_host(host: &str) -> bool {
    let h = host.strip_prefix("www.").unwrap_or(host);
    h == "chub.ai" || h == "characterhub.org"
}

fn is_pygmalion_host(host: &str) -> bool {
    let h = host.strip_prefix("www.").unwrap_or(host);
    h == "pygmalion.chat"
}

/// Supported sites: chub.ai / characterhub.org and pygmalion.chat. SillyTavern
/// accepts a few more; they can be added here later.
fn check_url(url: &str) -> Result<Site> {
    let host = host_of(url).ok_or_else(|| anyhow!("'{}' is not a valid URL", url))?;
    if is_pygmalion_host(&host) {
        return check_pygmalion_url(url);
    }
    if !is_chub_host(&host) {
        // Not a card site: a direct link to a card file (checked once downloaded).
        return Ok(Site::Direct);
    }
    let path = url
        .split_once("://")
        .map(|x| x.1)
        .unwrap_or(url)
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .split_once('/')
        .map(|x| x.1)
        .unwrap_or("");
    let seg: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if seg.first() == Some(&"lorebooks") {
        bail!("that is a lorebook URL. tarpi imports characters only.");
    }
    let n = if seg.first() == Some(&"characters") {
        seg.len().saturating_sub(1)
    } else {
        seg.len()
    };
    if n < 2 {
        bail!(
            "expected a character URL like https://chub.ai/characters/<creator>/<name>, got '{}'",
            url
        );
    }
    Ok(Site::Chub)
}

/// The HTTP client of the downloads.
fn http_client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        // Some hosts refuse clients without a browser-like User-Agent.
        .user_agent("Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36")
        .build()?)
}

/// The path segments of a URL (everything after the host, without the query).
fn url_segments(url: &str) -> Vec<String> {
    url.split_once("://")
        .map(|x| x.1)
        .unwrap_or(url)
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .split_once('/')
        .map(|x| x.1)
        .unwrap_or("")
        .split('/')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// `creator/name` of a chub.ai character URL (`/characters/<creator>/<name>`
/// or just `/<creator>/<name>`).
fn chub_full_path(url: &str) -> Result<String> {
    let seg = url_segments(url);
    let seg: &[String] = if seg.first().map(|s| s.as_str()) == Some("characters") {
        &seg[1..]
    } else {
        &seg[..]
    };
    if seg.len() < 2 {
        bail!("expected a character URL like https://chub.ai/characters/<creator>/<name>");
    }
    Ok(format!("{}/{}", seg[0], seg[1]))
}

/// The card PNG of a chub.ai character, as chub's image host serves it (the
/// card is embedded in it).
fn chub_cdn_url(full: &str) -> String {
    format!("https://avatars.charhub.io/avatars/{}/chara_card_v2.png", full)
}

/// Send a request that should answer with a card; the bytes are returned only
/// if they really are one (an error page or a plain picture is not).
async fn fetch_card(rb: reqwest::RequestBuilder) -> Result<Vec<u8>> {
    let resp = rb.send().await.context("no answer")?;
    let status = resp.status();
    if !status.is_success() {
        bail!("{}", status);
    }
    let bytes = resp.bytes().await.context("reading the download")?.to_vec();
    if bytes.len() > MAX_DIRECT {
        bail!("too large for a character card");
    }
    validate(&bytes).map_err(|e| anyhow!("not a card ({}, {})", sniff(&bytes), e))?;
    Ok(bytes)
}

/// Download a character from chub.ai (also characterhub.org). chub changes
/// its endpoints now and then, so several are tried in turn: the card PNG on
/// the image host first, then the download endpoints of the API.
async fn download_chub(url: &str) -> Result<(Vec<u8>, String)> {
    let full = chub_full_path(url)?;
    let name = full.rsplit('/').next().unwrap_or("card").to_string();
    let client = http_client()?;
    let mut tried: Vec<String> = Vec::new();

    let cdn = chub_cdn_url(&full);
    match fetch_card(client.get(&cdn).header("Referer", "https://chub.ai/")).await {
        Ok(b) => return Ok((b, format!("{}.png", name))),
        Err(e) => tried.push(format!("avatars.charhub.io: {:#}", e)),
    }
    for host in ["https://gateway.chub.ai", "https://api.chub.ai"] {
        let rb = client
            .post(format!("{}/api/characters/download", host))
            .header("Referer", "https://chub.ai/")
            .json(&serde_json::json!({ "format": "tavern", "fullPath": full, "version": "main" }));
        match fetch_card(rb).await {
            Ok(b) => return Ok((b, format!("{}.png", name))),
            Err(e) => tried.push(format!("{}: {:#}", host.trim_start_matches("https://"), e)),
        }
    }
    if tried.iter().all(|t| t.contains("404")) {
        bail!("chub.ai has no character '{}' (or it is private)", full);
    }
    bail!(
        "chub.ai download failed for '{}':\n  - {}\nDownload the card PNG from the character's page \
         in a browser and import the file instead.",
        full,
        tried.join("\n  - ")
    )
}

/// Download a character from pygmalion.chat: its export endpoint returns the
/// card as JSON (`{"character": {...}}`).
async fn download_pygmalion(url: &str) -> Result<(Vec<u8>, String)> {
    let seg = url_segments(url);
    let id = match seg.as_slice() {
        [a, id] if a == "character" => id.clone(),
        _ => bail!("expected a character URL like https://pygmalion.chat/character/<id>"),
    };
    let resp = http_client()?
        .get(format!(
            "https://server.pygmalion.chat/api/export/character/{}/v2",
            id
        ))
        .send()
        .await
        .context("downloading from pygmalion.chat")?;
    let status = resp.status();
    if !status.is_success() {
        bail!("pygmalion.chat download failed ({})", status);
    }
    let body: Value = resp
        .json()
        .await
        .context("pygmalion.chat did not return JSON")?;
    let card = body.get("character").cloned().unwrap_or(body);
    let bytes = serde_json::to_vec(&card)?;
    Ok((bytes, "card.json".to_string()))
}

/// Largest card file taken from a direct link.
const MAX_DIRECT: usize = 50 * 1024 * 1024;

/// Download a card `.json` / `.png` from a plain link. Returns the bytes and a
/// file name taken from the URL path (`card.json` / `card.png` when it has none).
async fn download_direct(url: &str) -> Result<(Vec<u8>, String)> {
    let client = http_client()?;
    let resp = client
        .get(url)
        .header("Accept", "application/json, image/png, */*")
        .send()
        .await
        .with_context(|| format!("downloading {}", url))?;
    let status = resp.status();
    if !status.is_success() {
        bail!("download failed ({}): {}", status, url);
    }
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if resp.content_length().is_some_and(|n| n as usize > MAX_DIRECT) {
        bail!("{} is too large for a character card", url);
    }
    let bytes = resp.bytes().await.context("reading download")?.to_vec();
    if bytes.len() > MAX_DIRECT {
        bail!("{} is too large for a character card", url);
    }
    if !bytes.starts_with(&PNG_SIG) {
        let t = bytes.iter().skip_while(|b| b.is_ascii_whitespace()).next();
        if t != Some(&b'{') {
            bail!(
                "the link did not return a PNG or JSON card: got {} ({} bytes, {}). \
                 Thumbnail / preview links are re-encoded images without the card data; \
                 use the site's real download link for the card.",
                if ctype.is_empty() { "an unknown type" } else { &ctype },
                bytes.len(),
                sniff(&bytes)
            );
        }
    }
    let last = url
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('/')
        .next()
        .unwrap_or("");
    let name = percent_decode(last);
    let name = if name.contains('.') && !name.contains("://") {
        name
    } else {
        String::new()
    };
    Ok((bytes, name))
}

/// A short description of what a downloaded blob looks like.
fn sniff(b: &[u8]) -> &'static str {
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "a JPEG image"
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        "a WebP image"
    } else if b.starts_with(b"GIF8") {
        "a GIF image"
    } else {
        let head = String::from_utf8_lossy(&b[..b.len().min(64)]).to_lowercase();
        if head.contains("<!doctype") || head.contains("<html") {
            "an HTML page"
        } else {
            "unrecognised data"
        }
    }
}

/// A pygmalion.chat character page: `https://pygmalion.chat/character/<id>`
/// (the id is a UUID; the card is fetched from pygmalion's server by it).
fn check_pygmalion_url(url: &str) -> Result<Site> {
    let path = url
        .split_once("://")
        .map(|x| x.1)
        .unwrap_or(url)
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .split_once('/')
        .map(|x| x.1)
        .unwrap_or("");
    let seg: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if seg.len() == 2 && seg[0] == "character" && !seg[1].is_empty() {
        return Ok(Site::Pygmalion);
    }
    bail!(
        "expected a character URL like https://pygmalion.chat/character/<id>, got '{}'",
        url
    )
}

/// Detect and sanity-check a card:
/// a PNG with an embedded `chara` / `ccv3` text chunk, or JSON that is a V2/V3
/// card (`spec`), a V1 card (`name`) or a Pygmalion card (`char_name`).
fn validate(bytes: &[u8]) -> Result<Kind> {
    if bytes.starts_with(&PNG_SIG) {
        charstore::check_png_card(bytes).context(
            "this PNG is not a character card (a card has its data embedded in a `chara` or \
             `ccv3` text chunk; a plain image has none)",
        )?;
        return Ok(Kind::Png);
    }
    // A file saved by some editors starts with a byte order mark.
    let text = String::from_utf8_lossy(bytes);
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| anyhow!("not a PNG, and not valid JSON either ({})", e))?;
    check_card_json(&v)?;
    Ok(Kind::Json)
}

fn check_card_json(v: &Value) -> Result<()> {
    if !v.is_object() {
        bail!("the JSON is not an object");
    }
    if let Some(spec) = v.get("spec") {
        let s = spec.as_str().unwrap_or("");
        if !s.starts_with("chara_card_v") {
            bail!("unknown card spec {} (expected chara_card_v2 or v3)", spec);
        }
        return Ok(());
    }
    let has_name = |o: &Value, k: &str| o.get(k).and_then(|x| x.as_str()).is_some();
    if has_name(v, "name") || has_name(v, "char_name") {
        return Ok(());
    }
    if v.get("entries").is_some() {
        bail!("this looks like a lorebook / World Info file, not a character card");
    }
    bail!("no `spec`, `name` or `char_name` field: not a V1, V2/V3 or Pygmalion card");
}

const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chub_urls_give_creator_and_name() {
        for u in [
            "https://chub.ai/characters/user/sofia-1a2b3c",
            "https://www.chub.ai/characters/user/sofia-1a2b3c?tab=x",
            "https://chub.ai/user/sofia-1a2b3c",
        ] {
            assert_eq!(chub_full_path(u).unwrap(), "user/sofia-1a2b3c", "{}", u);
        }
        assert!(chub_full_path("https://chub.ai/characters/user").is_err());
    }

    #[test]
    fn chub_cards_come_from_the_image_host() {
        assert_eq!(
            chub_cdn_url("cutenotlewd/cricket-674c71b2"),
            "https://avatars.charhub.io/avatars/cutenotlewd/cricket-674c71b2/chara_card_v2.png"
        );
    }

    /// A PNG made of nothing but a `chara` text chunk and the end chunk (the
    /// checks read chunks, not pixels, and do not look at the CRCs).
    fn tiny_card_png(card_b64: &str) -> Vec<u8> {
        let mut v = PNG_SIG.to_vec();
        let mut body = b"chara\0".to_vec();
        body.extend_from_slice(card_b64.as_bytes());
        v.extend_from_slice(&(body.len() as u32).to_be_bytes());
        v.extend_from_slice(b"tEXt");
        v.extend_from_slice(&body);
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(b"IEND");
        v.extend_from_slice(&[0, 0, 0, 0]);
        v
    }

    #[test]
    fn local_cards_are_recognised() {
        // `{"name":"A"}` in base64
        assert_eq!(validate(&tiny_card_png("eyJuYW1lIjoiQSJ9")).unwrap(), Kind::Png);
        assert!(validate(&tiny_card_png("not base64!")).is_err());
        let mut plain = PNG_SIG.to_vec();
        plain.extend_from_slice(&[0, 0, 0, 0]);
        plain.extend_from_slice(b"IEND");
        plain.extend_from_slice(&[0, 0, 0, 0]);
        assert!(validate(&plain).is_err());
        assert_eq!(validate(br#"{"name":"A"}"#).unwrap(), Kind::Json);
        assert_eq!(validate("\u{feff}{\"name\":\"A\"}".as_bytes()).unwrap(), Kind::Json);
        assert_eq!(
            validate(br#"{"spec":"chara_card_v2","data":{"name":"A"}}"#).unwrap(),
            Kind::Json
        );
        assert!(validate(br#"{"entries":{}}"#).is_err());
        assert!(validate(b"hello").is_err());
    }

    #[test]
    fn pygmalion_urls() {
        let id = "0a1b2c3d-1111-2222-3333-444455556666";
        assert_eq!(
            check_url(&format!("https://pygmalion.chat/character/{}", id)).unwrap(),
            Site::Pygmalion
        );
        assert_eq!(
            check_url(&format!("https://www.pygmalion.chat/character/{}?x=1#a", id)).unwrap(),
            Site::Pygmalion
        );
        assert!(check_url("https://pygmalion.chat/").is_err());
        assert!(check_url("https://pygmalion.chat/character/").is_err());
        assert!(check_url("https://pygmalion.chat/explore").is_err());
    }

    #[test]
    fn scheme_less_and_other_sites() {
        assert!(matches!(
            classify("pygmalion.chat/character/abc").unwrap(),
            Source::Url(_)
        ));
        assert_eq!(check_url("https://chub.ai/characters/u/n").unwrap(), Site::Chub);
        assert_eq!(check_url("https://example.com/c/sofia.png").unwrap(), Site::Direct);
        assert_eq!(check_url("https://example.com/c/sofia.json?x=1").unwrap(), Site::Direct);
    }

    #[test]
    fn typed_paths_are_understood() {
        assert_eq!(expand_path("/tmp/a b.png"), PathBuf::from("/tmp/a b.png"));
        assert_eq!(expand_path("'/tmp/a b.png'"), PathBuf::from("/tmp/a b.png"));
        assert_eq!(expand_path("\"/tmp/a b.png\""), PathBuf::from("/tmp/a b.png"));
        assert_eq!(expand_path("/tmp/a\\ b.png"), PathBuf::from("/tmp/a b.png"));
        assert_eq!(expand_path("file:///tmp/a%20b.png"), PathBuf::from("/tmp/a b.png"));
        if let Some(home) = dirs::home_dir() {
            assert_eq!(expand_path("~/cards/a.png"), home.join("cards/a.png"));
            assert_eq!(expand_path("~"), home.join(""));
        }
        // Not a home directory reference.
        assert_eq!(expand_path("~x/a.png"), PathBuf::from("~x/a.png"));
    }

    #[test]
    fn classify_tells_the_kinds_apart() {
        assert!(matches!(classify("{\"name\":\"A\"}").unwrap(), Source::Inline(_)));
        assert!(matches!(
            classify("https://chub.ai/characters/u/n").unwrap(),
            Source::Url(_)
        ));
        assert!(classify("/definitely/not/here.png").is_err());
    }
}
