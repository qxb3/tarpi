# tarpi

A standalone, chat-only terminal client for character cards. Characters, personas, system prompts, formatting, API connections and chats are all tarpi's own, so nothing needs a SillyTavern server (or any server except your model backend).

Characters, personas and system prompts are tarpi's own, kept in `~/.config/tarpi/characters/`, `~/.config/tarpi/persona/` and `~/.config/tarpi/system-prompt.jsonl` (see Importing characters, Personas and System prompts). Chats are tarpi's too (`~/.config/tarpi/chats/`). `tarpi` only chats. Chats are saved as SillyTavern-format `.jsonl` files in `~/.config/tarpi/chats/`.

```
tarpi                  # pick a character, then chat
tarpi sofia            # continue the latest chat with Sofia
tarpi sofia 3          # open Sofia's chat number 3 (or by name: Sofia03)
tarpi sofia -n         # start a new chat (named Sofia01, Sofia02, ...)
tarpi -l               # list characters
tarpi sofia -l         # list Sofia's chats
tarpi import <source>  # add a character card to ~/.config/tarpi/characters (see below)
tarpi import-st <url>  # copy the personas, system prompts and characters of a SillyTavern server
tarpi delete [sofia]   # delete a character card and all its chats (asks first)
```

That is the whole command line: `[character] [chat]`, plus `-n` (new chat) and `-l` (list), and the commands `import`, `import-st` and `delete`. The server address is set with `url` in `~/.config/tarpi/config.yaml`.

Everything else happens inside the chat:

| Command | What it does |
| --- | --- |
| `/chats` | opens a window over the chat with this character's chats: pick one (↑/↓ + Enter, or click / tap), choose `+ new chat`, or press `d` on one to delete it (asks `y`). Esc closes. The `_before-summary` copies are not listed. The chat that is open can't be deleted here (use `/newchat`), except when it is the last one: then it is deleted and tarpi exits |
| `/one-shot` | locks **this chat file** as it is: editing, `/undo`, regenerating, `/continue`, swipes (and greeting switching) and summary branches are turned off. The status row asks you to type `/one-shot` a second time; typing anything else cancels. A grey `ONE SHOT MODE` tag stays at the start of the status row below the input box for as long as the chat is open; other status messages follow it after a `·`. A reply that is being written can't be stopped either (no Esc, no Stop button, and `/quit`, `/chats` and `/chars` wait until it is done). Sending new messages still works, and other chats are not affected. It is stored in the chat file's metadata (`st_cli_one_shot`); there is no command to undo it |
| `/persona` | opens a window to choose who you are in the chat; `+ new persona` creates one, `e` edits, `p` sets where its description goes into the prompt, `d` deletes (see Personas) |
| `/system` | opens a window to choose the system prompt; `+ new system prompt` creates one, `e` edits, `d` deletes (see *System prompts*) |
| `/prompt` | opens a window with the whole prompt the next reply would be sent with, as plain text (see *Prompt window*) |
| `/note` or `/an` | opens this chat's Author's Note in a form: the text and where, how deep and how often it is sent (see *Author's Note*) |
| `/chars` | opens a window with every character (★ = favourite) to switch to someone else |
| `/api` | open the API settings (llama.cpp address and every model setting) right here; when you quit them (`q` / Esc) you are back in this same chat, and the very next reply uses what you saved |
| `/formatting` | open the formatting editor: context template (story string), instruct template and reply cleanup; when you quit it (`q` / Esc) you are back in this same chat (see *Formatting*) |
| `/char` | open this character's card in a window: every field, editable, with **Cancel** and **Save** (see below) |
| `/char-img` | shows the character card's picture in a square window (full resolution in terminals with the Kitty graphics protocol, iTerm2 images or Sixel (kitty, WezTerm, Ghostty, iTerm2, foot, Windows Terminal, mlterm); elsewhere, including Termux and tmux, it is drawn with coloured half-block cells, which is coarse. `TARPI_IMAGE=blocks|kitty|iterm|sixel` forces one (`sixel` is never auto-detected beyond the terminals listed); any key or click closes it) |
| `/new` | new chat with this character right away (keeps the current one) |
| `/newchat` or `/nc` | delete this chat and start a new one |
| `/summarize` or `/summ` | write an in-depth summary of the chat as a new summary branch (see below) |
| `/deletebranch` or `/db` | delete the summary branch that is showing (see *Summarizing a long chat*) |

### Command menu

Typing `/` in the input box opens a menu of commands just above it, narrowing as you type (`/ch` lists `/chats`, `/char`, `/chars`, `/char-img`). The top entry is highlighted from the start. It shows while the box holds a single `/word` in INSERT mode (and not while a message is being edited); once there is a space after the command (`/regen softer`) it closes.

| Key / click | Action |
| --- | --- |
| Tab or Down | move the highlight to the next command and fill it in (the top one is highlighted from the start, so the first press already goes to the second) |
| Shift+Tab | move the highlight to the previous command and fill it in; wraps around (from the top one to the last) |
| Up | on a bare `/` once a command has been run: the command history instead (see *Command history*). Otherwise the same as Shift+Tab |
| Esc | put back what you had typed before Tab started filling commands in |
| click / tap | fill in that command and close the menu |
| Enter | run the highlighted command when what you typed is only the start of one (`/su` runs `/summarize`); a full name or short form (`/c`, `/nc`) runs as typed |

### Command history

Every command that tarpi understood is remembered, with what you typed after it: `/regen she hesitates, then refuses`, `/undo 3`, `/chats`. With nothing but a `/` in the box, **Up** puts the newest one there; Up again goes further back, **Down** comes forward, and Down past the newest puts the `/` back. Enter runs the command shown, and it moves to the newest place in the history. Up stops at the oldest one.

Typing or editing anything while a command is shown ends the walk: the keys then do what they did before (Up / Down in the menu, or scrolling the chat). Tab and Shift+Tab always move through the command menu. Only INSERT mode and a box holding just `/` start a walk, and it works with `command_menu: false` too.

The history keeps the last 200 different commands in `~/.config/tarpi/command_history` (one per line, oldest first), so it is still there after quitting and carries over when you switch chat or character; several terminals share it. Unknown commands and ordinary messages are not kept. Delete the file to clear it.

The menu lists the short names `/char` and `/chars` and the short forms `/summ`, `/nc`, `/db`, `/c`, `/u`, `/q` and `/h` (shown in brackets). The longer spellings `/character`, `/characters` and `/summarise` still work when typed in full, but are not offered. While the menu is open Up / Down move through it instead of scrolling the chat.

`<character>` is matched case-insensitively against the name or card file name; a unique partial match works. New chats are numbered per character (`Sofia01`, `Sofia02`, ...). A chat can be given as a number, an exact name, or a unique partial name.

## API connection (`/api`)

The API connection belongs to tarpi, not to SillyTavern. `/api` inside a chat opens a full-screen manager; connections are stored in `~/.config/tarpi/api.yaml`. Replies go **straight to the backend** (llama.cpp's `/completion`), SillyTavern's API selection and sampler presets are ignored, and the token count in the status row comes from the backend's own `/tokenize`. SillyTavern is still used for some settings. There is no fallback to SillyTavern's API: if `api.yaml` has no active connection, sending a message shows `no API connection` and nothing is generated.

There is no separate command for it: type `/api` in any chat. The manager takes over the same terminal window and the chat is not closed, so when you quit it (`q` / Esc) you are back exactly where you were: same chat, same scroll position, unsent draft and swipes untouched. Whatever you saved applies at once (title bar and token count update) and to the next reply. If there is no connection yet, the chat starts with a hint to type `/api`, and the first time you open it a connection named `local` is created for you to fill in. On a phone, mouse reporting is switched on while the manager is open (so sliders can be tapped) and goes back to what the chat had when you leave it.

The first time it opens it creates a connection named `local` (llama.cpp at `http://127.0.0.1:8080`). Left: the connections (★ = the one chats use). Right: the server address, an optional API key (only for servers started with `--api-key`) and every setting of the selected connection, grouped as Generation, Sampling, Repetition, DRY, XTC, Mirostat, Dynamic temperature and Advanced. Numbers are sliders; settings that differ from their default are yellow.

| Key / click | Action |
| --- | --- |
| Up / Down (or `j` / `k`) | select a setting (in the list: select a connection) |
| Left / Right (or `h` / `l`) | move a slider by one step, flip a switch. Shift+arrows, `H` / `L`: 10 steps |
| Home / End | slider to its minimum / maximum |
| Enter | type an exact value (sliders, numbers, text, address, key); flips a switch |
| click / drag on a slider, wheel | set the value with the mouse; the wheel scrolls |
| `r` (or Backspace) | this setting back to its default; `R` resets every setting of the connection |
| `t` | test the connection: shows the loaded model and the server's context size |
| `c` | set *Context size* to the one the server reported |
| `p` | presets: pick **Normal**, **Creative** or **Crazy** and press Enter to apply it to the selected connection (`*` marks the one in use, `1`-`3` jump to one, Esc cancels) |
| `a` | use this connection for chats (★) |
| `n` / `d` | new connection / delete the selected one |
| `[` `]`, Tab | previous / next connection; Tab moves between the list and the settings |
| `s` | save to `api.yaml` (asking on quit if there are unsaved changes) |
| `q` / Esc | quit |

A preset sets every sampler setting (Sampling, Repetition, DRY, XTC, Mirostat, Dynamic temperature) in one go, so anything it does not name goes back to its default; Generation and Advanced settings (reply length, context size, stop strings, sampler order, grammar) and the address are left alone. Applying one only changes the settings in the window: press `s` to save. You can fine-tune afterwards with the sliders. **Normal** is balanced (temperature 0.8, min-p 0.05), **Creative** raises temperature to 1.15 and turns DRY on, and **Crazy** goes to temperature 1.8 with XTC, DRY and dynamic temperature. A new kind of connection supplies its own through `Provider::presets`.

Chats re-read `api.yaml` before every reply, so saving in `/api` (even from another terminal's chat) applies to the next message. *Context size* is how much history tarpi sends; keep it at or below the server's `-c`. `stop strings`, `DRY breakers` and `Sampler order` are text fields (the first two a JSON list); a value that is not valid is refused when you press Enter.

Only llama.cpp is built in, but a connection kind is one small file: implement `conn::Provider` (its settings list, request body and stream parser; see `src/conn/llamacpp.rs`) and add it to `conn::providers()`. The manager, the file format, token counting and generation pick it up without further changes.

## Formatting

`/formatting` opens a full-screen editor for how the prompt is laid out. The settings are tarpi's own, kept in `~/.config/tarpi/formatting.yaml` (every setting is written out; hand edits are fine). Until you have saved it, the built-in defaults apply, and the editor starts from them. Nothing here is read from a SillyTavern server.

- **Context template**: the story string (`{{#if description}}...{{/if}}`, `{{description}}`, `{{personality}}`, `{{scenario}}`, `{{persona}}`, `{{system}}`, `{{char}}`, `{{user}}`), the example separator and chat start, and the stop-on-separators / stop-on-names switches.
- **Instruct template**: instruct mode on/off, names (`none` / `force` / `always`), wrap and macros, the prefix and suffix of user, assistant and system messages, first / last assistant prefix, the stop sequence, sequences as stop strings, and the story string prefix / suffix.
- **Reply cleanup**: trim unfinished sentences, collapse blank lines, single line.

Texts are typed on one line in a small window: type `\n` for a line break and `\\` for a backslash. `p` offers presets for the part the cursor is in, as starting points. **Instruct template**: Off, ChatML, Llama 3, Llama 2, Mistral, Mistral V7, Phi-3, Phi-4, Gemma, Zephyr, Command R, DeepSeek V3, OpenChat, Granite, Vicuna, Alpaca and Metharme (roughly the chat templates llama.cpp knows); they replace the instruct template and keep the story string. **Context template**: Default, Short, Labeled, Pygmalion and No separators; they replace the story string, example separator and chat start and keep the stop switches. Reply cleanup has no presets.

| Key | Action |
| --- | --- |
| ↑ / ↓ (`k` / `j`), PgUp / PgDn, Home / End | select a setting (click works too; a second click acts on it) |
| Enter | edit a text (Enter accepts, Esc cancels) or flip / step the setting |
| ← / → (`h` / `l`) | flip a switch or step through the choices |
| `r` / `R` | reset the setting / every setting to its default |
| `p` | presets of the template the cursor is in (↑ / ↓, Enter applies) |
| `s` | save |
| `q` / Esc | quit (asks to save when there are unsaved changes) |

The next reply uses what you saved; nothing is sent to the server.

## Personas

Personas belong to tarpi, not to SillyTavern. Each one is a small YAML file in `~/.config/tarpi/persona/` (`<id>.yaml` with `name`, `title`, `description`, `position`, `depth`, `role`, `lorebook`; hand edits are fine), and the file `persona/active` holds the id of the one in use. SillyTavern's own persona settings are never read for chatting and never written.

`/persona` inside a chat opens a window with every persona as `Name (Title)` with a short preview of its description; the active one is marked `(current)`. Pick one with ↑/↓ + Enter (or click / tap); Esc closes it. The choice applies to the chat from the next message on (and to chats open in other terminals, which re-read the folder before every reply). The first row, `+ new persona`, opens a form (name, title, description; Ctrl+S saves, Esc cancels). `e` on a persona opens the same form for it; `d` deletes it (asks `y`; the persona in use can't be deleted).

`p` on a persona opens a small window for where its description is injected into the prompt (the `position`, `depth` and `role` of its file):

| Row | Values |
| --- | --- |
| Position | **In the prompt** (`position: 0`, the default: with the character card, in the context template's story string), **Top of the Author's Note** (`2`) or **Bottom of the Author's Note** (`3`): added above / below the text of the chat's Author's Note and sent wherever that goes (see *Author's Note*), **In the chat, at a depth** (`4`: added to the chat history), or **Not sent** (`9`: the description is left out) |
| Depth | only for the chat position: messages from the end, `0` is after the last message (default `2`) |
| Role | only for the chat position: whether the injected message is a `system`, `user` or `assistant` message |

↑/↓ (or `j` / `k`, Tab) pick a row, ←/→ (or `h` / `l`) change it (Shift+arrows, PgUp / PgDn, `H` / `L`: ten at a time on Depth), digits type the depth, Enter or Ctrl+S saves and Esc cancels; both bring the `/persona` window back. With the mouse or a finger, a click on the left half of a row steps back and on the right half steps on; a click outside closes it. The change applies from the next message on, like switching persona. A persona imported from SillyTavern with its old *After the character* position (`1`) shows it as *not supported, not sent*; stepping to another position replaces it. With no persona saved yet the list shows a plain `User`, which is written to disk once you edit it.

## Prompt window

`/prompt` shows the prompt exactly as tarpi would send it for the next reply, built right then from the current settings, persona, system prompt, character card and chat: the system prompt and character card (the context template's story string), the example dialogue, the instruct formatting, and the messages that fit into the *Context size* (older ones that would be cut are not shown, like in the real request). It is not changed or sent by opening it, and nothing is saved. The title shows its size in characters and an estimated token count (`~`). On a summary branch it is the prompt of the branch that is showing.

| Key / click | Action |
| --- | --- |
| Up / Down (or `k` / `j`), wheel | scroll one row (the wheel three) |
| PgUp / PgDn (or Space) | scroll a page |
| Ctrl+U / Ctrl+D | half a page up / down |
| Home / End (or `g` / `G`) | top / bottom |
| Esc, `q`, Enter, Ctrl+C or a click | close |

Long lines are wrapped to the window; tabs are shown as four spaces. The window is read-only and a typed draft in the box is not part of it (the box holds `/prompt` while the window is open).

## Author's Note

The Author's Note is a short text that belongs to one chat and is added to the prompt of the replies, to steer the story (`Keep the pace slow.`, `[Style: terse, present tense]`). It is kept in the chat file's metadata under SillyTavern's own names (`note_prompt`, `note_position`, `note_depth`, `note_role`, `note_interval`), so a chat copied from SillyTavern keeps its note, and the web UI shows the one written here.

`/note` (or `/an`) opens a form with five fields; Ctrl+S saves, Esc cancels (the fields work like those of `/char`, including the vim modes and the spell checker). The settings are typed as words and checked when you save:

| Field | Values |
| --- | --- |
| Author's note | the text; `{{char}}`, `{{user}}` and the other macros work. Empty = nothing is sent |
| Position | `chat` (default): into the chat history, `depth` messages from the end. `after`: with the system prompt and character card, after them. `before`: the same, before them |
| Depth | only for `chat`: messages from the end, `0` is after the last message (default `4`) |
| Role | only for `chat`: whether the injected message is a `system` (default), `user` or `assistant` message |
| Interval | the note is sent with every N-th message of yours: `1` (default) every reply, `3` every third, `0` never |

`before` and `after` fill `{{anchorBefore}}` and `{{anchorAfter}}` of the context template's story string; a template that has neither still gets the note, before or after the story string. The change applies from the next message on (the token count and `/prompt` change at once). The note is per chat: a new chat starts without one, and `/newchat` does not carry it over.

A persona whose position is **Top** or **Bottom of the Author's Note** (see *Personas*) is added above or below the note's text, on its own line, and travels with the note: the note's Position, Depth and Role decide where it goes. It is sent even when the note has no text, and also when the Interval says the text is skipped for this reply.

## System prompts

System prompts belong to tarpi, not to SillyTavern. They are kept in one file, `~/.config/tarpi/system-prompt.jsonl`, one JSON object per line (hand edits are fine):

```
{"id":"roleplay-simple","name":"Roleplay - Simple","content":"You are {{char}}. ...","post_history":"","active":true}
```

`id` is the short file-name-like id, `name` what the window shows, `content` the system prompt, `post_history` the post-history instructions, and `"active": true` marks the prompt in use (only one line has it; the key is left out on the others). The text of the prompt in use is what goes into the `{{system}}` part of the context template for every reply, exactly where SillyTavern's own system prompt used to go. SillyTavern's system prompt settings are no longer read for chatting and never written. With no system prompt saved or chosen the system prompt is empty (the character card's own system prompt and post-history instructions, if it has them, still apply).

`/system` inside a chat opens a window with every system prompt by name and a short preview of its text; the one in use is marked `(current)`. Pick one with ↑/↓ + Enter (or click / tap); Esc closes it. The choice applies from the next message on (and to chats open in other terminals, which re-read the file before every reply). The first row, `+ new system prompt`, opens a form (name, system prompt, post-history instructions; Ctrl+S saves, Esc cancels, the fields work like those of `/char`, including the vim modes and the spell checker). `e` on a prompt opens the same form for it; `d` deletes it (asks `y`; the one in use can't be deleted). A new prompt is not switched on by itself: pick it in the window.

`{{char}}`, `{{user}}` and the other macros work in a system prompt as they did in SillyTavern. A character card's own system prompt still replaces the prompt in use (`{{original}}` in it stands for the prompt in use), as before.

### Importing from SillyTavern

```
tarpi import-st http://127.0.0.1:8000
tarpi import-st http://127.0.0.1:8000 --overwrite
```

Copies every persona (and every system prompt and character, see System prompts and Characters) of that SillyTavern server (name, title, description, position, depth, role, lorebook) into the persona folder. The server is only read, never changed, and the personas' pictures are not copied. `<url>` may be left out to use `url` from `config.yaml`, and `host:port` works without `http://`. Personas may share a name: each gets its own file (`alex.yaml`, `alex-2.yaml`, ...; the window tells them apart by title and description preview, and a new persona made with `+ new persona` can reuse a name too). A persona identical to one already here (same name, title and description) is skipped, so running it again is safe; `--overwrite` / `-o` replaces the one with the same name and title instead of adding another. If you have not chosen a persona here yet, the one the server had active becomes yours too.

The server's system prompts (its system prompt presets, plus the one it has switched on when that is not one of them) go into `system-prompt.jsonl` the same way: an identical one (same name, prompt and post-history instructions) is skipped, so running it again is safe, and `--overwrite` replaces one with the same name instead of adding `name-2`. If you have not chosen a system prompt here yet, the one the server had on becomes yours too.

The chat's top bar reads ` tarpi <version> ` (white badge) `<character>/<chat file> • <persona> (<title>) • <api>@host:port`, e.g. `Mirei/Mire01`. The API type and `host:port` are those of the active connection (`/api`). The connection turns red when the backend does not answer (checked in the background every 5 seconds) or when no connection is set. Message names are black on white, with a grey rule between messages (none under the last one).

## Card save

While you edit the chat's first message, Ctrl+S (or the **Card save** button) writes the edit into the character card itself. tarpi works out which greeting you are looking at by matching its text against the card's first message and alternate greetings, so it updates the right one (`first_mes` or the matching alternate greeting) even when the swipes are in a different order. The status line says which one was saved. `{{char}}` / `{{user}}` (or `<BOT>` / `<USER>`) are put back in the card text if the original greeting used them.

## Character window

`/char` opens the whole card of the current character in a window, like `/help`: card file, name, description, personality, scenario, first message, example dialogue, system prompt, post-history instructions, every alternate greeting and the depth prompt. The text is the raw card text (`{{char}}` / `{{user}}` stay as they are).

All fields except the card file and the depth prompt (shown read-only) can be edited, including the name: renaming writes the new name to the card (the card file and your chats keep their names). The name is one line and can't be empty. The field with the cursor has a cyan title; the window scrolls to follow it. It opens with the cursor at the very top of the first editable field (Name), in NORMAL mode (with vim modes on; press `i` to type).

| Key / click | Action |
| --- | --- |
| Tab | with `spellcheck: true`: spelling suggestions for the misspelled word at the cursor (see *Configuration*); it no longer jumps to the next field. Move between fields with Up / Down past the first / last row, or click |
| Up / Down | move a row; past the first / last row of a field they go on to the previous / next field |
| Enter | new line inside the field |
| Left, Right, Home, End, Ctrl+A / Ctrl+E, Backspace, Delete, PgUp / PgDn | as in a text box |
| Ctrl+U / Ctrl+D (NORMAL / VISUAL mode) | half a window up / down; on reaching the edge of a field, move into the previous / next field |
| click | put the cursor in that field |
| mouse wheel | scroll the window |
| Ctrl+S, `:w` (NORMAL mode, vim modes on), or the **Save** button | write the changed fields to the character card file in `~/.config/tarpi/characters/` (as Card save does) and close |
| Esc (see above), or the **Cancel** button | close without saving |

The focused field is edited with the same editor as the chat box, so the vim modes work there too (unless `vim.enabled: false`): `qq` typed quickly or Esc goes to NORMAL mode, `i a I A o O` back to INSERT, then all the motions and operators from the tables above, VISUAL mode, and the `:%s/find/new/` command line (find / replace only ever touches the field you are in, never the rest of the card; while you type it the matches are highlighted and, once you type the replacement, the field shows the result with the new text in green, with the field name and match count on the row above the buttons; the mode shows on the bottom edge). In NORMAL mode `j` / `k` on the last / first row of a field carry on to the next / previous field. With vim modes on, Esc in INSERT mode goes to NORMAL mode; Esc in NORMAL mode closes the window, and when there are unsaved changes it asks for a second Esc first.

Only fields that changed are sent. The alternate greetings are saved as one list (a greeting you empty is dropped). If a save fails, the window stays open with the error in the status line. Chats already started keep their own messages; new chats use the saved card. `/char` is short for `/char`, and `/chars` for `/chars` (`/switch` also opens the character menu).

## Small screens

The layout adapts to the terminal size, down to about 10 columns by 6 rows (below that it just says `too small`):

- Under 44 columns the buttons shrink to `Send` / `Stop` / `Save` / `Card`, and the top bar drops the API, then the persona, then the version number, to fit.
- Under 16 rows the two stacked buttons while editing a greeting become two one-row buttons, and the input box never grows so tall that fewer than two message rows are left.
- The help window, the `/char` window, the `/chars`, `/chats` and persona windows and `/char-img` are drawn between the top bar and the input box, with a margin of one column on each side (and one row above and below when there is room), so both stay visible. When the terminal is small they take that whole area; help lines wrap. Only on a terminal under 6 rows by 12 columns of such space do they take the whole screen.
- The character menu printed before the chat fits one terminal row per entry; the `/chars`, `/chats` and persona windows take the full width (less the margin) on small screens and cut long lines with `…` (narrow terminals show `msgs`).

## Nudging a regeneration

`/regen <text>` regenerates the last reply with a direction for the model, e.g. `/regen she hesitates, then refuses` or `/regen shorter, more dialogue`. The nudge is added to that one prompt as the very last system message, right before the character's turn, as `[Direction for <char>'s next reply: <text>]` (macros like `{{user}}` work in it). It is never saved to the chat, the reply replaces the old one and its swipes as usual, and a plain `/regen` or Ctrl+R has no nudge.

## Prose colours

At start-up tarpi asks the terminal for its foreground, background and yellow (`OSC 10`, `OSC 11`, `OSC 4;3`), so the colours follow whatever theme the terminal uses. Plain text keeps the terminal's default colour; `*narration*` is a bold neutral grey between the theme's foreground and background brightness (readable on light and dark themes); `"speech"` is the theme's yellow. Prose inside the other kind (`*he said "hi"*`, `"well *sigh* ok"`) is blended: the inner colour is pulled 40% of the way towards the outer one, so it fits in but is still a visible step away from both. Terminals that don't answer cost at most 300 ms once; then `COLORFGBG` (light / dark) and xterm-like colours are used.

## Summarizing a long chat

`/summarize` shrinks a chat that has grown big so that continuing is fast again. It reads the whole chat and writes an in-depth summary: who is who and how they relate, the events in order, every fact the story may depend on later (names, places, objects, injuries, promises, secrets, decisions, feelings), what was said that matters, the open plot threads, and, in the last paragraph, the situation right now.

The summary is not a swipe. It is a **summary branch** of the chat, a separate kind of view next to swipes and greetings, and every `/summarize` adds a new one (there is no limit):

- **branch 1** is the actual conversation, exactly as it was,
- **branch 2, 3, ...** are the summaries, oldest first. A summary branch shows just the summary as its first message (about **a quarter** of the tokens, -75%; aimed at 25% of the text's length, but never more than a quarter of the *Context size* of your API connection), and everything you say from there on continues from it.

`/summarize` summarizes the branch that is showing (the conversation, or an earlier summary together with what came after it) and adds the result as a new branch at the end, which then shows. Nothing is replaced.

The first message shows a cyan `summary branch 1/3` counter. Each branch keeps its own messages, and the model only sees the branch that is showing.

While it is being written nothing streams into the chat: the status row says `summarizing…` (with `part 2/4` for long chats), and when the summary is finished it appears whole, as the new branch. Esc stops it and then nothing is changed.

`/deletebranch` (or `/db`) deletes the branch that is showing and shows the one before it (the one after it when the first was deleted). With only one branch left the chat is a normal chat again; in a chat without branches it just says so. It also deletes the `_before-summary` copy `/summarize` saved (the newest one; all that are left when no branch remains), and it works on branch 1, the conversation, too. Deleting is permanent.

Switching branches (empty input box, not while editing): **Ctrl+Left** (previous) and **Ctrl+Right** (next), wrapping around, or tap / click the left / right half of the `summary branch n/m` counter. Plain Left / Right are not used for this; they keep browsing swipes and greetings, and on a lone summary they generate nothing.

Details:

- The summary is written with the active API connection (`/api`), with its own settings: the reply length is raised as needed and sampling is made calm (low temperature, no DRY / XTC / penalties / dynamic temperature, no extra stop strings). Your saved connection is not changed. The character card, system prompt and persona are not part of that request.
- A chat that does not fit into the context together with its summary is read in parts: each request gets the summary so far plus the next part of the chat. If the *Context size* is too small even for that, `/summarize` says so and does nothing.
- Before anything is changed, the full chat is also saved as `<chat>_before-summary` (then `...2`, `...3`). It is a normal chat file, but `/chats` does not list it, and `/deletebranch` removes it again. If that copy cannot be saved, the chat is left alone.
- The branches are stored in the chat file, inside the first message (`st_cli_branch`), so the file stays a normal `.jsonl`. The web UI shows the branch that was showing when you left.
- `Ctrl+S` / **Card save** is not offered on the summary: it is never written into the character card. Branched chats no longer get the card's alternate greetings added as swipes. `/undo` (with or without a number) will not delete the first message of a branched chat.
- It needs at least three messages. Summaries are written by the model, so read the result; branch 1 and the saved copy have everything.
- Chats summarized by an older version (summary as a second swipe of the first message) are left as they are and behave like a normal chat; run `/summarize` again to get branches. Chats with the earlier two-branch format load as they are.

## Placeholders and tidy text

`{{user}}`, `{{char}}` and the old `<USER>` / `<BOT>` / `<CHAR>` / `<GROUP>` are replaced in greetings (when a chat is created) and in every message on screen. When you send a message, macros such as `{{roll:1d6}}`, `{{random::a::b}}`, `{{time}}`, `{{date}}`, `{{time_UTC+2}}`, `{{newline}}`, `{{trim}}` and `{{//comment}}` are filled in like in SillyTavern, and unknown ones are left as typed. Messages you send or edit (and replies) are trimmed: no spaces or empty lines at the start or end, no trailing spaces on lines.

## Chats

Chats belong to tarpi too: `~/.config/tarpi/chats/<Name>/<chat>.jsonl`, one folder per character (named like the card file without `.png`). The files are the same SillyTavern `.jsonl` format as before (header line, then one message per line, unknown fields kept), so a chat folder copied from SillyTavern's `data/<user>/chats/` works as it is. Saving refuses to overwrite a file that another terminal changed in the meantime (`chat_metadata.integrity`), as SillyTavern did; reopen tarpi to reload it. `tarpi import-st <url>` copies the chats of every character of a server here (a chat that already exists here is kept; `--overwrite` replaces it). 

## Characters

Characters belong to tarpi, not to SillyTavern. Each card is a file in `~/.config/tarpi/characters/`:

- `Name.png`: a card PNG with the card embedded (the `chara` / `ccv3` text chunks SillyTavern, chub.ai and others use). The picture is what `/char-img` shows.
- `Name.json`: a card that is only JSON (V1, V2, V3 or Pygmalion). It has no picture. V1 and Pygmalion cards are read as V2; if you edit one with `/char` or Card save it is saved back as V2.

A character is identified by `Name.png` whichever of the two it is stored as; that name is also the folder its chats live in (see Chats). `/char` and Card save edit the card file right here (a PNG keeps its picture; only the embedded card data is rewritten). Cards you drop into the folder by hand are picked up the next time the list is read.

## Importing characters

```
tarpi import sofia.png                                    # V2/V3 card PNG
tarpi import sofia.json                                   # card JSON (V1, V2, V3, Pygmalion)
tarpi import '{"name": "Sofia", ...}'                     # JSON given directly
tarpi import https://chub.ai/characters/<creator>/<name>  # chub.ai (also characterhub.org)
tarpi import https://pygmalion.chat/character/<id>        # pygmalion.chat
tarpi import https://example.com/cards/sofia.png         # direct link to a card .png or .json
```

The card is saved as it came into `~/.config/tarpi/characters/`, named after the character (`Sofia.png`, or `Sofia-2.png` when that name is taken; a PNG stays a PNG, JSON stays JSON). Before saving, tarpi checks that the input really is a card (a PNG with an embedded `chara`/`ccv3` chunk, or JSON with `spec`, `name` or `char_name`) and gives a clear error if not. Any other http(s) link is treated as a direct link to a card file: tarpi downloads it itself (up to 50 MB; the file type is detected from its content, not the URL) and runs the same check. chub.ai (also characterhub.org) and pygmalion.chat links (`/character/<id>`) are downloaded by tarpi straight from those sites (chub.ai's download endpoint, pygmalion.chat's export endpoint); no other program is needed. Lorebook links are refused.

### Copying everything from SillyTavern

`tarpi import-st <url>` (see Personas) also copies every character of that server into the characters folder, as its card PNG with the same file name, and copies its chats into the chats folder (see Chats). A character that is already here is skipped (`--overwrite` replaces it).

## Deleting characters

```
tarpi delete           # pick a character from a list
tarpi delete sofia     # a partial name works
tarpi delete sofia -y  # no confirmation question (needed when not in a terminal)
```

It shows how many chats the character has and asks `Really delete? [y/N]`. Then the card file is deleted from the characters folder and the character's folder in `chats/` goes with it. This cannot be undone. Nothing on a SillyTavern server is touched.

## Phone / Termux (touch)

No setup needed. tarpi notices it runs on Android:

- **Tap a message** to edit it, like a click on desktop. Swipes scroll the chat.
- **Swipe gesture on Termux:** swipe sideways across the chat with a slight slant (a thumb's natural arc is enough; a perfectly flat swipe sends nothing at all). Left = next swipe, right = previous. One drag flips exactly one swipe. Only in touch mode and only when the last reply can be swiped. This works because Termux's scroll events carry the finger's column, so a sideways drag is recognised from them.
- **Tap the `‹2/3›` counter** in the last reply's header to change swipes: the left half goes to the previous one, the right half to the next (past the last generates a new one, like →). A horizontal drag on the chat does the same where the terminal reports drags (swipe left = next, right = previous), also with a mouse on desktop. ←/→ still work.
- **Tap the text box** to open the keyboard. For that moment tarpi stops asking the terminal for mouse events (like `vim.opt.mouse = ""` in Neovim), and it turns them back on by itself as soon as the keyboard is up. If the keyboard doesn't show on the first tap, tap the box once more.

Enter sends; for a new line use the Termux extra-keys row: ALT then Enter, or CTRL then J. ESC on that row leaves INSERT mode, as does typing the NORMAL-mode keys (`qq` by default) quickly.

## Build

(tarpi reads only `~/.config/tarpi/`. A folder left over from the old name, `~/.config/st-cli/`, is ignored; copy it to `~/.config/tarpi/` yourself to keep its characters, chats and settings.)

```
cargo build --release
cp target/release/tarpi ~/.local/bin/
```

Requires a recent stable Rust toolchain.

## Configuration

Settings live in `~/.config/tarpi/config.yaml`. The first run creates it with the defaults, fully commented; every key is optional. A `config.toml` with the same keys is still read when there is no `config.yaml`.

```yaml
# SillyTavern server: only used by `tarpi import-st` when no address is given.
# Chatting never contacts it.
# url: http://127.0.0.1:8000

# Rough chars-per-token used to trim old history to the context window
# chars_per_token: 3.2

# Auto-close quotes, *asterisks* and brackets while typing in the input box
# autopair: true

# The menu of /commands above the input box (see "Command menu")
# command_menu: true

# The Author's Note (see "Author's Note"); false turns it off
# author_note: true

# Spell checking in the input box (off by default)
# spellcheck: false

mouse:
  # Phones (Termux): tapping the input box opens the on-screen keyboard
  open_keyboard_on_tap: true
  # Clicking / tapping in the input box moves the text cursor there
  tap_to_move_cursor: true

vim:
  # true: vim-style INSERT / NORMAL / VISUAL modes in the input box
  # false: normal text editing, no modes
  enabled: true
  # Typed quickly in INSERT mode, these keys switch to NORMAL mode (2+ characters)
  normal_mode_keys: qq
  # <leader> key for NORMAL / VISUAL mappings: one character, or the word space
  leader: space
```

- `author_note: true` (default) keeps the Author's Note. `author_note: false` turns it off: `/note` is no longer offered in the menu and says it is off when typed, no note is added to any prompt (the notes already saved in your chat files stay there, untouched, and come back if you turn it on again), and a persona whose position is *Top / Bottom of the Author's Note* is sent in the prompt like the default position instead.
- `command_menu: true` (default) opens the `/command` menu above the input box while you type a slash (see *Command menu*). `command_menu: false` turns it off: nothing opens, Tab / Up / Down / Esc keep their old meaning (Up / Down scroll the chat), and Enter runs exactly what you typed. The command history (Up on a bare `/`) still works.
- `spellcheck: true` turns on offline spell checking in the chat input box and in the `/char` and persona windows (default `false`; the Name fields are not checked). Misspelled words get a red underline (the red of the terminal's theme, asked from the terminal at start-up like the prose colours; the text keeps its colour); words of one or two letters are checked too (`ho` and `x` are marked, `a`, `I`, `of`, `up` are not). The word you are still typing is not marked until the cursor leaves it. The word list is English and built into the program, so no internet or system dictionary is needed. Extra words go in `~/.config/tarpi/dictionary.txt` (one per line, `#` starts a comment); the character's name and your persona's name are always accepted. Details: `src/spell.rs`.
- With spell checking on, press **Tab** with the cursor in a marked word to open a **suggestion menu**: above the input box in the chat (like the command menu), and at the text cursor in the `/char` and persona windows. Misspelled words are only underlined (in the theme's red); the menu never opens by itself. It lists up to six spellings, best first (a swapped pair such as `teh` -> `the`, then a missing letter such as `helo` -> `hello`, then other close words), and keeps a capital first letter. It works in INSERT and NORMAL mode. Tab / Down go to the next entry, Shift+Tab / Up to the previous one, Enter puts the highlighted one in, Esc closes the menu, and a click or tap on an entry puts it in; any other key closes it. The Name fields of the windows are not checked.
- `autopair: true` (default) closes `"`, `'`, `*` and `(` `[` `{` as you type them, in INSERT mode and in the `/char` window: `"` gives `"|"`, `'` gives `'|'`, `*` gives `*|*`, a second `*` inside an empty pair gives `**|**`. Typing the closing character steps over the one already there, and Backspace between an empty pair removes both. It stays out of the way next to words (`2*3`, `5"`, and apostrophes like `don't` or `Sofia's`), when the paragraph already has an unclosed one (so the second `"` closes), and when the next character is a letter. Apostrophes inside a word are not counted as quotes, so `'I don't know'` types out as written. An apostrophe at the start of a word (`'em`, `'til`) opens a pair like any quote: Backspace removes it while it is still empty. Pasted text is never changed. Set `autopair: false` to turn it off.
- `vim.enabled: false` removes the modes and the mode tag under the box; the box is plain text editing. Left/Right arrows move the cursor once the box has a draft (on an empty box they switch swipes), and Home/End and Alt+Left/Right (word jump) move the cursor, and Up/Down move through a multi-line draft (with a one-line draft they scroll the chat as before).
- `vim.normal_mode_keys` can be any sequence of two or more characters, for example `jk`. It is typed quickly in INSERT mode (within about 0.6 s) and the keys are removed again.
- `vim.leader` is the `<leader>` key (default `space`; one character that is not a letter or digit, such as `,` or `\`, or the word `space`). See `<leader>y` below.
- `mouse.open_keyboard_on_tap: false` stops tarpi from switching mouse reporting off when you tap the box, so the keyboard does not open by itself on a phone. `mouse.tap_to_move_cursor: false` keeps taps from moving the cursor. The two are independent.

`tarpi import-st` talks to a SillyTavern server that needs no login (the default single-user setup); accounts and basic auth are not supported. It is the only command that contacts one.

## Keys

The row under the input box shows the totals on the right: the number of messages and the token count of the whole prompt, that is the system prompt, character card, persona, example dialogue, instruct formatting and every message, not trimmed to the context window (compare it with your context size to see when old messages start to drop out). The count is exact: the API connection's own tokenizer (llama.cpp's `/tokenize`) is asked in the background whenever the chat or your settings change. Until the answer arrives, or when the backend can't be reached or there is no connection, the number is an estimate (characters / `chars_per_token`) and shown as `~1.2k`.

| Key | Action |
| --- | --- |
| Shift+Enter (also Alt+Enter, Ctrl+J) | new line. Shift+Enter needs a terminal with the kitty keyboard protocol (kitty, WezTerm, foot, Ghostty, Alacritty, recent iTerm2); elsewhere use Alt+Enter or Ctrl+J |
| Enter, or click **Send** | send. An empty box adds no message; it just asks the character to reply (like the web UI) |
| Esc | stop generating (partial reply is kept) / cancel an edit |
| Ctrl+K, `/continue` or `/c` | continue the last reply (appends to it) |
| Click in the input box | move the cursor there (also while editing a message) |
| Click a message | edit that message in the input box, in INSERT mode (Enter / **Save** saves, Shift+Enter adds a line, Esc cancels; the cursor starts at the end of the text; your unsent draft comes back) |
| Ctrl+S, `:w` (vim modes on), or the **Card save** button | only while editing the character's greeting (the very first message, at a swipe that came from the card): also writes the edit into the character card itself, as `first_mes` or the matching alternate greeting, so it is permanent and new chats get it. **Save** / Enter only changes this chat |
| Ctrl+R or `/regen [nudge]` | regenerate the last reply (`/regen make her more guarded` steers just this reply, see below): the new reply replaces it and wipes all its swipes (it ends up with a single one). Swipes are only added by actually swiping: ←/→ past the last swipe, a tap on the `‹n/m›` counter, or a sideways drag |
| Ctrl+P | edit the character's very last reply (opens in the input box like a click on it; Enter saves, Esc cancels) |
| Ctrl+O | edit your own very last message the same way |
| ←/→ | previous / next swipe (→ past the last generates a new one). Only while the input box is empty and no message is being edited; with a draft in the box they move the text cursor. In a new chat, while the greeting is the only message, they switch between the card's alternate greetings instead (wrapping around). Older chats get the card's alternate greetings too |
| PgUp/PgDn, mouse wheel, ↑/↓ | scroll the chat (↑/↓ move the cursor while editing). The wheel (or a swipe) over the text box scrolls only the box, anywhere else only the chat; typing or moving the cursor snaps the box back to the cursor |
| `/undo [n]` or `/u [n]` | delete the last message, or the last `n` (`/undo 3`); stops at the first message of a branched chat |
| `/new` | new chat with this character (keeps the current one) |
| `/newchat` or `/nc` | delete this chat and start a new one |
| F1 or `/help` | help window; scroll it with ↑/↓ (or j/k), PgUp/PgDn, Home/End or the mouse wheel, any other key closes it |
| Ctrl+C / Ctrl+D | quit |

### Vim motions in the input box

The box starts in INSERT mode, also when you open a message to edit. Type `qq` (configurable, or turn vim mode off in `config.yaml`) quickly to go to NORMAL mode (the mode shows under the box). Enter sends in INSERT and NORMAL mode; Shift+Enter adds a new line in INSERT mode.

| Keys | Action |
| --- | --- |
| `i` `a` `I` `A` `o` `O` | back to INSERT (at / after cursor, line start / end, new line below / above) |
| `h` `j` `k` `l` `w` `W` `b` `B` `e` `E` `0` `^` `$` `gg` `G` | move (counts work: `3w`; capitals are WORD motions) |
| `x` `X` `r<c>` | delete char / char before / replace char |
| `dd` `cc` `yy` `D` `C` `S` `s` | line and to-end-of-line operations |
| `d` `c` `y` + `w` `W` `e` `E` `b` `B` `$` `0` `^` `h` `l` | operator + motion (`dw`, `cW`, `y$`, ...) |
| `Y` | yank the line (same as `yy`) |
| `<leader>y` + motion, `<leader>yy`, `<leader>Y` | the same yanks, but the text also goes to the **system clipboard** (a count goes before the leader: `3<leader>yy`) |
| `v` `V` | VISUAL mode (characters / whole lines) |
| `p` `P` | paste after / before |
| `u` | undo; a count goes back that many steps (`3u`) |

`y` only fills vim's own register (for `p` / `P`). `<leader>y` also sends the text to the system clipboard through whatever the machine has (`wl-copy`, `xclip`, `xsel`, `pbcopy`, `termux-clipboard-set`, `clip.exe`) and falls back to the terminal's OSC 52 escape, which also works over SSH (in tmux: `set -g set-clipboard on`). The status line says what was copied and how. A linewise yank (`yy`, `V`) ends in a newline, like vim's `"+yy`.

**Undo steps.** Everything typed in one INSERT session is one step (like in vim), and every NORMAL-mode change (`x`, `dd`, `p`, `:s`, ...) is one. These are steps of their own, so `u` takes back just that and nothing else: picking a spelling suggestion (Tab menu), pasting text, emptying the box with Ctrl+U, and Backspace / Delete / Ctrl+W / Ctrl+J typed in NORMAL mode. Text typed after one of them is the next step. `u` skips steps that changed nothing and does nothing once the history is used up. The history is cleared when the box is emptied by sending a message or when a different message is opened for editing; in the `/char` and persona windows every field keeps its own history while you move between fields.

In VISUAL mode every motion above extends the selection (shown in reverse video). Then:

| Keys | Action |
| --- | --- |
| `d` `x` | delete the selection (`D` `X`: whole lines) |
| `y` `Y` | yank the selection (`Y`: whole lines) |
| `<leader>y` `<leader>Y` | same, and copy to the system clipboard |
| `c` `s` | change the selection (`C` `S` `R`: whole lines) |
| `p` `P` | replace the selection with the register (`p` keeps the replaced text in the register) |
| `o` | jump to the other end of the selection |
| `v` `V` | switch between character / line selection; pressing the active one leaves VISUAL |
| Esc | leave VISUAL mode (Esc again cancels an edit, as before) |

### Command line (`:`) and find / replace

`:` in NORMAL mode opens a vim-style command line on the bottom row (the box shows `COMMAND`). In VISUAL mode it starts with `'<,'>`, the selected lines. Enter runs it, Esc (or Backspace on an empty line) leaves, Ctrl+U clears it, Ctrl+W deletes a word.

| Command | Action |
| --- | --- |
| `:%s/find/new/` | replace the first `find` on every line |
| `:%s/find/new/g` | replace every `find` |
| `:s/find/new/` | the current line only |
| `:3,5s/...` `:.,$s/...` `:'<,'>s/...` | line ranges (`N`, `.` current line, `$` last line, `%` everything) |
| `:5` | go to line 5 |

Flags: `g` all matches on a line, `i` ignore case, `I` match case (default). Any character can be the delimiter (`:%s#a/b#c#`); `\/` is a literal `/`. In the replacement, `&` is the found text (`\&` a literal `&`) and `\n` a line break. A missing replacement deletes the match (`:%s/foo`). `u` undoes a whole `:s`.

While you type the pattern, every match that would be replaced is highlighted in the input box (black on yellow) and the bottom row shows how many there are (`no match` in red). As soon as you type the second delimiter (`:%s/foo/`), the box shows the text as it would look after the replacement, with the new text in green, and updates on every key. Nothing is really changed until you press Enter; Esc puts the original text back. Afterwards the status line says `N substitutions on M lines`, or `E486: Pattern not found`.

The pattern is matched as plain text (no regular expressions yet), so `.` and `*` mean themselves.

## How it works

Everything tarpi needs is its own: cards in `~/.config/tarpi/characters/`, personas in `persona/`, system prompts in `system-prompt.jsonl`, formatting in `formatting.yaml`, connections in `api.yaml` and chats in `chats/`. Before every reply tarpi re-reads those files, so a change to a template, persona, system prompt or sampler value applies to the next message. The prompt is built client side and the reply comes straight from your API connection (`/api`).

1. Settings are built in memory from the files above; no server is asked.
2. Character card and chat are read from, and saved to, tarpi's own folders.
3. `src/textgen.rs` renders your context template (story string), instruct template, example dialogue and stop strings. `src/conn/` sends that prompt with the connection's own sampler values to the backend (llama.cpp's `/completion`).
4. The stream is rendered live and the chat is saved back to the same `.jsonl` file.

## Limitations

- Only llama.cpp is supported, through a standalone connection (`/api`). No other server is contacted.
- No World Info/lorebooks, summaries, vector memory, group chats, or tool calling. The prompt is therefore close to, but not byte-identical to, the web UI's when you use those.
- Token counting for trimming history to the context window is an estimate (`chars_per_token`), not a real tokenizer. The totals in the status row are exact once the connection's tokenizer has answered (see Keys).
- Unfinished prose is closed for you while a reply streams: an open `*narration`, `**bold` or `"speech` is shown with its closing marker added, and the model's own closer simply takes its place. Nothing is added for a lone trailing `*`, `2 * 3`, bullets, a `*` inside a word, or past a blank line. When a reply ends with markers still open they are closed in the saved text too (Ctrl+K takes them off again before continuing).
- Markdown is minimal: `*narration*` is bold and a neutral grey, `**bold**` is bold and "quoted speech" is yellow. Styles end at a blank line, and a `*` that has no partner (`2 * 3`, a `* ` bullet, an unclosed one) is shown as a plain asterisk.
- The prose colours come from your terminal's theme (see *Prose colours*).
- Mouse capture is on (for the Send button and click-to-edit); hold Shift to select text with the terminal's own selection.
