//! The character card's greetings: building a chat's first message, keeping
//! its swipes in step with the card, and writing an edited greeting back to the
//! card.

use super::*;

/// First message for a fresh chat: `first_mes` plus alternate greetings as swipes.
pub fn make_greeting(ch: &Character, user_name: &str, persona: &str) -> Option<Value> {
    let mut all: Vec<String> = Vec::new();
    if !ch.first_mes.trim().is_empty() {
        all.push(ch.first_mes.clone());
    }
    all.extend(ch.alternate_greetings.iter().cloned());
    let all: Vec<String> = all
        .iter()
        .map(|g| chat::tidy(&prompt::subst_card(g, user_name, persona, ch)))
        .collect();
    if all.is_empty() {
        None
    } else {
        Some(chat::greeting_message(&ch.name, &all))
    }
}

/// Chats started before the card had alternate greetings (or by another
/// client) carry only one greeting. Add the card's other greetings as swipes of
/// the first message, so Left/Right can browse them. Only a first message
/// that really is one of the card's greetings is touched. Returns true when
/// something was added.
pub fn sync_greetings(
    messages: &mut [Value],
    ch: &Character,
    user_name: &str,
    persona: &str,
) -> bool {
    let mut cards: Vec<String> = Vec::new();
    if !ch.first_mes.trim().is_empty() {
        cards.push(ch.first_mes.clone());
    }
    cards.extend(ch.alternate_greetings.iter().cloned());
    let cards: Vec<String> = cards
        .iter()
        .map(|g| chat::tidy(&prompt::subst_card(g, user_name, persona, ch)))
        .collect();
    if cards.len() < 2 {
        return false;
    }
    let Some(first) = messages.first_mut() else {
        return false;
    };
    if chat::is_user(first) || chat::is_system(first) {
        return false;
    }
    // A branched chat (original conversation + summary) keeps its first messages as they are.
    if chat::branch_pos(first).is_some() {
        return false;
    }
    let have: Vec<String> = match first.get("swipes").and_then(|s| s.as_array()) {
        Some(a) if !a.is_empty() => a
            .iter()
            .filter_map(|t| t.as_str())
            .map(chat::tidy)
            .collect(),
        _ => vec![chat::tidy(chat::mes(first))],
    };
    if !have.iter().any(|t| cards.contains(t)) {
        return false;
    }
    let mut missing: Vec<String> = Vec::new();
    for c in cards {
        if !have.contains(&c) && !missing.contains(&c) {
            missing.push(c);
        }
    }
    if missing.is_empty() {
        return false;
    }
    chat::add_greeting_swipes(first, &missing);
    true
}

impl App {
    /// Which card greeting message `idx` shows: only the very first chat
    /// message can be one. The text it shows is matched against the card's
    /// `first_mes` and alternate greetings (with names filled in, as they
    /// were when the chat was made), so it is right even when the swipes are
    /// not in card order. If nothing matches (the greeting was edited in this
    /// chat only), the swipe number is used, counting `first_mes` first.
    pub(super) fn greeting_slot_of(&self, idx: usize) -> Option<GreetingSlot> {
        if idx != 0 {
            return None;
        }
        let m = self.chat.messages.first()?;
        if chat::is_user(m) || chat::is_system(m) {
            return None;
        }
        let ch = &self.character;
        let mut cards: Vec<(GreetingSlot, String)> = Vec::new();
        if !ch.first_mes.trim().is_empty() {
            cards.push((GreetingSlot::First, ch.first_mes.clone()));
        }
        for (n, g) in ch.alternate_greetings.iter().enumerate() {
            cards.push((GreetingSlot::Alt(n), g.clone()));
        }
        let shown = chat::tidy(chat::mes(m));
        let by_text = cards.iter().find(|(_, raw)| {
            chat::tidy(&prompt::subst_card(raw, &self.user_name, &self.persona, ch)) == shown
        });
        if let Some((slot, _)) = by_text {
            return Some(*slot);
        }
        // The summary is not a greeting: never write it into the card.
        if chat::is_summary_branch(m) {
            return None;
        }
        let (cur, _) = chat::swipe_pos(m).unwrap_or((0, 1));
        cards.get(cur).map(|(slot, _)| *slot)
    }

    /// The card greeting the message being edited stands for.
    pub(super) fn editing_slot(&self) -> Option<GreetingSlot> {
        self.editing.as_ref()?.slot
    }

    /// Human-readable name of a greeting slot, for status messages.
    pub(super) fn slot_name(&self, slot: GreetingSlot) -> String {
        match slot {
            GreetingSlot::First => "first message".to_string(),
            GreetingSlot::Alt(n) => format!("alternate greeting {}", n + 1),
        }
    }

    /// Whether the extra "Card save" button applies right now.
    pub fn card_edit_available(&self) -> bool {
        self.editing_slot().is_some()
    }

    /// "Card save" / Ctrl+S: write the edited greeting into the character
    /// card for good (first message or the matching alternate greeting), then
    /// save it into this chat too, like a normal edit.
    pub(super) async fn commit_edit_to_card(&mut self) {
        let Some(slot) = self.editing_slot() else {
            self.set_status(
                "Ctrl+S saves to the card only while editing the card's greeting (first message)",
                false,
            );
            return;
        };
        let new_text = chat::tidy(&self.input.text);
        if new_text.trim().is_empty() && slot == GreetingSlot::First {
            self.set_status("the first message would be empty \u{2014} not saving it to the card", true);
            return;
        }
        match self.write_greeting_to_card(slot, &new_text).await {
            Ok(()) => {
                self.commit_edit().await;
                let which = self.slot_name(slot);
                self.set_status(format!("saved to the character card ({})", which), false);
            }
            // Stay in the edit so nothing typed is lost.
            Err(e) => self.set_status(format!("{:#}", e), true),
        }
    }

    pub(super) async fn write_greeting_to_card(&mut self, slot: GreetingSlot, text: &str) -> anyhow::Result<()> {
        let avatar = self.character.avatar.clone();
        // Read the card fresh: it may have been edited since the chat opened.
        let full = charstore::get(&avatar)?;
        let data = full.get("data").cloned().unwrap_or(Value::Null);
        // The chat shows {{char}} / {{user}} already replaced. Put them back
        // if the card text used them, so the card keeps working with any persona.
        let unsubst = |raw: &str| -> String {
            let lower = raw.to_lowercase();
            let mut out = text.to_string();
            let cname = self.character.name.as_str();
            let uname = self.user_name.as_str();
            // Put back the placeholder style the card used.
            if !cname.is_empty() {
                if lower.contains("{{char}}") {
                    out = out.replace(cname, "{{char}}");
                } else if lower.contains("<bot>") {
                    out = out.replace(cname, "<BOT>");
                } else if lower.contains("<char>") {
                    out = out.replace(cname, "<CHAR>");
                }
            }
            if !uname.is_empty() {
                if lower.contains("{{user}}") {
                    out = out.replace(uname, "{{user}}");
                } else if lower.contains("<user>") {
                    out = out.replace(uname, "<USER>");
                }
            }
            out
        };
        match slot {
            GreetingSlot::First => {
                let raw = {
                    let d = jstr(&data, "first_mes");
                    if d.trim().is_empty() { jstr(&full, "first_mes") } else { d }
                };
                let value = unsubst(&raw);
                charstore::set_field(&avatar, "first_mes", &Value::String(value.clone()))?;
                self.character.first_mes = value;
            }
            GreetingSlot::Alt(n) => {
                let mut arr: Vec<Value> = data
                    .get("alternate_greetings")
                    .or_else(|| full.get("alternate_greetings"))
                    .and_then(|a| a.as_array())
                    .cloned()
                    .unwrap_or_default();
                // `n` counts non-empty greetings only; find it in the raw list.
                let pos = arr
                    .iter()
                    .enumerate()
                    .filter(|(_, g)| g.as_str().map(|t| !t.trim().is_empty()).unwrap_or(false))
                    .nth(n)
                    .map(|(i, _)| i)
                    .ok_or_else(|| anyhow::anyhow!("that alternate greeting is no longer on the card"))?;
                let raw = arr[pos].as_str().unwrap_or("").to_string();
                let value = unsubst(&raw);
                arr[pos] = Value::String(value.clone());
                charstore::set_field(&avatar, "alternate_greetings", &Value::Array(arr))?;
                if let Some(slot) = self.character.alternate_greetings.get_mut(n) {
                    *slot = value;
                }
            }
        }
        Ok(())
    }
}
