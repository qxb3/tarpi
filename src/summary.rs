//! `/summarize`: cutting a chat into parts a single request can digest, and
//! writing the instructions for each request. This file is plain text work;
//! the generation itself is driven by `app/summarize.rs` (`App::summarize`).

use crate::chat;
use crate::prompt;
use serde_json::Value;

/// Tokens the instructions and the template's own sequences take in one
/// request (a generous estimate).
const OVERHEAD: usize = 700;
/// Share of the context one request may fill. The chars-per-token figure is
/// only an estimate, so some room is kept.
const FILL: f32 = 0.95;
/// Smallest chat part worth a request of its own, in tokens.
const MIN_PART: usize = 300;
/// Smallest summary aimed for, in tokens.
const MIN_TARGET: usize = 200;

/// How a chat is going to be summarized.
pub struct Plan {
    /// The text to digest, one entry per request. Everything in one entry
    /// when the chat fits into the context together with the summary.
    pub parts: Vec<String>,
    /// Size of the final summary aimed for, in tokens: a quarter of the
    /// chat (that is the -75%), but never more than a quarter of the context.
    pub target: usize,
    /// The whole chat in tokens (estimate).
    pub before: usize,
}

/// A running `/summarize`.
pub struct Job {
    pub plan: Plan,
    /// Number of messages that are being summarized.
    pub count: usize,
    /// The request being written (0-based); every earlier one is finished.
    pub next: usize,
    /// The summary written so far: it goes into the next request.
    pub so_far: String,
}

/// Estimated tokens of `chars` characters.
pub fn est_tokens(chars: usize, cpt: f32) -> usize {
    let cpt = if cpt < 1.0 { 3.2 } else { cpt };
    ((chars as f32) / cpt).ceil() as usize
}

/// `12400` -> `12.4k`
pub fn fmt_tokens(n: usize) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f32 / 1000.0)
    } else {
        n.to_string()
    }
}

/// One block of text per message, `Name: text`, with `{{user}}` / `{{char}}`
/// filled in. On the summary branch the first message is the summary, which
/// is what the story starts from.
pub fn transcript(messages: &[Value], user_name: &str, char_name: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in messages {
        if chat::is_system(m) {
            continue;
        }
        let text = prompt::subst_display(chat::mes(m).trim(), user_name, char_name);
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        let nm = chat::name(m);
        let who = if !nm.trim().is_empty() {
            nm
        } else if chat::is_user(m) {
            user_name.to_string()
        } else {
            char_name.to_string()
        };
        out.push(format!("{}: {}", who, text));
    }
    out
}

/// How many tokens a reply aimed at `target` tokens may run to: a little
/// more, so that it can end on a full sentence.
pub fn reply_cap(target: usize) -> usize {
    target + target / 3 + 64
}

/// Words that fit into `tokens` tokens.
pub fn words_for(tokens: usize) -> usize {
    tokens * 3 / 4
}

/// Size aimed for by request `pass` of `passes`: the summary grows as more
/// of the chat has been read.
pub fn pass_target(target: usize, pass: usize, passes: usize) -> usize {
    if passes <= 1 {
        return target;
    }
    (target * (pass + 1) / passes).max(MIN_TARGET.min(target))
}

/// Decide how to summarize `blocks` (see `transcript`) with a context of
/// `ctx` tokens.
pub fn plan(blocks: &[String], ctx: usize, cpt: f32) -> Result<Plan, String> {
    let cpt = if cpt < 1.0 { 3.2 } else { cpt };
    let chars: usize = blocks.iter().map(|b| b.chars().count() + 2).sum();
    let before = est_tokens(chars, cpt);
    let ctx = ctx.max(512);
    let usable = (ctx as f32 * FILL) as usize;
    let target = (before / 4).max(MIN_TARGET).min((ctx / 4).max(MIN_TARGET));
    let cap = reply_cap(target);

    // Everything in one request?
    if before + OVERHEAD + cap <= usable {
        return Ok(Plan {
            parts: vec![blocks.join("\n\n")],
            target,
            before,
        });
    }

    // Otherwise the chat is read part by part. A request holds the
    // instructions, the summary so far, one part and the room for the reply.
    let room = usable.saturating_sub(OVERHEAD + target + cap);
    if room < MIN_PART {
        return Err(format!(
            "the context size ({} tokens) is too small to summarize this chat; raise it in `/api`",
            ctx
        ));
    }
    let max_chars = ((room as f32) * cpt) as usize;
    let mut parts: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize;
    for b in blocks {
        for piece in split_text(b, max_chars) {
            let n = piece.chars().count() + 2;
            if cur_len > 0 && cur_len + n > max_chars {
                parts.push(std::mem::take(&mut cur));
                cur_len = 0;
            }
            if !cur.is_empty() {
                cur.push_str("\n\n");
            }
            cur.push_str(&piece);
            cur_len += n;
        }
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    if parts.is_empty() {
        return Err("nothing to summarize yet".to_string());
    }
    Ok(Plan {
        parts,
        target,
        before,
    })
}

/// Cut `text` into pieces of at most `max` characters, at a line break, the
/// end of a sentence or a space when there is one in the second half of the
/// piece.
pub fn split_text(text: &str, max: usize) -> Vec<String> {
    let max = max.max(1);
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return vec![text.to_string()];
    }
    let mut out: Vec<String> = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let end = (start + max).min(chars.len());
        if end == chars.len() {
            out.push(chars[start..end].iter().collect());
            break;
        }
        let lo = start + max / 2;
        let mut cut = end;
        'search: for kind in 0..3 {
            let mut i = end;
            while i > lo {
                let c = chars[i - 1];
                let hit = match kind {
                    0 => c == '\n',
                    1 => matches!(c, '.' | '!' | '?'),
                    _ => c.is_whitespace(),
                };
                if hit {
                    cut = i;
                    break 'search;
                }
                i -= 1;
            }
        }
        out.push(chars[start..cut].iter().collect());
        start = cut;
    }
    out.into_iter()
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// The text of one request: what to write, then the chat part (and the
/// summary so far, when this is not the first request).
pub fn body(
    user: &str,
    ch: &str,
    part: &str,
    so_far: &str,
    words: usize,
    pass: usize,
    passes: usize,
) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "You are the archivist of an ongoing story that is told as a chat between {user} (the user's character) and {ch}. \
Write an in-depth summary of the story that replaces the chat: from now on the summary is the ONLY memory that the readers, and the AI that continues the story, have of what happened.\n\n",
        user = user,
        ch = ch
    ));
    s.push_str(
        "Include every important and necessary detail:\n\
- The events in the order they happened, with their causes and consequences: who did what, where and when.\n\
- Every fact the story may depend on later: names, places, objects, abilities, injuries, money, promises, secrets, lies, rules, plans and decisions, and what each character feels about and wants from the others, including how that changed.\n\
- Important things that were said, in your own words (quote a line only when its exact wording matters).\n\
- Open threads: unanswered questions, goals, conflicts, and everything that was set up but not paid off yet.\n\
- In the last paragraph, the situation right now: where everyone is, what they are doing, the time, the mood, and what was said or done last, so that the story can go on from there without a break.\n\n",
    );
    s.push_str(&format!(
        "Rules:\n\
- Plain paragraphs in the past tense and third person. No headings, no lists, no markdown, no remarks to the reader, nothing about summarizing or about being an AI.\n\
- Call the characters by name ({user} and {ch}), never \"the user\" or \"I\".\n\
- Only what is in the text: invent nothing, change nothing, and leave out nothing important.\n\
- About {words} words long.\n\n",
        user = user,
        ch = ch,
        words = words
    ));
    if so_far.trim().is_empty() {
        s.push_str(&format!("=== CHAT ===\n{}\n=== END ===\n\n", part));
    } else {
        s.push_str(
            "Below are the summary of the story up to this point and then the next part of the chat. \
Merge them into ONE new summary of everything: keep all the important details of the earlier summary (make its wording shorter, not its facts) and add the new part.\n\n",
        );
        s.push_str(&format!(
            "=== SUMMARY SO FAR ===\n{}\n\n=== NEXT PART OF THE CHAT ===\n{}\n=== END ===\n\n",
            so_far.trim(),
            part
        ));
    }
    if passes > 1 && pass + 1 < passes {
        s.push_str("The story goes on after this part, so do not wrap it up or write a conclusion. ");
    }
    s.push_str("Write the summary now.");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_keeps_all_text_and_respects_the_limit() {
        let text = "One two three. Four five six.\nSeven eight nine ten eleven twelve.";
        let pieces = split_text(text, 20);
        assert!(pieces.len() > 1);
        assert!(pieces.iter().all(|p| p.chars().count() <= 20));
        let joined: String = pieces.join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
        let orig: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(joined, orig);
    }

    #[test]
    fn small_chat_is_one_request_and_a_quarter() {
        let blocks: Vec<String> = (0..40).map(|i| format!("Name: {}", "word ".repeat(40 + i))).collect();
        let p = plan(&blocks, 16384, 3.2).unwrap();
        assert_eq!(p.parts.len(), 1);
        assert!(p.target <= p.before / 4 + 1 || p.target == MIN_TARGET);
    }

    #[test]
    fn big_chat_is_read_in_parts() {
        let blocks: Vec<String> = (0..400).map(|_| format!("Name: {}", "word ".repeat(60))).collect();
        let p = plan(&blocks, 8192, 3.2).unwrap();
        assert!(p.parts.len() > 1);
        assert!(p.target <= 8192 / 4);
    }

    #[test]
    fn tiny_context_is_refused() {
        let blocks: Vec<String> = (0..400).map(|_| format!("Name: {}", "word ".repeat(60))).collect();
        assert!(plan(&blocks, 512, 3.2).is_err());
    }
}
