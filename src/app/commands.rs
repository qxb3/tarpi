//! Sending the input box: `submit` decides whether the text is a message or a
//! `/command` and runs it.

use super::*;

impl App {
    pub(super) async fn submit(&mut self) {
        self.submit_inner().await;
        if self.input.text.is_empty() && self.editing.is_none() {
            self.input.enter_insert();
            self.input.reset_vim();
        }
    }

    pub(super) async fn submit_inner(&mut self) {
        if self.editing.is_some() {
            self.commit_edit().await;
            return;
        }
        if self.is_generating() {
            self.set_status(
                if self.chat.is_one_shot() {
                    "still generating \u{2014} wait for the reply to finish"
                } else {
                    "still generating \u{2014} press Esc to stop it first"
                },
                false,
            );
            return;
        }
        if self.apply_top_completion() {
            return;
        }
        let text = self.input.text.trim().to_string();
        self.hist_pos = None;

        if text.is_empty() {
            // Like the web UI: an empty send adds nothing to the chat, it just
            // asks the character to reply. (Chat Completion adds the "Replace
            // empty message" text from the prompt settings, see prompt.rs.)
            // This also retries when the last message is already the user's.
            self.start_generation(false).await;
            return;
        }

        // Anything but a second `/one-shot` withdraws the first one.
        if !matches!(text.to_lowercase().as_str(), "/one-shot" | "/oneshot") {
            self.one_shot_armed = false;
        }

        if let Some(cmd) = text.strip_prefix('/') {
            let head = cmd.split_whitespace().next().unwrap_or("").to_lowercase();
            // Commands that were understood go in the history (see `history_key`).
            let mut known = true;
            match head.as_str() {
                "help" | "h" | "?" => {
                    self.input.clear();
                    self.show_help = true;
                }
                "quit" | "q" | "exit" => {
                    self.input.clear();
                    self.quit().await;
                }
                "chats" | "chat" | "open" => {
                    self.input.clear();
                    self.open_chats_picker().await;
                }
                "character" | "char" => {
                    self.input.clear();
                    self.open_card_modal().await;
                }
                "char-img" | "charimg" => {
                    self.input.clear();
                    self.open_image_modal().await;
                }
                "characters" | "chars" | "switch" => {
                    self.input.clear();
                    self.open_characters_picker().await;
                }
                "import" => {
                    // Everything after the command is the card: a path, a link or JSON.
                    let arg = cmd.trim_start_matches(|c: char| !c.is_whitespace()).trim().to_string();
                    if arg.is_empty() {
                        known = false;
                        // Ready for the path (Tab opens its menu).
                        self.set_input_text("/import ".to_string());
                        self.set_status(crate::import::USAGE, true);
                    } else {
                        self.input.clear();
                        self.start_import(arg).await;
                    }
                }
                "persona" | "personas" | "me" => {
                    self.input.clear();
                    self.open_persona_picker().await;
                }
                "prompt" => {
                    self.input.clear();
                    self.show_prompt().await;
                }
                "system" | "sys" | "sysprompt" | "system-prompt" => {
                    self.input.clear();
                    self.open_sysprompt_picker().await;
                }
                "note" | "an" | "authors-note" | "author" => {
                    self.input.clear();
                    self.open_note().await;
                }
                "config" | "settings" => {
                    self.input.clear();
                    self.open_config().await;
                }
                "one-shot" | "oneshot" => {
                    self.input.clear();
                    self.one_shot().await;
                }
                "api" | "connection" | "conn" => {
                    self.input.clear();
                    self.open_api = true;
                }
                "formatting" | "format" => {
                    self.input.clear();
                    self.open_formatting = true;
                }
                "lore" | "lorebook" | "lorebooks" | "world-info" | "worldinfo" => {
                    self.input.clear();
                    self.open_lore = true;
                }
                "regen" | "regenerate" | "reroll" => {
                    // Anything after the command is a nudge for this reply.
                    let nudge = cmd
                        .trim_start_matches(|c: char| !c.is_whitespace())
                        .trim()
                        .to_string();
                    self.input.clear();
                    self.regenerate(if nudge.is_empty() { None } else { Some(nudge) })
                        .await;
                }
                "continue" | "cont" | "c" => {
                    self.input.clear();
                    self.continue_message().await;
                }
                "undo" | "u" | "delete" | "del" => {
                    // An optional count: `/undo 3` deletes the last three messages.
                    let arg = cmd.trim_start_matches(|c: char| !c.is_whitespace()).trim();
                    let count = if arg.is_empty() {
                        Some(1)
                    } else {
                        arg.parse::<usize>().ok().filter(|n| *n >= 1)
                    };
                    match count {
                        Some(n) => {
                            self.input.clear();
                            self.undo(n).await;
                        }
                        None => {
                            known = false;
                            self.set_status("usage: /undo [number of messages], e.g. /undo 3", true);
                        }
                    }
                }
                "new" => {
                    self.input.clear();
                    self.new_chat().await;
                }
                "newchat" | "nc" => {
                    self.input.clear();
                    self.delete_chat().await;
                }
                "deletebranch" | "deletebrach" | "db" => {
                    self.input.clear();
                    self.delete_branch().await;
                }
                "summarize" | "summarise" | "summary" | "summ" => {
                    self.input.clear();
                    self.summarize().await;
                }
                _ => {
                    known = false;
                    self.set_status(format!("unknown command /{} (try /help)", head), true);
                }
            }
            if known {
                self.cmd_hist.add(&text);
            }
            return;
        }

        let message = text;
        // Macros ({{user}}, {{char}}, {{roll:1d6}}, ...) are filled in when the
        // message is sent, like the web UI. No stray blank space around it.
        let message = chat::tidy(&prompt::subst_card(
            &message,
            &self.user_name,
            &self.persona,
            &self.character,
        ));
        self.input.clear();
        self.chat
            .messages
            .push(chat::user_message(&self.user_name, &message));
        self.scroll = 0;
        self.save().await;
        self.start_generation(false).await;
    }
}
