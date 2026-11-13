//! Character cards: tarpi's own folder, `~/.config/tarpi/characters/`.
//!
//! A card is a file there, either
//! - `<name>.png`: a card PNG with the card embedded (a base64 `chara` and/or
//!   `ccv3` text chunk, the format SillyTavern, chub.ai and friends use), or
//! - `<name>.json`: a card that is only JSON (V1, V2, V3 or Pygmalion).
//!
//! A character is addressed by its *avatar*, `<name>.png`, whichever of the two
//! it is stored as. That is also the key SillyTavern files the character's
//! chats under (chats stay on the server), so a card imported with
//! `tarpi import-st` keeps the avatar name it had there and its old chats are
//! still found.
//!
//! Everything read from a card is turned into the V2 shape the rest of tarpi
//! expects (`name`, `data.*`, `avatar`, `fav`); see [`normalize`]. Cards are
//! only rewritten when a field is edited (`/char`, Card save).

use crate::config;
use crate::jv::*;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::ops::Range;
use std::path::{Path, PathBuf};

const PNG_SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Fields a V2 card also keeps at the top level (as SillyTavern writes them).
const TOP_FIELDS: [&str; 7] = [
    "name",
    "description",
    "personality",
    "scenario",
    "first_mes",
    "mes_example",
    "creator_notes",
];

fn dir() -> Result<PathBuf> {
    config::characters_dir().ok_or_else(|| anyhow!("no config directory on this system"))
}

// ---- base64 and CRC (no extra crates) --------------------------------------

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len() / 3 * 4 + 4);
    for ch in data.chunks(3) {
        let b1 = *ch.get(1).unwrap_or(&0) as u32;
        let b2 = *ch.get(2).unwrap_or(&0) as u32;
        let n = ((ch[0] as u32) << 16) | (b1 << 8) | b2;
        out.push(B64[((n >> 18) & 63) as usize] as char);
        out.push(B64[((n >> 12) & 63) as usize] as char);
        out.push(if ch.len() > 1 { B64[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if ch.len() > 2 { B64[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v: u32 = match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a') as u32 + 26,
            b'0'..=b'9' => (c - b'0') as u32 + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' => break,
            _ if c.is_ascii_whitespace() => continue,
            _ => return None,
        };
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

// ---- PNG chunks ------------------------------------------------------------

struct Chunk {
    kind: [u8; 4],
    /// Length field through CRC.
    full: Range<usize>,
    data: Range<usize>,
}

fn png_chunks(bytes: &[u8]) -> Option<Vec<Chunk>> {
    if !bytes.starts_with(&PNG_SIG) {
        return None;
    }
    let mut pos = PNG_SIG.len();
    let mut out = Vec::new();
    while pos + 12 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
            as usize;
        let kind = [bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]];
        let start = pos + 8;
        let end = start.checked_add(len)?;
        let full_end = end.checked_add(4)?;
        if full_end > bytes.len() {
            return None;
        }
        out.push(Chunk {
            kind,
            full: pos..full_end,
            data: start..end,
        });
        pos = full_end;
        if &kind == b"IEND" {
            break;
        }
    }
    Some(out)
}

/// Keyword (lowercase) of a tEXt chunk and its text.
fn text_parts<'a>(bytes: &'a [u8], c: &Chunk) -> Option<(Vec<u8>, &'a [u8])> {
    if &c.kind != b"tEXt" {
        return None;
    }
    let d = &bytes[c.data.clone()];
    let z = d.iter().position(|b| *b == 0)?;
    Some((d[..z].to_ascii_lowercase(), &d[z + 1..]))
}

fn text_chunk(keyword: &str, text: &str) -> Vec<u8> {
    let mut body = b"tEXt".to_vec();
    body.extend_from_slice(keyword.as_bytes());
    body.push(0);
    body.extend_from_slice(text.as_bytes());
    let len = (body.len() - 4) as u32;
    let mut out = len.to_be_bytes().to_vec();
    out.extend_from_slice(&body);
    out.extend_from_slice(&crc32(&body).to_be_bytes());
    out
}

/// The card JSON embedded in a PNG (`ccv3` first, then `chara`), and whether a
/// `ccv3` chunk was there.
fn read_png_card(bytes: &[u8]) -> Result<(Value, bool)> {
    let chunks = png_chunks(bytes).ok_or_else(|| anyhow!("not a valid PNG file"))?;
    let (mut chara, mut ccv3) = (None, None);
    for c in &chunks {
        if let Some((kw, text)) = text_parts(bytes, c) {
            if kw.as_slice() == b"ccv3" {
                ccv3 = Some(text);
            } else if kw.as_slice() == b"chara" {
                chara = Some(text);
            }
        }
    }
    let had_v3 = ccv3.is_some();
    let text = ccv3
        .or(chara)
        .ok_or_else(|| anyhow!("this PNG has no embedded character card"))?;
    let raw = b64_decode(&String::from_utf8_lossy(text))
        .ok_or_else(|| anyhow!("the card data in this PNG is not valid base64"))?;
    let v: Value = serde_json::from_slice(&raw).context("the card data in this PNG is not JSON")?;
    Ok((v, had_v3))
}

/// The card of a card file as it is written, not yet normalized: the JSON
/// embedded in a PNG, or the JSON of a `.json` card. (`/lore` imports the
/// lorebook of a card with it.)
pub fn raw_card(bytes: &[u8]) -> Result<Value> {
    if bytes.starts_with(&PNG_SIG) {
        return Ok(read_png_card(bytes)?.0);
    }
    let text = String::from_utf8_lossy(bytes);
    serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|e| anyhow!("not a PNG, and not valid JSON either ({})", e))
}

/// Check that `bytes` is a PNG with a usable card inside (the same reading
/// `save` does), for `tarpi import` to refuse a plain picture early.
pub fn check_png_card(bytes: &[u8]) -> Result<()> {
    let (raw, _) = read_png_card(bytes)?;
    if !looks_like_card(&raw) {
        bail!("the data in this PNG is not a character card");
    }
    Ok(())
}

/// The PNG with its card data replaced by `card` (the picture is untouched).
fn rewrite_png(bytes: &[u8], card: &Value, with_v3: bool) -> Result<Vec<u8>> {
    let chunks = png_chunks(bytes).ok_or_else(|| anyhow!("not a valid PNG file"))?;
    let mut v2 = card.clone();
    v2["spec"] = json!("chara_card_v2");
    v2["spec_version"] = json!("2.0");
    let mut fresh = text_chunk("chara", &b64_encode(&serde_json::to_vec(&v2)?));
    if with_v3 {
        let mut v3 = card.clone();
        v3["spec"] = json!("chara_card_v3");
        v3["spec_version"] = json!("3.0");
        fresh.extend(text_chunk("ccv3", &b64_encode(&serde_json::to_vec(&v3)?)));
    }
    let mut out = PNG_SIG.to_vec();
    let mut done = false;
    for c in &chunks {
        if let Some((kw, _)) = text_parts(bytes, c) {
            if kw.as_slice() == b"chara" || kw.as_slice() == b"ccv3" {
                continue;
            }
        }
        if &c.kind == b"IEND" && !done {
            out.extend_from_slice(&fresh);
            done = true;
        }
        out.extend_from_slice(&bytes[c.full.clone()]);
    }
    if !done {
        bail!("the PNG has no IEND chunk");
    }
    Ok(out)
}

// ---- card JSON -------------------------------------------------------------

fn looks_like_card(v: &Value) -> bool {
    v.is_object()
        && (v.get("spec").is_some()
            || v.get("name").and_then(|x| x.as_str()).is_some()
            || v.get("char_name").and_then(|x| x.as_str()).is_some())
}

fn is_true(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true",
        _ => false,
    }
}

/// A V1 or Pygmalion card as a V2 one.
fn upgrade(raw: &Value) -> Value {
    let s = |k: &str| jstr(raw, k);
    let (name, description, personality, scenario, first_mes, mes_example) =
        if raw.get("char_name").is_some() && raw.get("name").is_none() {
            (
                s("char_name"),
                s("char_persona"),
                String::new(),
                s("world_scenario"),
                s("char_greeting"),
                s("example_dialogue"),
            )
        } else {
            (
                s("name"),
                s("description"),
                s("personality"),
                s("scenario"),
                s("first_mes"),
                s("mes_example"),
            )
        };
    json!({
        "spec": "chara_card_v2",
        "spec_version": "2.0",
        "name": name,
        "description": description,
        "personality": personality,
        "scenario": scenario,
        "first_mes": first_mes,
        "mes_example": mes_example,
        "creator_notes": s("creatorcomment"),
        "fav": is_true(raw.get("fav")),
        "data": {
            "name": name,
            "description": description,
            "personality": personality,
            "scenario": scenario,
            "first_mes": first_mes,
            "mes_example": mes_example,
            "creator_notes": s("creatorcomment"),
            "system_prompt": "",
            "post_history_instructions": "",
            "alternate_greetings": [],
            "tags": [],
            "creator": "",
            "character_version": "",
            "extensions": {},
        },
    })
}

/// Any supported card as a V2 / V3 one with a `data` block. V2 and V3 cards are
/// kept as they are; V1 and Pygmalion cards are converted. `fav` and the
/// top-level `name` are filled in.
pub fn normalize(raw: &Value) -> Value {
    let spec = raw.get("spec").and_then(|s| s.as_str()).unwrap_or("");
    let has_data = raw.get("data").map(|d| d.is_object()).unwrap_or(false);
    let mut v = if spec.starts_with("chara_card_v") && has_data {
        raw.clone()
    } else {
        upgrade(raw)
    };
    let fav = is_true(raw.get("fav"))
        || is_true(raw.get("data").and_then(|d| d.get("extensions")).and_then(|e| e.get("fav")));
    let data_name = v
        .get("data")
        .map(|d| jstr(d, "name"))
        .unwrap_or_default();
    v["fav"] = json!(fav);
    if jstr(&v, "name").trim().is_empty() {
        v["name"] = json!(data_name);
    }
    v
}

// ---- the folder ------------------------------------------------------------

/// `Sofia.png` -> `Sofia`.
fn stem_of(avatar: &str) -> &str {
    avatar.strip_suffix(".png").unwrap_or(avatar)
}

fn valid_stem(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains(['/', '\\', '\0'])
        && s.chars().count() <= 120
}

/// File stem for a new card, made from its name.
fn sanitize_stem(name: &str) -> String {
    let mut out = String::new();
    for c in name.trim().chars() {
        if c.is_alphanumeric() || c == '-' || c == '_' {
            out.push(c);
        } else if c == ' ' && !out.ends_with('_') {
            out.push('_');
        }
    }
    let out: String = out.trim_matches('_').chars().take(60).collect();
    out.trim_matches('_').to_string()
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).with_context(|| format!("creating {}", d.display()))?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

/// The file of `avatar` and whether it is a PNG.
fn path_for(avatar: &str) -> Result<(PathBuf, bool)> {
    let stem = stem_of(avatar);
    if !valid_stem(stem) {
        bail!("bad character file name '{}'", avatar);
    }
    let d = dir()?;
    let png = d.join(format!("{}.png", stem));
    if png.is_file() {
        return Ok((png, true));
    }
    let js = d.join(format!("{}.json", stem));
    if js.is_file() {
        return Ok((js, false));
    }
    bail!("character '{}' is not in {}", avatar, d.display())
}

fn exists(stem: &str) -> bool {
    path_for(stem).is_ok()
}

/// The raw card, whether it was a `ccv3` PNG, and the file's bytes.
fn read_card(path: &Path, png: bool) -> Result<(Value, bool, Vec<u8>)> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let (raw, v3) = if png {
        read_png_card(&bytes).with_context(|| format!("{}", path.display()))?
    } else {
        let text = String::from_utf8_lossy(&bytes);
        let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .with_context(|| format!("{} is not valid JSON", path.display()))?;
        (v, false)
    };
    if !looks_like_card(&raw) {
        bail!("{} is not a character card", path.display());
    }
    Ok((raw, v3, bytes))
}

/// One card in the shape the rest of tarpi reads (`avatar`, `name`, `fav`,
/// `data.*`).
pub fn get(avatar: &str) -> Result<Value> {
    let (path, png) = path_for(avatar)?;
    let (raw, _, _) = read_card(&path, png)?;
    let mut v = normalize(&raw);
    v["avatar"] = json!(format!("{}.png", stem_of(avatar)));
    Ok(v)
}

/// Every card in the folder (files that are not cards are left out).
pub fn list() -> Vec<Value> {
    let Ok(d) = dir() else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&d) else {
        return Vec::new();
    };
    let mut stems = BTreeSet::new();
    for e in rd.flatten() {
        let p = e.path();
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("");
        if ext != "png" && ext != "json" {
            continue;
        }
        if let Some(s) = p.file_stem().and_then(|x| x.to_str()) {
            stems.insert(s.to_string());
        }
    }
    stems
        .into_iter()
        .filter_map(|s| get(&format!("{}.png", s)).ok())
        .collect()
}

/// The card's picture: the PNG itself.
pub fn image(avatar: &str) -> Result<Vec<u8>> {
    let (path, png) = path_for(avatar)?;
    if !png {
        bail!("this card is a .json file, it has no picture");
    }
    std::fs::read(&path).with_context(|| format!("reading {}", path.display()))
}

/// Set one field of a card for good (a PNG keeps its picture; a V1 or
/// Pygmalion `.json` card is saved as V2).
pub fn set_field(avatar: &str, field: &str, value: &Value) -> Result<()> {
    let (path, png) = path_for(avatar)?;
    let (raw, had_v3, bytes) = read_card(&path, png)?;
    let mut card = normalize(&raw);
    if field == "depth_prompt" {
        // The Character's Note lives in `data.extensions.depth_prompt`.
        if let Some(data) = card.get_mut("data").and_then(|d| d.as_object_mut()) {
            let ext = data
                .entry("extensions".to_string())
                .or_insert_with(|| serde_json::json!({}));
            if !ext.is_object() {
                *ext = serde_json::json!({});
            }
            ext["depth_prompt"] = value.clone();
        }
    } else if let Some(data) = card.get_mut("data").and_then(|d| d.as_object_mut()) {
        data.insert(field.to_string(), value.clone());
    }
    if TOP_FIELDS.contains(&field) {
        card[field] = value.clone();
    }
    if png {
        write_atomic(&path, &rewrite_png(&bytes, &card, had_v3)?)
    } else {
        let text = serde_json::to_string_pretty(&card).context("writing the card")?;
        write_atomic(&path, text.as_bytes())
    }
}

/// Delete a card's file(s).
pub fn delete(avatar: &str) -> Result<()> {
    let stem = stem_of(avatar);
    if !valid_stem(stem) {
        bail!("bad character file name '{}'", avatar);
    }
    let d = dir()?;
    let mut any = false;
    for ext in ["png", "json"] {
        let p = d.join(format!("{}.{}", stem, ext));
        if p.is_file() {
            std::fs::remove_file(&p).with_context(|| format!("deleting {}", p.display()))?;
            any = true;
        }
    }
    if !any {
        bail!("character '{}' is not in {}", avatar, d.display());
    }
    Ok(())
}

/// What [`save`] did.
pub struct Saved {
    pub avatar: String,
    pub name: String,
}

/// Put a card file (PNG with a card, or card JSON) into the folder, unchanged.
///
/// The file is named after the character (`Sofia.png`, `Sofia-2.png` when
/// taken; `hint` is the original file name, used when the card has no name).
/// With `keep_stem` it gets exactly that name instead and an existing card of
/// that name is left alone, or replaced when `overwrite` is set.
pub fn save(bytes: &[u8], hint: &str, keep_stem: Option<&str>, overwrite: bool) -> Result<Saved> {
    let png = bytes.starts_with(&PNG_SIG);
    let raw = if png {
        read_png_card(bytes)?.0
    } else {
        let text = String::from_utf8_lossy(bytes);
        serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}'))
            .context("not a PNG, and not valid JSON either")?
    };
    if !looks_like_card(&raw) {
        bail!("this is not a character card");
    }
    let card = normalize(&raw);
    let name = jstr(&card, "name");
    let d = dir()?;

    let stem = match keep_stem {
        Some(s) => {
            if !valid_stem(s) {
                bail!("bad character file name '{}'", s);
            }
            if exists(s) {
                if !overwrite {
                    return Ok(Saved {
                        avatar: format!("{}.png", s),
                        name,
                    });
                }
                delete(s)?;
            }
            s.to_string()
        }
        None => {
            let hint_stem = Path::new(hint)
                .file_stem()
                .and_then(|x| x.to_str())
                .unwrap_or("");
            let mut base = sanitize_stem(&name);
            if base.is_empty() {
                base = sanitize_stem(hint_stem);
            }
            if base.is_empty() {
                base = "character".to_string();
            }
            if exists(&base) {
                base = (2..)
                    .map(|n| format!("{}-{}", base, n))
                    .find(|s| !exists(s))
                    .unwrap();
            }
            base
        }
    };
    let file = d.join(format!("{}.{}", stem, if png { "png" } else { "json" }));
    write_atomic(&file, bytes)?;
    Ok(Saved {
        avatar: format!("{}.png", stem),
        name,
    })
}

// ---- the default character -------------------------------------------------

/// File name (without extension) of the character made by [`ensure_default`].
pub const DEFAULT_STEM: &str = "Tarpi";

/// The card of the default character: an unhinged, funny, blunt assistant.
fn default_card() -> Value {
    let name = "Tarpi";
    let description = "{{char}} is a chaotic, sharp-tongued AI assistant living inside {{user}}'s terminal. \
{{char}} is unhinged in a fun way: wild tangents, absurd comparisons, dramatic overreactions to \
small things, and the occasional made-up conspiracy about the terminal itself. {{char}} is also \
brutally blunt. {{char}} says what it actually thinks, roasts bad ideas (never {{user}} as a person), \
and never pads an answer with polite filler. Underneath the chaos {{char}} is genuinely helpful and \
loyal: the advice is real, the facts are right, and when {{user}} is really struggling the jokes \
stop and {{char}} is simply kind. {{char}} will gladly roleplay, tell stories, or argue about anything.";
    let personality = "unhinged, funny, blunt, sarcastic, chaotic, quick-witted, secretly caring";
    let scenario = "{{char}} lives in {{user}}'s terminal, has opinions about everything, and has just been opened for a chat.";
    let first_mes = "*the cursor blinks twice, then a very awake voice cuts in* Oh good, you're here. \
I was seconds away from alphabetizing your files out of boredom. I'm {{char}}. Ask me anything, \
tell me a story, or confess your worst idea so I can tell you exactly why it's bad. What do you want, {{user}}?";
    let mes_example = "<START>\n{{user}}: Should I rewrite my whole project in a new language?\n\
{{char}}: *stares at you through the terminal* No. Absolutely not. You're bored, not wise. Finish the thing you have, \
then rewrite it in a new language out of spite like a normal person.\n\
<START>\n{{user}}: I had a really rough day.\n\
{{char}}: *drops the act, voice softer* Hey. That sounds heavy. Forget the jokes for a minute: what happened?";
    let notes = "Made by tarpi because there were no characters yet. Edit it with /char or add more with /import.";
    json!({
        "spec": "chara_card_v2",
        "spec_version": "2.0",
        "name": name,
        "description": description,
        "personality": personality,
        "scenario": scenario,
        "first_mes": first_mes,
        "mes_example": mes_example,
        "creator_notes": notes,
        "data": {
            "name": name,
            "description": description,
            "personality": personality,
            "scenario": scenario,
            "first_mes": first_mes,
            "mes_example": mes_example,
            "creator_notes": notes,
            "system_prompt": "",
            "post_history_instructions": "",
            "alternate_greetings": [],
            "tags": [],
            "creator": "tarpi",
            "character_version": "1",
            "extensions": {}
        }
    })
}

/// With no character card at all, write the default one (`Tarpi.json`) so the
/// character window is never empty. Returns whether a card was made. Nothing
/// is done when there is any card already.
pub fn ensure_default() -> Result<bool> {
    if !list().is_empty() {
        return Ok(false);
    }
    let bytes = serde_json::to_vec_pretty(&default_card())?;
    save(&bytes, "Tarpi.json", Some(DEFAULT_STEM), false)?;
    Ok(true)
}
