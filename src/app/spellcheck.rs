//! Spell checking of the input box and its suggestion menu.

use super::*;

impl App {
    /// Misspelled words (char ranges) of `text`, the text of the input box.
    /// Empty when spell checking is off. The word the cursor is in is left
    /// alone while typing (INSERT mode), since it is not finished yet.
    pub fn spell_marks(&mut self, text: &str) -> Vec<(usize, usize)> {
        // The `/config` window holds YAML, not prose: nothing is underlined.
        if self.config_edit {
            return Vec::new();
        }
        self.spell_touch();
        let typing = (self.input.mode == crate::input::Mode::Insert && !self.spell_rested())
            .then_some(self.input.cursor);
        self.spell_names();
        match self.spell.as_mut() {
            Some(sp) => sp.marks_typing(text, typing),
            None => Vec::new(),
        }
    }

    /// The text box being typed into: the one of the open `/char` or persona
    /// window, otherwise the chat box.
    pub(super) fn spell_input(&self) -> &crate::input::Input {
        match self.card_modal.as_ref() {
            Some(c) => &c.input,
            None => &self.input,
        }
    }

    /// Note a change of text, cursor or mode: the menu closes and the rest
    /// timer (the word at the cursor is underlined once it is over) starts
    /// again.
    pub fn spell_touch(&mut self) {
        let mut h = DefaultHasher::new();
        let inp = self.spell_input();
        inp.text.hash(&mut h);
        inp.cursor.hash(&mut h);
        (inp.mode as u8).hash(&mut h);
        self.card_modal.as_ref().map(|c| c.focus).hash(&mut h);
        let key = h.finish();
        if key != self.spell_key {
            self.spell_key = key;
            self.spell_since = Instant::now();
            self.spell_menu = None;
        }
    }

    /// The cursor has not moved and nothing was typed for a second.
    pub fn spell_rested(&self) -> bool {
        self.spell_since.elapsed() >= SPELL_DELAY
    }

    /// The misspelled word the cursor is in (or right behind), if any.
    pub(super) fn spell_word_at_cursor(&mut self) -> Option<((usize, usize), String)> {
        let inp = self.spell_input();
        if !matches!(inp.mode, Mode::Insert | Mode::Normal) {
            return None;
        }
        // In the windows the Name fields are not checked.
        if let Some(c) = self.card_modal.as_ref() {
            let f = c.fields.get(c.focus)?;
            if !f.editable || f.key == "name" || c.pick_of(c.focus).is_some() {
                return None;
            }
        }
        let text = inp.text.clone();
        let cur = inp.cursor;
        let (s, e) = self
            .spell
            .as_mut()?
            .marks(&text)
            .into_iter()
            .find(|&(s, e)| cur >= s && cur <= e)?;
        let word: String = text.chars().skip(s).take(e - s).collect();
        Some(((s, e), word))
    }

    /// Open the menu for the word at the cursor.
    pub(super) fn spell_open(&mut self) -> bool {
        self.spell_names();
        let Some((range, word)) = self.spell_word_at_cursor() else {
            return false;
        };
        let sugg = self
            .spell
            .as_ref()
            .map(|sp| sp.suggest(&word, 6))
            .unwrap_or_default();
        if sugg.is_empty() {
            return false;
        }
        self.spell_menu = Some(SpellMenu {
            range,
            word,
            sugg,
            sel: 0,
        });
        true
    }

    /// Keys for the spelling menu: Tab opens it on a misspelled word (and,
    /// open, moves down); Shift+Tab / Up / Down move; Enter puts the
    /// highlighted entry in; Esc closes it. Returns false when the key is
    /// not for the menu (and then closes it).
    pub(super) fn spell_key(&mut self, k: &KeyEvent) -> bool {
        if self.spell.is_none()
            || self.config_edit
            || !matches!(self.spell_input().mode, Mode::Insert | Mode::Normal)
            || k.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            || (self.card_modal.is_none()
                && (self.completion().is_some() || self.path_completion().is_some()))
        {
            self.spell_menu = None;
            return false;
        }
        if self.spell_menu.is_none() {
            return k.code == KeyCode::Tab && self.spell_open();
        }
        let Some(m) = self.spell_menu.as_mut() else {
            return false;
        };
        let n = m.sugg.len().max(1);
        match k.code {
            KeyCode::Tab | KeyCode::Down => m.sel = (m.sel + 1) % n,
            KeyCode::BackTab | KeyCode::Up => m.sel = (m.sel + n - 1) % n,
            KeyCode::Enter => {
                let i = m.sel;
                self.spell_accept(i);
            }
            KeyCode::Esc => self.spell_menu = None,
            _ => {
                self.spell_menu = None;
                return false;
            }
        }
        true
    }

    /// Replace the misspelled word with suggestion `i`.
    pub fn spell_accept(&mut self, i: usize) {
        let Some(m) = self.spell_menu.take() else {
            return;
        };
        let Some(new) = m.sugg.get(i) else {
            return;
        };
        let put = |inp: &mut crate::input::Input| {
            // One undo step of its own: `u` puts the misspelled word back.
            inp.replace_chars(m.range.0, m.range.1, new);
            // In NORMAL mode the cursor sits on a letter, not after the last one.
            inp.clamp_normal();
        };
        match self.card_modal.as_mut() {
            Some(c) => {
                put(&mut c.input);
                c.follow = true;
                c.sync();
            }
            None => {
                put(&mut self.input);
                self.scroll = 0;
            }
        }
    }

    /// Tell the spell checker the names it must accept (character, persona).
    pub fn spell_names(&mut self) {
        if self.spell.is_none() {
            return;
        }
        // The character (its name and card file name) and every persona
        // (names and titles), so nothing the story is about gets marked.
        let avatar = self.character.avatar.trim_end_matches(".png").to_string();
        let mut names: Vec<String> = vec![
            self.character.name.clone(),
            avatar,
            self.user_name.clone(),
            self.persona_title.clone(),
        ];
        for p in crate::persona::list() {
            names.push(p.name);
            names.push(p.title);
        }
        let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        if let Some(sp) = self.spell.as_mut() {
            sp.set_names(&refs);
        }
    }
}
