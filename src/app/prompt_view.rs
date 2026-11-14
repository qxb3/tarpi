//! The `/prompt` window: the exact prompt of the next reply.

use super::*;

impl App {
    /// `/prompt`: show the prompt the next reply would be sent with, on two
    /// pages: the pre-prompt (system prompt, character card, persona, examples,
    /// instruct formatting; no messages) and the post-prompt (the latest
    /// `textgen::POST_MESSAGES` messages that fit into the context size, and
    /// the cue).
    pub(super) async fn show_prompt(&mut self) {
        // The same fresh settings, persona and system prompt a reply would use.
        self.refresh_settings().await;
        let note = Note::from_chat(&self.chat);
        let tp = textgen::build_prompt(&textgen::TgArgs {
            settings: &self.settings,
            ch: &self.character,
            user_name: &self.user_name,
            persona: &self.persona,
            persona_position: self.persona_position,
            note: &note,
            history: &self.chat.messages[..],
            chars_per_token: self.chars_per_token,
            continue_text: None,
            nudge: None,
        });
        let chars = tp.prompt.chars().count();
        let tokens = prompt::est_tokens(&tp.prompt, self.chars_per_token);
        let tidy = |t: &str| -> String {
            let mut text = String::new();
            for c in t.chars() {
                match c {
                    '\n' => text.push('\n'),
                    '\t' => text.push_str("    "),
                    '\r' => {}
                    c if c.is_control() => text.push(' '),
                    c => text.push(c),
                }
            }
            text
        };
        let split = if tp.prompt.is_char_boundary(tp.split) {
            tp.split
        } else {
            0
        };
        let view = if tp.prompt.is_char_boundary(tp.view) {
            tp.view.max(split)
        } else {
            split
        };
        // Page 1: no messages. Page 2: the latest ones and the cue; older
        // messages that are in the request are left out of both pages.
        let pre = &tp.prompt[..split];
        let post = &tp.prompt[view..];
        let mut post_text = tidy(post);
        if tp.hidden > 0 {
            post_text = format!(
                "[{} earlier message{} in the prompt, not shown here]\n{}",
                tp.hidden,
                if tp.hidden == 1 { "" } else { "s" },
                post_text
            );
        }
        self.prompt_view = Some(PromptView {
            texts: [tidy(pre), post_text],
            tab: 0,
            title: format!(
                " \u{b7} {} characters \u{b7} ~{} tokens ",
                chars,
                summary::fmt_tokens(tokens)
            ),
            scroll: 0,
            page: 10,
            wrapped: Vec::new(),
            wrapped_w: 0,
        });
    }

    /// Keys in the `/prompt` window: scroll, Esc / q / Enter close.
    pub(super) fn prompt_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let mut close = false;
        if let Some(v) = self.prompt_view.as_mut() {
            let page = v.page.max(1);
            let half = (page / 2).max(1);
            match k.code {
                KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter if !ctrl => close = true,
                KeyCode::Char('c') if ctrl => close = true,
                KeyCode::Up | KeyCode::Char('k') if !ctrl => v.scroll = v.scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') if !ctrl => {
                    v.scroll = v.scroll.saturating_add(1)
                }
                KeyCode::PageUp => v.scroll = v.scroll.saturating_sub(page),
                KeyCode::PageDown | KeyCode::Char(' ') => v.scroll = v.scroll.saturating_add(page),
                KeyCode::Char('u') if ctrl => v.scroll = v.scroll.saturating_sub(half),
                KeyCode::Char('d') if ctrl => v.scroll = v.scroll.saturating_add(half),
                KeyCode::Home | KeyCode::Char('g') => v.scroll = 0,
                KeyCode::End | KeyCode::Char('G') => v.scroll = usize::MAX,
                // The two pages: pre-prompt and post-prompt.
                KeyCode::Tab | KeyCode::Right | KeyCode::Char('l') if !ctrl => v.set_tab(1),
                KeyCode::BackTab | KeyCode::Left | KeyCode::Char('h') if !ctrl => v.set_tab(0),
                KeyCode::Char('1') if !ctrl => v.set_tab(0),
                KeyCode::Char('2') if !ctrl => v.set_tab(1),
                _ => {}
            }
        }
        if close {
            self.prompt_view = None;
        }
    }
}
