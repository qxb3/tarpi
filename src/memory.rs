//! Vector memory, the minimal core: remember what was said earlier in a chat
//! and bring back the few older messages that are closest in meaning to what
//! was said last.
//!
//! The three parts of a vector memory, kept as small as they can be:
//!
//! 1. **embed**: text becomes a fixed-size vector. Here that is a hashed
//!    bag of words (each word adds to one of `DIM` slots, then the vector is
//!    scaled to length 1). It needs no model, no network and no extra crate.
//! 2. **store**: the chat file *is* the store. The vectors are cheap to make,
//!    so they are made again for every reply and nothing is saved.
//! 3. **retrieve**: cosine similarity between the query vector and every
//!    stored vector; the best `top_k` above `MIN_SCORE` come back, oldest
//!    first so they read in story order.
//!
//! Only messages that are *not* among the latest `skip_recent` are searched:
//! those are normally still in the prompt, so repeating them would waste
//! tokens. `textgen::build_prompt` injects what comes back (see
//! the `/memory` window, saved in `vector-memory.yaml`).
//!
//! This file has no dependencies besides `std`.

/// Size of a vector. Bigger means fewer words sharing a slot.
pub const DIM: usize = 256;
/// A memory longer than this many characters is cut when injected.
pub const MAX_CHARS: usize = 600;

/// Common words that say nothing about what a message is about.
const STOP: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "but", "by", "for", "from", "had", "has", "have",
    "he", "her", "his", "i", "in", "is", "it", "its", "me", "my", "of", "on", "or", "she", "so",
    "that", "the", "their", "them", "then", "they", "this", "to", "was", "we", "were", "what",
    "with", "you", "your",
];

/// Lowercase words of `text` (letters and digits, apostrophes kept inside a
/// word), without the stop words.
fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let flush = |cur: &mut String, out: &mut Vec<String>| {
        let w = cur.trim_matches('\'').to_string();
        cur.clear();
        // `dragon's` is about the dragon.
        let w = w.strip_suffix("'s").map(str::to_string).unwrap_or(w);
        if !w.is_empty() && !STOP.contains(&w.as_str()) {
            out.push(w);
        }
    };
    for c in text.chars() {
        // Models and phones write the curly apostrophe.
        let c = if c == '\u{2019}' || c == '\u{2018}' { '\'' } else { c };
        if c.is_alphanumeric() || c == '\'' {
            cur.extend(c.to_lowercase());
        } else {
            flush(&mut cur, &mut out);
        }
    }
    flush(&mut cur, &mut out);
    out
}

/// FNV-1a, so the same word lands in the same slot on every machine.
fn hash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Text to a vector of length 1 (all zeros when the text has no usable word).
pub fn embed(text: &str) -> Vec<f32> {
    let mut v = vec![0.0f32; DIM];
    for w in words(text) {
        let h = hash(&w);
        // One bit of the hash picks the sign, so two unrelated words that
        // share a slot tend to cancel instead of adding up.
        let sign = if (h >> 63) & 1 == 0 { 1.0 } else { -1.0 };
        v[(h % DIM as u64) as usize] += sign;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
    v
}

/// Cosine similarity of two vectors that [`embed`] made (so a dot product).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The `top_k` messages closest to `query`, as indexes into `messages`,
/// oldest first. The latest `skip_recent` messages are not searched, and a
/// message scoring below `min_score` is left out.
pub fn recall(
    messages: &[String],
    query: &str,
    top_k: usize,
    skip_recent: usize,
    min_score: f32,
) -> Vec<usize> {
    let searched = messages.len().saturating_sub(skip_recent);
    if top_k == 0 || searched == 0 {
        return Vec::new();
    }
    let q = embed(query);
    let mut scored: Vec<(usize, f32)> = messages[..searched]
        .iter()
        .enumerate()
        .map(|(i, m)| (i, cosine(&q, &embed(m))))
        .filter(|(_, s)| *s >= min_score && *s > 0.0)
        .collect();
    // Best first; a tie goes to the newer message.
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.0.cmp(&a.0))
    });
    scored.truncate(top_k);
    let mut idx: Vec<usize> = scored.into_iter().map(|(i, _)| i).collect();
    idx.sort_unstable();
    idx
}

/// What a memory block looks like until the user writes their own: `{{result}}`
/// stands for the recalled messages, one per line.
pub const DEFAULT_TEMPLATE: &str = "[Relevant memories from earlier in this chat:\n{{result}}]";

/// The block that goes into the prompt, or `None` when nothing was recalled.
/// `lines` are `Name: text` per memory; `template` is the user's text with
/// `{{result}}` where they go (a template without it gets them on a new line
/// at its end, an empty one is the default).
pub fn render(template: &str, lines: &[String]) -> Option<String> {
    if lines.is_empty() {
        return None;
    }
    let result = lines
        .iter()
        .map(|l| l.chars().take(MAX_CHARS).collect::<String>().trim().to_string())
        .collect::<Vec<_>>()
        .join("\n");
    let template = if template.trim().is_empty() { DEFAULT_TEMPLATE } else { template };
    Some(if template.contains("{{result}}") {
        template.replace("{{result}}", &result)
    } else {
        format!("{}\n{}", template.trim_end(), result)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN_SCORE: f32 = 0.15;

    fn msgs(m: &[&str]) -> Vec<String> {
        m.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_vector_has_length_one() {
        let v = embed("The red dragon sleeps under the mountain.");
        assert_eq!(v.len(), DIM);
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn text_without_words_is_the_zero_vector() {
        assert!(embed("the a of ...").iter().all(|x| *x == 0.0));
        assert!(embed("").iter().all(|x| *x == 0.0));
    }

    #[test]
    fn same_words_in_any_case_and_order_match() {
        let a = embed("Dragon MOUNTAIN, sleeping");
        let b = embed("sleeping mountain dragon");
        assert!((cosine(&a, &b) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn related_text_scores_higher_than_unrelated() {
        let q = embed("where did the dragon sleep");
        let near = cosine(&q, &embed("The dragon sleeps in a cave under the mountain."));
        let far = cosine(&q, &embed("We ate pasta with tomatoes for dinner."));
        assert!(near > far, "{} vs {}", near, far);
    }

    #[test]
    fn the_best_match_comes_back() {
        let m = msgs(&[
            "I love baking bread on Sundays.",
            "My sister Elena lives in Lisbon and teaches piano.",
            "It rained all week.",
            "recent one",
            "recent two",
        ]);
        let got = recall(&m, "Where does Elena live?", 1, 2, MIN_SCORE);
        assert_eq!(got, vec![1]);
    }

    #[test]
    fn recent_messages_are_not_searched() {
        let m = msgs(&["Elena lives in Lisbon.", "filler", "Elena lives in Lisbon again."]);
        assert_eq!(recall(&m, "Elena Lisbon", 5, 1, MIN_SCORE), vec![0]);
        assert!(recall(&m, "Elena Lisbon", 5, 3, MIN_SCORE).is_empty());
    }

    #[test]
    fn nothing_close_nothing_recalled() {
        let m = msgs(&["We ate pasta with tomatoes.", "x", "y"]);
        assert!(recall(&m, "dragon mountain treasure", 3, 1, MIN_SCORE).is_empty());
    }

    #[test]
    fn results_come_out_in_story_order_and_top_k_caps_them() {
        let m = msgs(&[
            "The dragon guards gold.",
            "The dragon sleeps in the mountain.",
            "The dragon hates thunder.",
            "tail",
        ]);
        let got = recall(&m, "dragon", 2, 1, MIN_SCORE);
        assert_eq!(got.len(), 2);
        assert!(got[0] < got[1]);
    }

    #[test]
    fn a_higher_cutoff_leaves_weak_matches_out() {
        let m = msgs(&["Elena lives in Lisbon and teaches piano to children.", "x", "y"]);
        assert_eq!(recall(&m, "Elena", 3, 1, 0.15), vec![0]);
        assert!(recall(&m, "Elena", 3, 1, 0.95).is_empty());
    }

    #[test]
    fn render_fills_the_template_and_cuts_long_memories() {
        assert_eq!(render(DEFAULT_TEMPLATE, &[]), None);
        let one = vec!["Bob: hi".to_string()];
        assert_eq!(
            render(DEFAULT_TEMPLATE, &one).unwrap(),
            "[Relevant memories from earlier in this chat:\nBob: hi]"
        );
        let two = vec!["Bob: hi".to_string(), "Sofia: hello".to_string()];
        let r = render("MEMORY START\n\n{{result}}\n\nMEMORY END", &two).unwrap();
        assert_eq!(r, "MEMORY START\n\nBob: hi\nSofia: hello\n\nMEMORY END");
        let long = "x".repeat(MAX_CHARS + 50);
        let r = render("{{result}}", &[long]).unwrap();
        assert_eq!(r.len(), MAX_CHARS);
    }

    #[test]
    fn a_template_without_result_still_gets_the_memories() {
        let one = vec!["Bob: hi".to_string()];
        assert_eq!(render("Remember:", &one).unwrap(), "Remember:\nBob: hi");
        assert_eq!(render("  ", &one).unwrap(), render(DEFAULT_TEMPLATE, &one).unwrap());
    }

    #[test]
    fn curly_apostrophes_and_possessives_are_the_same_word() {
        let a = embed("the dragon\u{2019}s cave");
        let b = embed("dragon cave");
        assert!((cosine(&a, &b) - 1.0).abs() < 1e-5);
        let c = embed("Elena's piano");
        let d = embed("Elena\u{2019}s piano");
        assert!((cosine(&c, &d) - 1.0).abs() < 1e-5);
    }
}
