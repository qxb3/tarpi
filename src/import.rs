//! `tarpi import <file | url | json>`: add a character card to tarpi's own
//! characters folder (`~/.config/tarpi/characters/`, see `charstore`).
//!
//! The card is stored as it came: a PNG with an embedded card stays a PNG, a
//! JSON card (V1, V2, V3, Pygmalion) stays JSON. chub.ai / characterhub.org and
//! pygmalion.chat links are downloaded directly from those sites. This module
//! checks the input first so that a wrong file fails with a clear message.

use crate::charstore;
use crate::config;
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "tarpi import",
    about = "Import a character card into tarpi",
    long_about = "Import a character card into tarpi's own characters folder, \
                  ~/.config/tarpi/characters/. PNG cards keep their picture, JSON-only \
                  cards are stored as JSON. chub.ai and pygmalion.chat links are \
                  downloaded directly from those sites.",
    after_help = concat!(
        "EXAMPLES:\n",
        "  tarpi import sofia.png                        V2/V3 card PNG\n",
        "  tarpi import sofia.json                       card JSON (V1, V2, V3, Pygmalion)\n",
        "  tarpi import '{\"name\":\"Sofia\", ...}'          JSON given directly\n",
        "  tarpi import https://chub.ai/characters/user/sofia-1a2b3c\n",
        "  tarpi import chub.ai/characters/user/sofia-1a2b3c\n",
        "  tarpi import https://pygmalion.chat/character/<id>\n",
        "  tarpi import https://example.com/cards/sofia.png    direct link to a .png / .json card"
    )
)]
struct ImportCli {
    /// A card file (.png / .json), a chub.ai / pygmalion.chat character URL, a direct link to a .png / .json card, or JSON text
    source: String,
}

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

pub async fn run() -> Result<()> {
    // argv is `tarpi import ...`; drop the program name so that "import"
    // plays the part of the binary name in clap's messages.
    let cli = ImportCli::parse_from(std::env::args().skip(1));

    // Check local input before touching the network.
    let source = classify(&cli.source)?;
    let mut remote: Option<(String, Site)> = None;
    let mut local: Option<(Vec<u8>, String, Kind)> = None;
    match source {
        Source::File(path) => {
            let bytes = std::fs::read(&path)
                .with_context(|| format!("reading {}", path.display()))?;
            let kind = validate(&bytes)
                .with_context(|| format!("{} is not a usable character card", path.display()))?;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "card".to_string());
            local = Some((bytes, name, kind));
        }
        Source::Inline(text) => {
            let bytes = text.into_bytes();
            let kind = validate(&bytes).context("that JSON is not a usable character card")?;
            local = Some((bytes, "card.json".to_string(), kind));
        }
        Source::Url(u) => {
            let site = check_url(&u)?;
            remote = Some((u, site));
        }
    }

    let (bytes, file_name, _kind) = match (local, remote) {
        (Some(x), _) => x,
        (None, Some((u, site))) => {
            eprintln!("Downloading...");
            let (bytes, name, content) = match site {
                Site::Chub => {
                    let (bytes, name) = download_chub(&u).await?;
                    (bytes, name, String::new())
                }
                Site::Pygmalion => {
                    let (bytes, name) = download_pygmalion(&u).await?;
                    (bytes, name, String::new())
                }
                Site::Direct => {
                    let (bytes, name) = download_direct(&u).await?;
                    (bytes, name, String::new())
                }
            };
            if content == "lorebook" {
                bail!(
                    "that URL is a lorebook. tarpi imports characters only \
                     (it has no World Info support)."
                );
            }
            let kind = validate(&bytes).context("the downloaded file is not a character card")?;
            let name = if name.is_empty() {
                format!("card.{}", kind.file_type())
            } else {
                name
            };
            (bytes, name, kind)
        }
        (None, None) => unreachable!("a source is either local or a URL"),
    };

    let saved = charstore::save(&bytes, &file_name, None, false)?;
    let name = if saved.name.is_empty() {
        saved.avatar.clone()
    } else {
        saved.name.clone()
    };
    println!(
        "Imported \"{}\" ({}) into {}",
        name,
        saved.avatar,
        config::characters_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_else(|| "the characters folder".to_string())
    );
    println!("Chat with it:  tarpi \"{}\"", name);
    Ok(())
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
    let path = PathBuf::from(t);
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
        "'{}' is not an existing file, a card URL or JSON text. \
         See `tarpi import --help`.",
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

/// Download a character from chub.ai (also characterhub.org): its own
/// download endpoint returns the card as a PNG.
async fn download_chub(url: &str) -> Result<(Vec<u8>, String)> {
    let full = chub_full_path(url)?;
    let resp = http_client()?
        .post("https://api.chub.ai/api/characters/download")
        .json(&serde_json::json!({ "format": "tavern", "fullPath": full, "version": "main" }))
        .send()
        .await
        .context("downloading from chub.ai")?;
    let status = resp.status();
    if !status.is_success() {
        if status.as_u16() == 404 {
            bail!("chub.ai has no character '{}' (or it is private)", full);
        }
        bail!("chub.ai download failed ({})", status);
    }
    let bytes = resp.bytes().await.context("reading download")?.to_vec();
    if bytes.len() > MAX_DIRECT {
        bail!("the download is too large for a character card");
    }
    let name = full.rsplit('/').next().unwrap_or("card");
    Ok((bytes, format!("{}.png", name)))
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
        if !png_has_card(bytes) {
            bail!(
                "this PNG has no embedded character data (no `chara` or `ccv3` chunk). \
                 It is a plain image, not a card."
            );
        }
        return Ok(Kind::Png);
    }
    let v: Value = serde_json::from_slice(bytes)
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

/// Whether the PNG has a tEXt chunk whose keyword is `chara` or `ccv3`
/// (where SillyTavern looks for the base64 card).
fn png_has_card(bytes: &[u8]) -> bool {
    let mut pos = PNG_SIG.len();
    while pos + 8 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
            as usize;
        let kind = &bytes[pos + 4..pos + 8];
        let start = pos + 8;
        let Some(end) = start.checked_add(len) else {
            return false;
        };
        if end > bytes.len() {
            return false;
        }
        if kind == b"tEXt" {
            let data = &bytes[start..end];
            let kw = data.split(|b| *b == 0).next().unwrap_or(&[]);
            let kw = kw.to_ascii_lowercase();
            if kw == b"chara" || kw == b"ccv3" {
                return true;
            }
        }
        if kind == b"IEND" {
            return false;
        }
        pos = end + 4; // skip the CRC
    }
    false
}

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
}
