//! `st-cli import <file | url | json>`: add a character card to SillyTavern.
//!
//! The card itself is never converted here. SillyTavern's own
//! `POST /api/characters/import` reads V1, V2/V3 and Pygmalion JSON as well as
//! PNGs with an embedded card, so the file is sent as it is and the server
//! saves it exactly like the web UI's "Import character" button does. For
//! URLs, `POST /api/content/importURL` makes the server download the card
//! (chub.ai / characterhub.org / pygmalion.chat). This module only checks the input first so
//! that a wrong file fails with a clear message instead of a bare 400.

use crate::api::Api;
use crate::config;
use crate::jv::*;
use anyhow::{anyhow, bail, Context, Result};
use clap::Parser;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "st-cli import",
    about = "Import a character card into SillyTavern",
    long_about = "Import a character card into SillyTavern. The card is saved by the \
                  SillyTavern server itself (like the web UI's import button), so the \
                  character shows up in the web UI right away.",
    after_help = concat!(
        "EXAMPLES:\n",
        "  st-cli import sofia.png                        V2/V3 card PNG\n",
        "  st-cli import sofia.json                       card JSON (V1, V2, V3, Pygmalion)\n",
        "  st-cli import '{\"name\":\"Sofia\", ...}'          JSON given directly\n",
        "  st-cli import https://chub.ai/characters/user/sofia-1a2b3c\n",
        "  st-cli import chub.ai/characters/user/sofia-1a2b3c\n",
        "  st-cli import https://pygmalion.chat/character/<id>"
    )
)]
struct ImportCli {
    /// A card file (.png / .json), a chub.ai or pygmalion.chat character URL, or JSON text
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
    fn mime(self) -> &'static str {
        match self {
            Kind::Png => "image/png",
            Kind::Json => "application/json",
        }
    }
}

pub async fn run() -> Result<()> {
    // argv is `st-cli import ...`; drop the program name so that "import"
    // plays the part of the binary name in clap's messages.
    let cli = ImportCli::parse_from(std::env::args().skip(1));
    let cfg = config::load()?;
    let server = cfg
        .url
        .clone()
        .unwrap_or_else(|| "http://127.0.0.1:8000".to_string());

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

    let api = Api::connect(&server).await.with_context(|| {
        format!(
            "Is SillyTavern running at {}? Start it, or set `url:` in \
             ~/.config/st-cli/config.yaml",
            server
        )
    })?;

    let (bytes, file_name, kind) = match (local, remote) {
        (Some(x), _) => x,
        (None, Some((u, site))) => {
            eprintln!("Downloading...");
            let (bytes, name, content) = match site {
                Site::Chub | Site::Pygmalion => {
                    let d = api.import_url(&u).await?;
                    (d.bytes, d.file_name, d.kind)
                }
            };
            if content == "lorebook" {
                bail!(
                    "that URL is a lorebook. st-cli imports characters only \
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

    let saved = api
        .import_character(bytes, &file_name, kind.file_type(), kind.mime())
        .await?;

    // `saved` is the new avatar file name without ".png". Look the character
    // up for a friendly message; failing to do so is not an error.
    let avatar = format!("{}.png", saved);
    let name = api
        .characters()
        .await
        .ok()
        .and_then(|list| {
            list.iter()
                .find(|c| jstr(c, "avatar") == avatar)
                .map(|c| jstr(c, "name"))
        })
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| saved.clone());
    println!("Imported \"{}\" ({})", name, avatar);
    println!("Chat with it:  st-cli \"{}\"", name);
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
        "'{}' is not an existing file, a chub.ai / pygmalion.chat URL or JSON text. \
         See `st-cli import --help`.",
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
        bail!(
            "unsupported site '{}'. Supported: chub.ai, pygmalion.chat. For other \
             sites, download the card and import the file.",
            host
        );
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
        bail!("that is a lorebook URL. st-cli imports characters only.");
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

/// A pygmalion.chat character page: `https://pygmalion.chat/character/<id>`
/// (the id is a UUID; SillyTavern's server fetches the card by it).
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

/// Detect and sanity-check a card, mirroring what SillyTavern accepts:
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

/// Pull the file name out of a `Content-Disposition` header (the server sends
/// it percent-encoded).
pub fn filename_from_disposition(h: &str) -> String {
    let Some(i) = h.find("filename=") else {
        return String::new();
    };
    let rest = h[i + "filename=".len()..].trim_start_matches('"');
    let raw = rest.split('"').next().unwrap_or("").split(';').next().unwrap_or("");
    percent_decode(raw.trim())
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
        assert!(check_url("https://example.com/characters/u/n").is_err());
    }
}
