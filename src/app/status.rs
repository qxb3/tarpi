//! Small read-only views of the app state (what the title bar and
//! status row show) and the token statistics behind them.

use super::*;

impl App {
    pub fn is_generating(&self) -> bool {
        self.pending.is_some()
    }

    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// No connection is set, or the backend did not answer the last check.
    pub fn api_down(&self) -> bool {
        self.conn.is_none() || self.health.down()
    }

    /// `api@host:port`: the type and address of the API connection.
    pub fn model_label(&self) -> String {
        if self.server.is_empty() {
            self.label.clone()
        } else {
            format!("{}@{}", self.label, self.server)
        }
    }

    /// `Name (Title)` of the active persona, for the title bar.
    pub fn persona_label(&self) -> String {
        if self.persona_title.trim().is_empty() {
            self.user_name.clone()
        } else {
            format!("{} ({})", self.user_name, self.persona_title.trim())
        }
    }

    pub(super) fn set_status(&mut self, msg: impl Into<String>, is_error: bool) {
        self.status = Some((msg.into(), is_error, Instant::now()));
    }

    /// Text and "is error" flag for the bottom status line.
    /// Where the backend can tokenize: only through the standalone connection.
    pub(super) fn token_backend(&self) -> Option<tokens::Backend> {
        let c = self.conn.as_ref()?;
        // A connection that cannot count tokens keeps the estimate.
        if !c.provider().is_some_and(|p| p.tokenizes(&c.resolved())) {
            return None;
        }
        if jstr(&self.settings, "main_api") != "textgenerationwebui" {
            return None;
        }
        let api_type = textgen::api_type(&self.settings);
        let server = textgen::server_url(&self.settings);
        if api_type.is_empty() || server.is_empty() {
            return None;
        }
        let model = textgen::current_model(&self.settings);
        Some(tokens::Backend {
            api_type,
            server,
            model,
            conn: self.conn.clone(),
        })
    }

    /// Hash of everything the prompt is built from (chat and settings).
    pub(super) fn prompt_fingerprint(&self) -> u64 {
        let mut h = DefaultHasher::new();
        self.settings_ver.hash(&mut h);
        for m in &self.chat.messages {
            chat::is_user(m).hash(&mut h);
            chat::mes(m).hash(&mut h);
        }
        h.finish()
    }

    /// The complete prompt for the current chat as text: system prompt,
    /// character card, persona, examples, formatting and all messages. Not
    /// trimmed to the context window, so the count is the full total.
    pub(super) fn full_prompt_text(&self) -> String {
        // A chars-per-token this large keeps the history trimming from cutting anything.
        const NO_TRIM: f32 = textgen::NO_TRIM;
        let history = &self.chat.messages[..];
        let note = Note::from_chat(&self.chat);
        match jstr(&self.settings, "main_api").as_str() {
            "textgenerationwebui" => {
                textgen::build_prompt(&textgen::TgArgs {
                    settings: &self.settings,
                    ch: &self.character,
                    user_name: &self.user_name,
                    persona: &self.persona,
                    persona_position: self.persona_position,
                    note: &note,
                    history,
                    chars_per_token: NO_TRIM,
                    continue_text: None,
                    nudge: None,
                })
                .prompt
            }
            "openai" => {
                let null = Value::Null;
                let oai = self.settings.get("oai_settings").unwrap_or(&null);
                let pu = self.settings.get("power_user").unwrap_or(&null);
                prompt::build_chat_completion(&prompt::BuildArgs {
                    oai,
                    power_user: pu,
                    ch: &self.character,
                    user_name: &self.user_name,
                    persona: &self.persona,
                    persona_position: self.persona_position,
                    history,
                    chars_per_token: NO_TRIM,
                    continue_last: false,
                })
                .iter()
                .map(|m| m.content.as_str())
                .collect::<Vec<_>>()
                .join("\n")
            }
            _ => history
                .iter()
                .map(|m| chat::mes(m))
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// Recount the totals. The prompt is rebuilt only when the chat or the
    /// settings changed; its token count is exact once the backend has
    /// counted it, and the characters / `chars_per_token` estimate until then.
    /// Nothing is recounted while a reply is being generated (see below).
    pub(super) fn update_stats(&mut self) {
        // While a reply (or a summary pass) streams, the totals stay as they
        // were: no estimate (`~`) flickers in and no tokenizer request is
        // made. They are recounted on the first tick after the stream ends.
        if self.is_generating() {
            return;
        }
        // The reply is in the chat now: the backend's count belongs to it.
        if let Some(n) = self.reported_pending.take() {
            self.reported = Some((self.prompt_fingerprint(), n));
            self.stats_at = None;
        }
        let fp = self.prompt_fingerprint();
        if self.prompt_fp != Some(fp) {
            self.prompt_text = self.full_prompt_text();
            self.prompt_fp = Some(fp);
        } else if self.stats.exact && self.stats_at.is_some() {
            self.stats_at = Some(Instant::now());
            return;
        }
        // Exactly the chat the backend counted: its own figure, as reported.
        if let Some((rfp, n)) = self.reported {
            if rfp == fp {
                self.stats = ChatStats {
                    messages: self.chat.messages.len(),
                    tokens: n,
                    exact: true,
                };
                self.stats_at = Some(Instant::now());
                return;
            }
        }
        let backend = self.token_backend();
        let t = self.tokens.total(
            backend.as_ref(),
            &self.prompt_text,
            self.chars_per_token,
        );
        self.stats = ChatStats {
            messages: self.chat.messages.len(),
            tokens: t.tokens,
            exact: t.exact,
        };
        self.stats_at = Some(Instant::now());
        // Say why the exact count is not available (once per failure).
        if let Some(e) = self.tokens.take_error() {
            self.set_status(
                format!("token count failed, using the estimate: {}", e),
                true,
            );
        }
    }

    pub fn status_line(&self) -> (String, bool) {
        if let Some(j) = &self.summary_job {
            let n = j.plan.parts.len();
            let text = if n > 1 {
                format!("summarizing\u{2026} part {}/{} (Esc stops)", j.next + 1, n)
            } else {
                "summarizing\u{2026} (Esc stops)".to_string()
            };
            return (text, false);
        }
        if let Some((_, started)) = &self.import_job {
            return (format!("importing\u{2026} {}s", started.elapsed().as_secs()), false);
        }
        if let Some((text, is_err, when)) = &self.status {
            if when.elapsed() < Duration::from_secs(10) {
                return (text.clone(), *is_err);
            }
        }
        // A fresh chat with alternate greetings: say how to browse them.
        let greetings = match self.chat.messages.as_slice() {
            [m] if !chat::is_user(m) => chat::swipe_pos(m).filter(|&(_, t)| t > 1),
            _ => None,
        };
        if let Some((cur, total)) = greetings {
            let how = if self.touch {
                format!("tap \u{2039}{}/{}\u{203A} to switch greeting", cur + 1, total)
            } else {
                format!("Ctrl+\u{2190}/\u{2192} switch greeting ({}/{})", cur + 1, total)
            };
            return (how, false);
        }
        // A summarized chat: say which branch this is and how to switch.
        if let Some((cur, total)) = self.chat.messages.first().and_then(chat::branch_pos) {
            let how = if self.touch {
                format!("tap summary branch {}/{} to switch", cur + 1, total)
            } else {
                format!("Ctrl+\u{2190}/\u{2192} switch summary branch ({}/{})", cur + 1, total)
            };
            return (how, false);
        }
        // Nothing useful to add when idle: the box title already lists the
        // basic keys, and F1 has the rest.
        (String::new(), false)
    }
}
