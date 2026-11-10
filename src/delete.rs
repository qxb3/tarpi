//! `st-cli delete [character]`: delete a character card and all of its chats.
//!
//! The card file leaves `~/.config/st-cli/characters/` and the character's
//! folder in `~/.config/st-cli/chats/` goes with it. Nothing on the
//! SillyTavern server is touched.

use crate::charstore;
use crate::chatstore;
use crate::jv::*;
use crate::{char_display_name, confirm, find_character, pick_character};
use anyhow::{bail, Result};
use clap::Parser;
use std::io::{self, IsTerminal};

#[derive(Parser, Debug)]
#[command(
    name = "st-cli delete",
    about = "Delete a character card together with all of its chats",
    long_about = "Delete a character card from ~/.config/st-cli/characters/ and every chat \
                  it has in ~/.config/st-cli/chats/. This cannot be undone. You are asked to \
                  confirm first.",
    after_help = concat!(
        "EXAMPLES:\n",
        "  st-cli delete          pick a character from a list, then confirm\n",
        "  st-cli delete sofia    delete Sofia (a partial name works), then confirm\n",
        "  st-cli delete sofia -y delete Sofia without asking"
    )
)]
struct DeleteCli {
    /// Character name or card file name (a partial match works).
    /// Leave it out to pick from a list.
    character: Option<String>,

    /// Do not ask for confirmation
    #[arg(short = 'y', long)]
    yes: bool,

}

pub async fn run() -> Result<()> {
    // argv is `st-cli delete ...`; drop the program name so "delete" plays
    // the part of the binary name in clap's messages.
    let cli = DeleteCli::parse_from(std::env::args().skip(1));
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();

    let characters = charstore::list();
    let entry = match cli.character.as_deref() {
        Some(q) => find_character(&characters, q)?,
        None if interactive => pick_character(&characters)?,
        None => bail!("say which character to delete: st-cli delete <character>"),
    };
    let avatar = jstr(entry, "avatar");
    let name = char_display_name(entry);

    // A character with no chat folder yet simply has no chats.
    let chats = chatstore::list(&avatar).map(|c| c.len()).unwrap_or(0);
    let what = match chats {
        0 => "it has no chats".to_string(),
        1 => "and its 1 chat".to_string(),
        n => format!("and its {} chats", n),
    };

    if !cli.yes {
        if !interactive {
            bail!("refusing to delete without a terminal to confirm on; add -y to skip the question");
        }
        println!("Delete \"{}\" ({}) -- {}. This cannot be undone.", name, avatar, what);
        if !confirm("Really delete? [y/N] ") {
            println!("cancelled");
            return Ok(());
        }
    }

    charstore::delete(&avatar)?;
    chatstore::delete_all(&avatar)?;
    match chats {
        0 => println!("Deleted \"{}\".", name),
        n => println!("Deleted \"{}\" and {} chat{}.", name, n, if n == 1 { "" } else { "s" }),
    }
    Ok(())
}
