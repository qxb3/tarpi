//! Operations on the chat as a whole: the `/one-shot` lock, undo, new and
//! deleted chats, regenerate, branches and swipes.

use super::*;

impl App {
    /// Is this chat `/one-shot`? Then say that `what` is off and return true.
    pub(super) fn one_shot_blocks(&mut self, what: &str) -> bool {
        if self.chat.is_one_shot() {
            self.set_status(format!("this chat is one-shot: {} is turned off", what), false);
            true
        } else {
            false
        }
    }

    /// In an `/one-shot` chat a reply that is being written can't be stopped
    /// (not with Esc, the Stop button, or by leaving the chat).
    pub(super) fn stop_blocked(&mut self) -> bool {
        if self.is_generating() && self.chat.is_one_shot() {
            self.set_status(
                "one-shot chat: a reply that is being written can't be stopped \u{2014} wait for it to finish",
                false,
            );
            true
        } else {
            false
        }
    }

    /// `/one-shot`: lock this chat file as it is (no editing, undo, regenerate,
    /// swipes or branches). Asks for the command a second time to confirm.
    pub(super) async fn one_shot(&mut self) {
        if self.chat.is_one_shot() {
            self.set_status("this chat is already one-shot", false);
            return;
        }
        if self.is_generating() || self.editing.is_some() {
            self.set_status("finish what you are doing first, then /one-shot", false);
            return;
        }
        if !self.one_shot_armed {
            self.one_shot_armed = true;
            self.set_status(
                "this chat will lose editing, undo, regen, swipes and branches for good \u{2014} type /one-shot again to confirm",
                true,
            );
            return;
        }
        self.one_shot_armed = false;
        self.chat.set_one_shot();
        self.save().await;
        self.set_status("this chat is now one-shot", false);
    }

    /// `/undo [n]`: delete the last `n` messages (default 1). A branched chat
    /// always keeps its first message; otherwise `n` is capped at what exists.
    pub(super) async fn undo(&mut self, count: usize) {
        if self.one_shot_blocks("undo") {
            return;
        }
        if self.editing.is_some() {
            return;
        }
        if self.is_generating() {
            self.set_status("stop the generation first (Esc)", false);
            return;
        }
        let branched =
            !self.chat.messages.is_empty() && chat::branch_pos(&self.chat.messages[0]).is_some();
        let available = self.chat.messages.len().saturating_sub(usize::from(branched));
        if available == 0 {
            if branched {
                self.set_status(
                    "this is the first message of a branched chat \u{2014} it can't be undone (switch branch instead)",
                    false,
                );
            }
            return;
        }
        let n = count.min(available);
        let keep = self.chat.messages.len() - n;
        self.chat.messages.truncate(keep);
        self.auto_closed = None;
        self.scroll = 0;
        self.save().await;
        let msg = if n == 1 {
            if count > 1 {
                "deleted the last message (that was all there was to undo)".to_string()
            } else {
                "deleted the last message".to_string()
            }
        } else if n < count {
            format!("deleted the last {} messages (that was all there was to undo)", n)
        } else {
            format!("deleted the last {} messages", n)
        };
        self.set_status(msg, false);
    }

    pub(super) async fn new_chat(&mut self) {
        if self.is_generating() {
            self.set_status("stop the generation first (Esc)", false);
            return;
        }
        let ids: Vec<String> = match chatstore::list(&self.chat.avatar) {
            Ok(list) => list.iter().map(chat::file_id).collect(),
            Err(_) => vec![self.chat.file.clone()],
        };
        let file = chat::next_chat_name(&self.character.name, &ids);
        let mut state = ChatState::new_empty(&self.chat.avatar, &self.chat.char_name, &file);
        if let Some(g) = make_greeting(&self.character, &self.user_name, &self.persona) {
            state.messages.push(g);
        }
        self.chat = state;
        self.auto_closed = None;
        self.scroll = 0;
        self.save().await;
        self.set_status(format!("started new chat: {}", self.chat.file), false);
    }

    /// `/newchat` (`/nc`): deletes the chat file right away and
    /// starts a fresh chat.
    pub(super) async fn delete_chat(&mut self) {
        if self.is_generating() {
            self.set_status("stop the generation first (Esc)", false);
            return;
        }
        let file = self.chat.file.clone();
        match chatstore::delete(&self.chat.avatar, &file) {
            Ok(()) => {
                self.new_chat().await;
                let now = self.chat.file.clone();
                self.set_status(format!("deleted {}; started new chat {}", file, now), false);
            }
            Err(e) => self.set_status(format!("{:#}", e), true),
        }
    }

    pub(super) async fn regenerate(&mut self, nudge: Option<String>) {
        if self.one_shot_blocks("regenerate") {
            return;
        }
        if self.is_generating() || self.editing.is_some() {
            return;
        }
        let Some(last) = self.chat.messages.last() else {
            return;
        };
        if chat::is_user(last) {
            self.next_nudge = nudge;
            self.start_generation(false).await;
        } else if self.chat.messages.len() < 2 {
            self.set_status(
                "nothing to regenerate yet \u{2014} send a message first (Ctrl+\u{2192} cycles greetings)",
                false,
            );
        } else {
            self.next_nudge = nudge;
            self.start_generation_mode(true, false, true).await;
        }
    }

    /// `/deletebranch` (`/db`): delete the branch that is showing and show
    /// the one before it.
    pub(super) async fn delete_branch(&mut self) {
        if self.one_shot_blocks("branching") {
            return;
        }
        if self.is_generating() || self.editing.is_some() {
            self.set_status("finish what you are doing first, then /deletebranch", false);
            return;
        }
        let Some((cur, total)) = self.chat.messages.first().and_then(chat::branch_pos) else {
            self.set_status("there is no branch to delete: this chat has no summary branches", false);
            return;
        };
        let was = cur + 1;
        let Some((now, left)) = chat::delete_branch(&mut self.chat.messages) else {
            return;
        };
        self.auto_closed = None;
        self.scroll = 0;
        self.reveal = Some(0);
        let msg = if left >= 2 {
            format!(
                "deleted branch {} of {}; now on summary branch {}/{}",
                was,
                total,
                now + 1,
                left
            )
        } else {
            format!("deleted branch {} of {}; the chat has no more branches", was, total)
        };
        self.save().await;
        // The full-chat copy `/summarize` kept goes with the branch: the newest
        // one, or all that are left when no branch remains.
        let mut gone: Vec<String> = Vec::new();
        if let Ok(chats) = chatstore::list(&self.chat.avatar) {
            let ids: Vec<String> = chats.iter().map(chat::file_id).collect();
            let mut backups = chat::backups_of(&self.chat.file, &ids);
            if left >= 2 {
                backups = backups.into_iter().last().into_iter().collect();
            }
            for name in backups {
                if chatstore::delete(&self.chat.avatar, &name).is_ok() {
                    gone.push(name);
                }
            }
        }
        let msg = if gone.is_empty() {
            msg
        } else {
            format!("{}; deleted {}", msg, gone.join(", "))
        };
        self.set_status(msg, false);
    }

    /// Show the previous (`dir` < 0) or next summary branch of a summarized
    /// chat, wrapping around: branch 1 is the whole conversation, the others
    /// are summaries. Each keeps its own messages.
    pub(super) async fn switch_branch(&mut self, dir: i32) {
        if self.one_shot_blocks("branching") {
            return;
        }
        if self.is_generating() || self.editing.is_some() {
            return;
        }
        if !chat::switch_branch(&mut self.chat.messages, dir) {
            self.set_status("no summary branches yet: /summarize makes one", false);
            return;
        }
        self.auto_closed = None;
        self.scroll = 0;
        self.reveal = Some(0);
        if let Some((cur, total)) = self.chat.messages.first().and_then(chat::branch_pos) {
            let what = if cur == 0 { "the conversation" } else { "a summary" };
            self.set_status(format!("summary branch {}/{}: {}", cur + 1, total, what), false);
        }
        self.save().await;
    }

    /// `/swipe [nudge]`: write a new swipe of the last reply, keeping the
    /// ones it already has. Left / Right past the last swipe does the same
    /// without a nudge.
    pub(super) async fn swipe_command(&mut self, nudge: Option<String>) {
        if self.one_shot_blocks("swipes") {
            return;
        }
        if self.is_generating() || self.editing.is_some() {
            return;
        }
        self.auto_closed = None;
        let Some(last) = self.chat.messages.last() else {
            return;
        };
        if self.chat.messages.len() < 2 || chat::is_user(last) {
            self.set_status(
                "nothing to swipe yet \u{2014} send a message first (Ctrl+\u{2192} cycles greetings)",
                false,
            );
            return;
        }
        self.next_nudge = nudge;
        self.start_generation(true).await;
    }

    /// Left / Right (empty input box): browse swipes of the last reply (or, while the greeting is the only message, the alternate greetings); going past the
    /// last one generates a new swipe (like the web UI).
    pub(super) async fn swipe(&mut self, dir: i32) {
        if self.one_shot_blocks("swipes") {
            return;
        }
        if self.is_generating() || self.editing.is_some() {
            return;
        }
        self.auto_closed = None;
        let len = self.chat.messages.len();
        let Some(m) = self.chat.messages.last_mut() else {
            return;
        };
        if chat::is_user(m) {
            return;
        }
        let (cur, total) = chat::swipe_pos(m).unwrap_or((0, 1));
        if len == 1 && total > 1 {
            // Only the greeting so far: browse the alternate greetings,
            // wrapping around at both ends.
            let next = if dir < 0 {
                (cur + total - 1) % total
            } else {
                (cur + 1) % total
            };
            chat::set_swipe(m, next);
            self.set_status(format!("greeting {}/{}", next + 1, total), false);
            self.save().await;
            return;
        }
        if dir < 0 {
            if cur > 0 {
                chat::set_swipe(m, cur - 1);
                self.save().await;
            }
        } else if cur + 1 < total {
            chat::set_swipe(m, cur + 1);
            self.save().await;
        } else if len >= 2 {
            self.start_generation(true).await;
        } else if self.chat.messages.first().is_some_and(chat::is_summary_branch) {
            self.set_status(
                "the summary has no swipes \u{2014} Ctrl+\u{2190}/\u{2192} switches summary branch",
                false,
            );
        } else {
            self.set_status("no more greetings", false);
        }
    }
}
