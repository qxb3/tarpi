//! The `/commands` of the chat screen, for the completion menu that opens
//! above the input box while a command is being typed (see
//! `App::completion` and `ui::draw_completion`).
//!
//! Running a command is still done by the `match` in `App::submit_inner`;
//! this table only lists what the menu offers. Older spellings that still run
//! (`/characters`, `/character`, `/summarise`, ...) are deliberately not
//! listed, so `/char` offers `/char`, `/chars` and `/char-img` and nothing
//! else.

/// One entry of the completion menu.
pub struct Cmd {
    /// What the command is called, without the slash.
    pub name: &'static str,
    /// A short form that also finds it (`/summ` -> `/summarize`); empty if none.
    pub alias: &'static str,
    /// One line about what it does.
    pub desc: &'static str,
}

const fn cmd(name: &'static str, alias: &'static str, desc: &'static str) -> Cmd {
    Cmd { name, alias, desc }
}

/// In the order the menu shows them (for a given prefix, the ones whose
/// *name* starts with it come first, then the ones found by their short form).
pub const COMMANDS: &[Cmd] = &[
    cmd("help", "h", "all keys and commands"),
    cmd("chats", "", "open, create or delete chats"),
    cmd("char", "", "view and edit this character's card"),
    cmd("chars", "", "switch to another character"),
    cmd("char-img", "", "show the card's picture"),
    cmd("persona", "", "choose who you are in the chat"),
    cmd("api", "", "API connection and sampler settings"),
    cmd("new", "", "new chat with this character"),
    cmd("newchat", "nc", "delete this chat, start a new one"),
    cmd("regen", "", "regenerate the last reply [nudge]"),
    cmd("continue", "c", "continue the last reply"),
    cmd("undo", "u", "delete the last message [n]"),
    cmd("summarize", "summ", "summary of the chat as a new branch"),
    cmd("deletebranch", "db", "delete the branch that is showing"),
    cmd("one-shot", "", "lock this chat (permanent)"),
    cmd("quit", "q", "exit"),
];

/// The command word being typed, if `text` is one: it starts with `/` and has
/// no space or line break yet (after that it is arguments, or a message).
pub fn typed_word(text: &str) -> Option<&str> {
    let rest = text.strip_prefix('/')?;
    if rest.chars().any(char::is_whitespace) {
        return None;
    }
    Some(rest)
}

/// The commands that `word` (typed without the slash) is the start of,
/// ignoring case. Matches on the name come before matches on the short form.
pub fn matches(word: &str) -> Vec<&'static Cmd> {
    let w = word.to_lowercase();
    let mut by_name: Vec<&'static Cmd> = Vec::new();
    let mut by_alias: Vec<&'static Cmd> = Vec::new();
    for c in COMMANDS {
        if c.name.starts_with(&w) {
            by_name.push(c);
        } else if !c.alias.is_empty() && c.alias.starts_with(&w) {
            by_alias.push(c);
        }
    }
    by_name.extend(by_alias);
    by_name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(word: &str) -> Vec<&'static str> {
        matches(word).iter().map(|c| c.name).collect()
    }

    #[test]
    fn a_bare_slash_lists_everything() {
        assert_eq!(names("").len(), COMMANDS.len());
    }

    #[test]
    fn char_offers_only_the_short_names() {
        assert_eq!(names("char"), vec!["char", "chars", "char-img"]);
        assert_eq!(names("chars"), vec!["chars"]);
        assert!(names("characters").is_empty());
    }

    #[test]
    fn summ_finds_summarize() {
        assert_eq!(names("summ"), vec!["summarize"]);
        assert_eq!(names("SUM"), vec!["summarize"]);
    }

    #[test]
    fn short_forms_find_their_command() {
        assert_eq!(names("nc"), vec!["newchat"]);
        assert_eq!(names("db"), vec!["deletebranch"]);
        // `c` is the start of several names; `continue` is also found by its alias.
        let c = names("c");
        assert_eq!(c[..4], ["chats", "char", "chars", "char-img"]);
        assert!(c.contains(&"continue"));
    }

    #[test]
    fn every_name_and_alias_is_unique() {
        for (i, a) in COMMANDS.iter().enumerate() {
            for b in &COMMANDS[i + 1..] {
                assert_ne!(a.name, b.name);
                if !a.alias.is_empty() {
                    assert_ne!(a.alias, b.alias);
                    assert_ne!(a.alias, b.name);
                }
            }
        }
    }

    #[test]
    fn only_a_lone_command_word_is_completed() {
        assert_eq!(typed_word("/ch"), Some("ch"));
        assert_eq!(typed_word("/"), Some(""));
        assert_eq!(typed_word("/regen soft"), None);
        assert_eq!(typed_word("hello /ch"), None);
        assert_eq!(typed_word("/a\nb"), None);
    }
}
