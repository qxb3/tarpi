//! Writing a reply: building the request, streaming it in, and finishing,
//! stopping or continuing it.

use super::*;

impl App {
    /// The same prompt the Text Completion path builds, sent straight to the
    /// standalone connection with its own sampler settings.
    pub(super) fn build_direct_request(
        &self,
        c: &conn::Connection,
        history_end: usize,
        continue_last: bool,
        nudge: Option<&str>,
    ) -> anyhow::Result<conn::DirectRequest> {
        let history = &self.chat.messages[..history_end];
        let (history, continue_text) = match history.split_last() {
            Some((last, rest)) if continue_last => (rest, Some(chat::mes(last))),
            _ => (history, None),
        };
        let note = Note::from_chat(&self.chat);
        let tp = textgen::build_prompt(&textgen::TgArgs {
            settings: &self.settings,
            ch: &self.character,
            user_name: &self.user_name,
            persona: &self.persona,
            persona_position: self.persona_position,
            note: &note,
            history,
            chars_per_token: self.chars_per_token,
            continue_text,
            nudge,
        });
        conn::build_request(c, &tp.prompt, &tp.stop)
    }

    /// Clean up a finished reply the way the web UI does.
    pub(super) fn clean_reply(&self, text: &str) -> String {
        let mut out = chat::tidy(text);
        if jstr(&self.settings, "main_api") == "textgenerationwebui" {
            let name_prefix = format!("{}:", self.character.name);
            if let Some(rest) = out.strip_prefix(name_prefix.as_str()) {
                out = rest.trim_start().to_string();
            }
            if self.trims_sentences() {
                out = textgen::trim_to_end_sentence(&out);
            }
        }
        out
    }

    pub(super) fn trims_sentences(&self) -> bool {
        let null = Value::Null;
        let pu = self.settings.get("power_user").unwrap_or(&null);
        pu.get("context")
            .map(|c| jbool(c, "trim_sentences", false))
            .unwrap_or(false)
            || jbool(pu, "trim_sentences", false)
    }

    /// The old text plus the new chunk, or `None` when nothing was added.
    pub(super) fn clean_continuation(&self, orig: &str, chunk: &str) -> Option<String> {
        if chunk.trim().is_empty() {
            return None;
        }
        let mut out = chat::join_continuation(orig, chunk);
        if jstr(&self.settings, "main_api") == "textgenerationwebui" && self.trims_sentences() {
            out = textgen::trim_to_end_sentence(&out);
        }
        Some(out)
    }

    pub(super) async fn start_generation(&mut self, replace_last: bool) {
        self.start_generation_mode(replace_last, false, false).await;
    }

    pub(super) async fn start_generation_mode(&mut self, replace_last: bool, continue_last: bool, reset: bool) {
        // A nudge belongs to this one call, whether or not it gets to run.
        let nudge = self.next_nudge.take();
        if self.is_generating() {
            return;
        }
        if self.editing.is_some() {
            self.set_status(
                "finish editing first (Enter save \u{b7} Esc cancel)",
                false,
            );
            return;
        }
        self.refresh_settings().await;
        let history_end = if replace_last {
            self.chat.messages.len().saturating_sub(1)
        } else {
            self.chat.messages.len()
        };

        let continue_from = if continue_last {
            self.chat.messages.last().map(|m| chat::mes(m).to_string())
        } else {
            None
        };
        // Replies come only from the standalone connection (`/api`).
        let Some(c) = self.conn.clone() else {
            self.set_status(NO_CONNECTION, true);
            return;
        };
        let direct = match self.build_direct_request(&c, history_end, continue_last, nudge.as_deref()) {
            Ok(r) => r,
            Err(e) => {
                self.set_status(format!("{:#}", e), true);
                return;
            }
        };

        self.gen_id += 1;
        let id = self.gen_id;
        let tx = self.gen_tx.clone();
        self.gen_handle = Some(tokio::spawn(conn::run(direct, id, tx)));
        self.pending = Some(Pending {
            text: String::new(),
            replace_last,
            reset,
            started: chat::now_iso(),
            continue_from,
            summary: false,
        });
        self.scroll = 0;
        self.status = None;
    }

    /// Ctrl+K, `/continue` or `/c`: let the model keep writing the last reply.
    pub(super) async fn continue_message(&mut self) {
        if self.one_shot_blocks("continue") {
            return;
        }
        if self.is_generating() {
            return;
        }
        let ok = self
            .chat
            .messages
            .last()
            .map(|m| !chat::is_user(m) && !chat::is_system(m))
            .unwrap_or(false);
        if !ok {
            self.set_status(
                "nothing to continue \u{2014} the last message isn't a reply",
                false,
            );
            return;
        }
        // Take off the markers the TUI added when the reply finished, so the
        // continuation can carry on inside them.
        if let Some((idx, closer)) = self.auto_closed.take() {
            let last = self.chat.messages.len() - 1;
            if idx == last {
                if let Some(m) = self.chat.messages.get_mut(last) {
                    let text = chat::mes(m).to_string();
                    if let Some(stripped) = text.strip_suffix(closer.as_str()) {
                        let stripped = stripped.to_string();
                        chat::set_text(m, &stripped);
                    }
                }
            }
        }
        self.start_generation_mode(false, true, false).await;
    }

    pub async fn on_gen_event(&mut self, msg: GenMsg) {
        let (id, ev) = msg;
        if id != self.gen_id || self.pending.is_none() {
            return; // stale event from a stopped generation
        }
        match ev {
            GenEvent::Chunk(t) => {
                if let Some(p) = self.pending.as_mut() {
                    p.text.push_str(&t);
                }
            }
            GenEvent::Usage(u) => {
                // The backend says how long the prompt was and how long the
                // reply is: that is the chat's size, as the backend counts it.
                // (A summary pass is about other text.)
                if self.pending.as_ref().is_some_and(|p| !p.summary) {
                    self.reported_pending = Some(u.prompt_tokens + u.completion_tokens);
                }
            }
            GenEvent::Done => self.finish_generation(Outcome::Done).await,
            GenEvent::Error(e) => self.finish_generation(Outcome::Failed(e)).await,
        }
    }

    pub(super) async fn cancel_generation(&mut self) {
        if let Some(h) = self.gen_handle.take() {
            h.abort();
        }
        // Any events still in flight belong to the old id and will be ignored.
        self.gen_id += 1;
        self.finish_generation(Outcome::Stopped).await;
    }

    pub(super) async fn finish_generation(&mut self, outcome: Outcome) {
        let Some(p) = self.pending.take() else {
            return;
        };
        self.gen_handle = None;

        if p.summary {
            self.finish_summary_pass(p, outcome).await;
            return;
        }

        if let Some(orig) = p.continue_from.as_deref() {
            match self.clean_continuation(orig, &p.text) {
                None => match outcome {
                    Outcome::Done => self.set_status("the model had nothing to add", true),
                    Outcome::Stopped => self.set_status("stopped", false),
                    Outcome::Failed(e) => {
                        self.set_status(format!("generation failed: {}", e), true)
                    }
                },
                Some(new_text) => {
                    let finished = chat::now_iso();
                    let (new_text, closer) = prose::close_final(&new_text);
                    if !closer.is_empty() {
                        self.auto_closed = Some((self.chat.messages.len().saturating_sub(1), closer));
                    }
                    if let Some(m) = self.chat.messages.last_mut() {
                        chat::set_text(m, &new_text);
                        chat::touch_finished(m, &finished);
                    }
                    match outcome {
                        Outcome::Done => {}
                        Outcome::Stopped => {
                            self.set_status("stopped (partial continuation kept)", false)
                        }
                        Outcome::Failed(e) => self.set_status(
                            format!("interrupted: {} (partial continuation kept)", e),
                            true,
                        ),
                    }
                    self.save().await;
                }
            }
            return;
        }

        let text = self.clean_reply(&p.text);
        // Close markers the model left open (`*narration`, `"speech`).
        let (text, closer) = prose::close_final(&text);

        if text.is_empty() {
            match outcome {
                Outcome::Done => self.set_status("the model returned an empty reply", true),
                Outcome::Stopped => self.set_status("stopped", false),
                Outcome::Failed(e) => self.set_status(format!("generation failed: {}", e), true),
            }
            return;
        }

        let finished = chat::now_iso();
        let last_is_assistant = self
            .chat
            .messages
            .last()
            .map(|m| !chat::is_user(m))
            .unwrap_or(false);
        if p.replace_last && last_is_assistant {
            if let Some(m) = self.chat.messages.last_mut() {
                if p.reset {
                    chat::reset_swipes(m, &text, &self.model, &p.started, &finished);
                } else {
                    chat::add_swipe(m, &text, &self.model, &p.started, &finished);
                }
            }
        } else {
            self.chat.messages.push(chat::assistant_message(
                &self.character.name,
                &text,
                &self.model,
                &p.started,
                &finished,
            ));
        }

        self.auto_closed = if closer.is_empty() {
            None
        } else {
            Some((self.chat.messages.len().saturating_sub(1), closer))
        };

        match outcome {
            Outcome::Done => {}
            Outcome::Stopped => self.set_status("stopped (partial reply kept)", false),
            Outcome::Failed(e) => {
                self.set_status(format!("interrupted: {} (partial reply kept)", e), true)
            }
        }
        self.save().await;
    }
}
