//! `/import`: the background download and what happens when it is done
//! (including the lorebook question that may follow).

use super::*;

impl App {
    /// `/import <card>`: add a character card (file, chub.ai / pygmalion.chat
    /// link, direct link or JSON text) to the characters folder. It runs in
    /// the background, so the chat stays usable while a download is going.
    /// The card lands in the data folder, which under `tarpi connect` is the
    /// copy that the running sync uploads, so the other devices get it too.
    pub(super) async fn start_import(&mut self, arg: String) {
        if self.import_job.is_some() {
            self.set_status("an import is already running", false);
            return;
        }
        let job = tokio::spawn(async move {
            let saved = crate::import::import(&arg).await?;
            let name = if saved.name.is_empty() { saved.avatar.clone() } else { saved.name.clone() };
            crate::connect::report(format!("imported character {} ({})", name, saved.avatar));
            Ok::<_, anyhow::Error>(saved)
        });
        self.status = None;
        self.import_job = Some((job, Instant::now()));
    }

    /// Pick up the result of `/import` once it is done (called on every tick).
    pub(super) fn collect_import(&mut self) {
        if !self.import_job.as_ref().is_some_and(|(j, _)| j.is_finished()) {
            return;
        }
        let Some((job, _)) = self.import_job.take() else {
            return;
        };
        match job.now_or_never() {
            Some(Ok(Ok(saved))) => {
                let name = if saved.name.is_empty() { saved.avatar.clone() } else { saved.name.clone() };
                // A card with a lorebook: ask whether to copy it into /lore
                // and link it to this character.
                let ask = charstore::get(&saved.avatar)
                    .and_then(|card| crate::lorestore::from_card(&card, &saved.avatar))
                    .ok()
                    // Not again when a book of that name (or from this very
                    // card) is already among the user's lorebooks.
                    .filter(|imp| {
                        !crate::lorestore::list().0.iter().any(|b| {
                            b.name.trim().eq_ignore_ascii_case(imp.book.name.trim())
                                || (!b.source.is_empty() && b.source == imp.book.source)
                        })
                    })
                    .map(|imp| {
                        LoreAsk::new(saved.avatar.clone(), name.clone(), imp.book.name.clone(), imp.book.entries.len())
                    });
                let asking = ask.is_some();
                if let Some(a) = ask {
                    self.lore_pending.push(a);
                    self.promote_lore_ask();
                }
                self.set_status(
                    format!(
                        "imported \"{}\" ({}) \u{2014} {}",
                        name,
                        saved.avatar,
                        if asking { "it has a lorebook" } else { "switch to it with /chars" }
                    ),
                    false,
                );
            }
            Some(Ok(Err(e))) => {
                // One row: the message may have line breaks (a download that failed several ways).
                let msg = format!("{:#}", e).split_whitespace().collect::<Vec<_>>().join(" ");
                self.set_status(format!("import failed: {}", msg), true);
            }
            Some(Err(_)) => self.set_status("import failed (stopped)", true),
            None => {}
        }
    }

    /// Is a window open that the lorebook question must not cover?
    pub(super) fn other_window_open(&self) -> bool {
        self.show_help
            || self.prompt_view.is_some()
            || self.image_modal.is_some()
            || self.card_modal.is_some()
            || self.pos_edit.is_some()
            || self.picker.is_some()
    }

    /// Show the next waiting lorebook question when nothing else is open.
    pub(super) fn promote_lore_ask(&mut self) {
        if self.lore_ask.is_none() && !self.lore_pending.is_empty() && !self.other_window_open() {
            self.lore_ask = Some(self.lore_pending.remove(0));
        }
    }

    /// The answer to the lorebook question: *Yes* copies the card's lorebook
    /// into `/lore` and puts the character in its *On for* list, *No* leaves
    /// it (it can still be copied later with `c` in `/lore`).
    pub(super) fn lore_ask_action(&mut self, act: LoreAskAction) {
        match act {
            LoreAskAction::None => {}
            LoreAskAction::No => {
                if let Some(a) = self.lore_ask.take() {
                    self.set_status(
                        format!(
                            "lorebook of {} not copied \u{2014} /lore, then c, copies it later",
                            a.character
                        ),
                        false,
                    );
                }
                self.promote_lore_ask();
            }
            LoreAskAction::Yes => {
                let Some(a) = self.lore_ask.as_ref() else {
                    return;
                };
                let (avatar, character) = (a.avatar.clone(), a.character.clone());
                let result = charstore::get(&avatar)
                    .and_then(|card| crate::lorestore::from_card(&card, &avatar))
                    .map(|mut imp| {
                        imp.book.characters.push(avatar.trim_end_matches(".png").to_string());
                        imp
                    })
                    .and_then(crate::lorestore::add);
                match result {
                    Ok(b) => {
                        self.lore_ask = None;
                        self.set_status(
                            format!(
                                "lorebook '{}' ({} entries) copied and linked to {}",
                                b.name,
                                b.entries.len(),
                                character
                            ),
                            false,
                        );
                        self.promote_lore_ask();
                    }
                    Err(e) => {
                        if let Some(a) = self.lore_ask.as_mut() {
                            a.note = Some(format!("could not copy: {:#}", e));
                        }
                    }
                }
            }
        }
    }
}
