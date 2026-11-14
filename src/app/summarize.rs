//! `/summarize`: a summary of a long chat, written in passes and kept as a new
//! branch next to the original.

use super::*;

impl App {
    /// `/summarize`: write an in-depth summary of the whole chat. While it is
    /// written nothing is shown but the status row; when it is done it
    /// appears whole as a new summary branch of the chat (the summary is the only
    /// message), and branch 1 stays the conversation as it was. The chat is
    /// also saved whole under another name, and nothing changes if the
    /// summary is stopped or fails.
    pub(super) async fn summarize(&mut self) {
        if self.one_shot_blocks("branching (summarize)") {
            return;
        }
        if self.is_generating() || self.editing.is_some() {
            self.set_status("finish what you are doing first, then /summarize", false);
            return;
        }
        let blocks =
            summary::transcript(&self.chat.messages, &self.user_name, &self.character.name);
        if blocks.len() < 3 {
            self.set_status("nothing to summarize yet", false);
            return;
        }
        // The context size and the connection may have changed since the last reply.
        self.refresh_settings().await;
        let ctx = ji64(&self.settings, "max_context", 8192).max(512) as usize;
        let plan = match summary::plan(&blocks, ctx, self.chars_per_token) {
            Ok(p) => p,
            Err(e) => {
                self.set_status(e, true);
                return;
            }
        };
        self.summary_job = Some(summary::Job {
            plan,
            count: blocks.len(),
            next: 0,
            so_far: String::new(),
        });
        self.start_summary_pass().await;
    }

    /// Start request `job.next` of the running `/summarize`.
    pub(super) async fn start_summary_pass(&mut self) {
        let (part, so_far, target, pass, passes) = match self.summary_job.as_ref() {
            Some(j) if j.next < j.plan.parts.len() => (
                j.plan.parts[j.next].clone(),
                j.so_far.clone(),
                j.plan.target,
                j.next,
                j.plan.parts.len(),
            ),
            _ => {
                self.summary_job = None;
                return;
            }
        };
        let Some(c) = self.conn.clone() else {
            self.summary_job = None;
            self.set_status(NO_CONNECTION, true);
            return;
        };
        let pt = summary::pass_target(target, pass, passes);
        let body = summary::body(
            &self.user_name,
            &self.character.name,
            &part,
            &so_far,
            summary::words_for(pt),
            pass,
            passes,
        );
        let tp = textgen::build_summary_prompt(
            &self.settings,
            &self.user_name,
            &self.character.name,
            &body,
        );
        let c = conn::for_summary(&c, summary::reply_cap(pt));
        let direct = match conn::build_request(&c, &tp.prompt, &tp.stop) {
            Ok(r) => r,
            Err(e) => {
                self.summary_job = None;
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
            replace_last: false,
            reset: false,
            started: chat::now_iso(),
            continue_from: None,
            summary: true,
        });
        self.scroll = 0;
        self.status = None;
    }

    /// A request of `/summarize` ended: go on with the next one, or put the
    /// finished summary into the chat.
    pub(super) async fn finish_summary_pass(&mut self, p: Pending, outcome: Outcome) {
        let Some(mut job) = self.summary_job.take() else {
            return;
        };
        match outcome {
            Outcome::Done => {}
            Outcome::Stopped => {
                self.set_status("summary stopped \u{2014} the chat is unchanged", false);
                return;
            }
            Outcome::Failed(e) => {
                self.set_status(
                    format!("summary failed: {} \u{2014} the chat is unchanged", e),
                    true,
                );
                return;
            }
        }
        let text = self.clean_summary(&p.text);
        if text.is_empty() {
            self.set_status(
                "the model returned an empty summary \u{2014} the chat is unchanged",
                true,
            );
            return;
        }
        job.so_far = text;
        job.next += 1;
        if job.next < job.plan.parts.len() {
            self.summary_job = Some(job);
            self.start_summary_pass().await;
        } else {
            self.apply_summary(job, p.started).await;
        }
    }

    /// Tidy a finished summary: no name cue in front, and nothing after the
    /// last full sentence (a reply that hit its length limit stops mid-sentence).
    pub(super) fn clean_summary(&self, text: &str) -> String {
        let mut out = chat::tidy(text);
        let cue = format!("{}:", self.character.name);
        if let Some(rest) = out.strip_prefix(cue.as_str()) {
            out = rest.trim_start().to_string();
        }
        if let Some(rest) = out.strip_prefix("SUMMARY:") {
            out = rest.trim_start().to_string();
        }
        let out = textgen::trim_to_end_sentence(&out);
        let (out, _) = prose::close_final(&out);
        out
    }

    /// Put the finished summary into the chat as its summary branch (see `summarize`).
    pub(super) async fn apply_summary(&mut self, job: summary::Job, started: String) {
        let summary::Job {
            plan,
            count,
            so_far,
            ..
        } = job;
        let before = plan.before;
        let after = summary::est_tokens(so_far.chars().count(), self.chars_per_token);
        if after >= before {
            self.set_status(
                "the summary is not shorter than the chat \u{2014} the chat is unchanged",
                true,
            );
            return;
        }
        // Keep the whole chat under another name first: a summary can lose
        // details, the story must not be lost with them.
        let backup = match self.backup_chat().await {
            Ok(name) => name,
            Err(e) => {
                self.set_status(
                    format!("could not keep a copy of the chat, so nothing was changed: {:#}", e),
                    true,
                );
                return;
            }
        };
        let finished = chat::now_iso();
        self.chat.messages = chat::with_summary_branch(
            &self.chat.messages,
            &self.character.name,
            &so_far,
            &self.model,
            &started,
            &finished,
        );
        self.auto_closed = None;
        self.scroll = 0;
        self.reveal = Some(0);
        self.save().await;
        let cut = 100usize.saturating_sub(after * 100 / before.max(1));
        let branch_no = self
            .chat
            .messages
            .first()
            .and_then(chat::branch_pos)
            .map(|(a, _)| a + 1)
            .unwrap_or(2);
        self.set_status(
            format!(
                "summarized {} messages: ~{} \u{2192} ~{} tokens (-{}%). Added as summary branch {}/{}; branch 1 is the whole conversation, and a copy is saved as {}",
                count,
                summary::fmt_tokens(before),
                summary::fmt_tokens(after),
                cut,
                branch_no,
                branch_no,
                backup
            ),
            false,
        );
    }

    /// Save the chat as it is now under a new name (`<chat>_before-summary`,
    /// then `...2`, `...3`), and return that name.
    pub(super) async fn backup_chat(&self) -> anyhow::Result<String> {
        let existing: Vec<String> = chatstore::list(&self.chat.avatar)?
            .iter()
            .map(chat::file_id)
            .collect();
        let base = format!("{}_before-summary", self.chat.file);
        let mut name = base.clone();
        let mut n = 2;
        while existing.iter().any(|e| e.eq_ignore_ascii_case(&name)) {
            name = format!("{}{}", base, n);
            n += 1;
        }
        chatstore::save(&self.chat.avatar, &name, &self.chat.copy_lines())?;
        Ok(name)
    }
}
