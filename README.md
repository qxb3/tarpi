# st-cli

A chat-only terminal client for a running [SillyTavern](https://github.com/SillyTavern/SillyTavern) server.

SillyTavern keeps doing everything else (characters, API keys, connections, presets, personas). `st-cli` only chats, using whatever is currently configured in the web UI, and saves chats to the same `.jsonl` files, so you can switch between the TUI and the web UI at any time.

```
st-cli                  # pick a character, then chat
st-cli sofia            # continue the latest chat with Sofia
st-cli sofia 3          # open Sofia's chat number 3 (or by name: Sofia03)
st-cli sofia -n         # start a new chat (named Sofia01, Sofia02, ...)
st-cli -l               # list characters
st-cli sofia -l         # list Sofia's chats
st-cli import <source>  # import a character card (see below)
st-cli delete [sofia]   # delete a character and all its chats (asks first)
```

That is the whole command line: `[character] [chat]`, plus `-n` (new chat) and `-l` (list). The server address is set with `url` in `~/.config/st-cli/config.yaml`.

Everything else happens inside the chat:

| Command | What it does |
| --- | --- |
| `/chats` | opens a window over the chat with this character's chats: pick one (↑/↓ + Enter, or click / tap), choose `+ new chat`, or press `d` on one to delete it (asks `y`). Esc closes. The `_before-summary` copies are not listed. The chat that is open can't be deleted here (use `/newchat`), except when it is the last one: then it is deleted and st-cli exits |
| `/one-shot` | locks **this chat file** as it is: editing, `/undo`, regenerating, `/continue`, swipes (and greeting switching) and summary branches are turned off. The status row asks you to type `/one-shot` a second time; typing anything else cancels. A grey `ONE SHOT MODE` tag stays at the start of the status row below the input box for as long as the chat is open; other status messages follow it after a `·`. A reply that is being written can't be stopped either (no Esc, no Stop button, and `/quit`, `/chats` and `/chars` wait until it is done). Sending new messages still works, and other chats are not affected. It is stored in the chat file's metadata (`st_cli_one_shot`); there is no command to undo it |
| `/persona` | opens a window to choose who you are in the chat; `+ new persona` creates one, `e` edits, `d` deletes (see Personas) |
| `/chars` | opens a window with every character (★ = favourite) to switch to someone else |
| `/api` | open the API settings (llama.cpp address and every model setting) right here; when you quit them (`q` / Esc) you are back in this same chat, and the very next reply uses what you saved |
| `/char` | open this character's card in a window: every field, editable, with **Cancel** and **Save** (see below) |
| `/char-img` | shows the character card's picture in a square window (full resolution in terminals with the Kitty graphics protocol, iTerm2 images or Sixel (kitty, WezTerm, Ghostty, iTerm2, foot, Windows Terminal, mlterm); elsewhere, including Termux and tmux, it is drawn with coloured half-block cells, which is coarse. `ST_CLI_IMAGE=blocks|kitty|iterm|sixel` forces one (`sixel` is never auto-detected beyond the terminals listed); any key or click closes it) |
| `/new` | new chat with this character right away (keeps the current one) |
| `/newchat` or `/nc` | delete this chat and start a new one |
| `/summarize` or `/summ` | write an in-depth summary of the chat as a new summary branch (see below) |
| `/deletebranch` or `/db` | delete the summary branch that is showing (see *Summarizing a long chat*) |

### Command menu

Typing `/` in the input box opens a menu of commands just above it, narrowing as you type (`/ch` lists `/chats`, `/char`, `/chars`, `/char-img`). The top entry is highlighted from the start. It shows while the box holds a single `/word` in INSERT mode (and not while a message is being edited); once there is a space after the command (`/regen softer`) it closes.

| Key / click | Action |
| --- | --- |
| Tab or Down | move the highlight to the next command and fill it in (the top one is highlighted from the start, so the first press already goes to the second) |
| Shift+Tab or Up | move the highlight to the previous command and fill it in; wraps around (from the top one to the last) |
| Esc | put back what you had typed before Tab started filling commands in |
| click / tap | fill in that command and close the menu |
| Enter | run the highlighted command when what you typed is only the start of one (`/su` runs `/summarize`); a full name or short form (`/c`, `/nc`) runs as typed |

The menu lists the short names `/char` and `/chars` and the short forms `/summ`, `/nc`, `/db`, `/c`, `/u`, `/q` and `/h` (shown in brackets). The longer spellings `/character`, `/characters` and `/summarise` still work when typed in full, but are not offered. While the menu is open Up / Down move through it instead of scrolling the chat.

`<character>` is matched case-insensitively against the name or card file name; a unique partial match works. New chats are numbered per character (`Sofia01`, `Sofia02`, ...). A chat can be given as a number, an exact name, or a unique partial name.

## API connection (`/api`)

The API connection belongs to st-cli, not to SillyTavern. `/api` inside a chat opens a full-screen manager; connections are stored in `~/.config/st-cli/api.yaml`. Replies go **straight to the backend** (llama.cpp's `/completion`), SillyTavern's API selection and sampler presets are ignored, and the token count in the status row comes from the backend's own `/tokenize`. SillyTavern is still used for characters, chats, personas and the context / instruct templates. There is no fallback to SillyTavern's API: if `api.yaml` has no active connection, sending a message shows `no API connection` and nothing is generated.

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

Chats re-read `api.yaml` before every reply, so saving in `/api` (even from another terminal's chat) applies to the next message. *Context size* is how much history st-cli sends; keep it at or below the server's `-c`. `stop strings`, `DRY breakers` and `Sampler order` are text fields (the first two a JSON list); a value that is not valid is refused when you press Enter.

Only llama.cpp is built in, but a connection kind is one small file: implement `conn::Provider` (its settings list, request body and stream parser; see `src/conn/llamacpp.rs`) and add it to `conn::providers()`. The manager, the file format, token counting and generation pick it up without further changes.

## Personas

`/persona` inside a chat opens a window with every persona saved in SillyTavern as `Name (Title)` with a short preview of its description; the active one is marked `(current)`. Pick one with ↑/↓ + Enter (or click / tap); Esc closes it. The choice is written to SillyTavern's settings (`user_avatar`, name, description, position), the same fields the web UI changes when you click a persona, so st-cli and the web UI both use it from then on. It applies to the chat from the next message on. The first row, `+ new persona`, opens a form (name, title, description; Ctrl+S saves, Esc cancels) and makes the persona in SillyTavern with a plain placeholder picture you can change in the web UI. `e` on a persona opens the same form for it; `d` deletes it (asks `y`, removes its picture like the web UI does; the persona in use can't be deleted). A web UI tab that is already open needs a reload to show the change.

The chat's top bar reads ` st-cli <version> ` (white badge) `<character>/<chat file> • <persona> (<title>) • <api>@host:port`, e.g. `Mirei/Mire01`. The API type is what SillyTavern is set to; `host:port` is the `url` from `config.yaml`. Message names are black on white, with a grey rule between messages (none under the last one).

## Card save

While you edit the chat's first message, Ctrl+S (or the **Card save** button) writes the edit into the character card itself. st-cli works out which greeting you are looking at by matching its text against the card's first message and alternate greetings, so it updates the right one (`first_mes` or the matching alternate greeting) even when the swipes are in a different order. The status line says which one was saved. `{{char}}` / `{{user}}` (or `<BOT>` / `<USER>`) are put back in the card text if the original greeting used them.

## Character window

`/char` opens the whole card of the current character in a window, like `/help`: name, card file, description, personality, scenario, first message, example dialogue, system prompt, post-history instructions, every alternate greeting and the depth prompt. The text is the raw card text (`{{char}}` / `{{user}}` stay as they are).

All fields except the name, the card file and the depth prompt (shown read-only) can be edited. The field with the cursor has a cyan title; the window scrolls to follow it. It opens with the cursor at the very top of the first editable field (Description).

| Key / click | Action |
| --- | --- |
| Tab / Shift+Tab | next / previous field |
| Up / Down | move a row; past the first / last row of a field they go on to the previous / next field |
| Enter | new line inside the field |
| Left, Right, Home, End, Ctrl+A / Ctrl+E, Backspace, Delete, PgUp / PgDn | as in a text box |
| Ctrl+U / Ctrl+D (NORMAL / VISUAL mode) | half a window up / down; on reaching the edge of a field, move into the previous / next field |
| click | put the cursor in that field |
| mouse wheel | scroll the window |
| Ctrl+S, or the **Save** button | write the changed fields to the character card on the server (`/api/characters/edit-attribute`, as Card save does) and close |
| Esc (see above), or the **Cancel** button | close without saving |

The focused field is edited with the same editor as the chat box, so the vim modes work there too (unless `vim.enabled: false`): `qq` typed quickly or Esc goes to NORMAL mode, `i a I A o O` back to INSERT, then all the motions and operators from the tables above, VISUAL mode, and the `:%s/find/new/` command line (find / replace only ever touches the field you are in, never the rest of the card; while you type it the matches are highlighted and, once you type the replacement, the field shows the result with the new text in green, with the field name and match count on the row above the buttons; the mode shows on the bottom edge). In NORMAL mode `j` / `k` on the last / first row of a field carry on to the next / previous field. With vim modes on, Esc in INSERT mode goes to NORMAL mode; Esc in NORMAL mode closes the window, and when there are unsaved changes it asks for a second Esc first.

Only fields that changed are sent. The alternate greetings are saved as one list (a greeting you empty is dropped). If a save fails, the window stays open with the error in the status line. Chats already started keep their own messages; new chats use the saved card. `/char` is short for `/char`, and `/chars` for `/chars` (`/switch` also opens the character menu).

## Small screens

The layout adapts to the terminal size, down to about 10 columns by 6 rows (below that it just says `too small`):

- Under 44 columns the buttons shrink to `Send` / `Stop` / `Save` / `Card`, and the top bar drops the API, then the persona, then the version number, to fit.
- Under 16 rows the two stacked buttons while editing a greeting become two one-row buttons, and the input box never grows so tall that fewer than two message rows are left.
- The help window and the `/char` window take the whole screen when it is small; help lines wrap.
- The character menu printed before the chat fits one terminal row per entry; the `/chars`, `/chats` and persona windows take the full width on small screens and cut long lines with `…` (narrow terminals show `msgs`).

## Nudging a regeneration

`/regen <text>` regenerates the last reply with a direction for the model, e.g. `/regen she hesitates, then refuses` or `/regen shorter, more dialogue`. The nudge is added to that one prompt as the very last system message, right before the character's turn, as `[Direction for <char>'s next reply: <text>]` (macros like `{{user}}` work in it). It is never saved to the chat, the reply replaces the old one and its swipes as usual, and a plain `/regen` or Ctrl+R has no nudge.

## Prose colours

At start-up st-cli asks the terminal for its foreground, background and yellow (`OSC 10`, `OSC 11`, `OSC 4;3`), so the colours follow whatever theme the terminal uses. Plain text keeps the terminal's default colour; `*narration*` is a bold neutral grey between the theme's foreground and background brightness (readable on light and dark themes); `"speech"` is the theme's yellow. Prose inside the other kind (`*he said "hi"*`, `"well *sigh* ok"`) is blended: the inner colour is pulled 40% of the way towards the outer one, so it fits in but is still a visible step away from both. Terminals that don't answer cost at most 300 ms once; then `COLORFGBG` (light / dark) and xterm-like colours are used.

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
- Before anything is changed, the full chat is also saved as `<chat>_before-summary` (then `...2`, `...3`). It is a normal chat file on the server, but `/chats` does not list it, and `/deletebranch` removes it again. If that copy cannot be saved, the chat is left alone.
- The branches are stored in the chat file, inside the first message (`st_cli_branch`), so the file stays a normal `.jsonl`. The web UI shows the branch that was showing when you left.
- `Ctrl+S` / **Card save** is not offered on the summary: it is never written into the character card. Branched chats no longer get the card's alternate greetings added as swipes. `/undo` (with or without a number) will not delete the first message of a branched chat.
- It needs at least three messages. Summaries are written by the model, so read the result; branch 1 and the saved copy have everything.
- Chats summarized by an older version (summary as a second swipe of the first message) are left as they are and behave like a normal chat; run `/summarize` again to get branches. Chats with the earlier two-branch format load as they are.

## Placeholders and tidy text

`{{user}}`, `{{char}}` and the old `<USER>` / `<BOT>` / `<CHAR>` / `<GROUP>` are replaced in greetings (when a chat is created) and in every message on screen. When you send a message, macros such as `{{roll:1d6}}`, `{{random::a::b}}`, `{{time}}`, `{{date}}`, `{{time_UTC+2}}`, `{{newline}}`, `{{trim}}` and `{{//comment}}` are filled in like in SillyTavern, and unknown ones are left as typed. Messages you send or edit (and replies) are trimmed: no spaces or empty lines at the start or end, no trailing spaces on lines.

## Importing characters

```
st-cli import sofia.png                                    # V2/V3 card PNG
st-cli import sofia.json                                   # card JSON (V1, V2, V3, Pygmalion)
st-cli import '{"name": "Sofia", ...}'                     # JSON given directly
st-cli import https://chub.ai/characters/<creator>/<name>  # chub.ai (also characterhub.org)
st-cli import https://pygmalion.chat/character/<id>        # pygmalion.chat
st-cli import https://example.com/cards/sofia.png         # direct link to a card .png or .json
```

st-cli does not convert anything itself. The card is sent to SillyTavern's own import endpoint (`/api/characters/import`, what the web UI's import button uses), so the server reads V1/V2/V3/Pygmalion JSON and PNG cards and saves it the same way. URLs are downloaded by the server too (`/api/content/importURL`). Before sending, st-cli checks that the input really is a card (a PNG with an embedded `chara`/`ccv3` chunk, or JSON with `spec`, `name` or `char_name`) and gives a clear error if not. Any other http(s) link is treated as a direct link to a card file: st-cli downloads it itself (up to 50 MB; the file type is detected from its content, not the URL), runs the same check, and uploads it to SillyTavern. Supported card sites: chub.ai (also characterhub.org) and pygmalion.chat (`/char/<id>` links; the server fetches the card by its id); lorebook links are refused.

The server comes from `url` in `config.yaml`, as for chatting.

## Deleting characters

```
st-cli delete           # pick a character from a list
st-cli delete sofia     # a partial name works
st-cli delete sofia -y  # no confirmation question (needed when not in a terminal)
```

It shows how many chats the character has and asks `Really delete? [y/N]`. Then SillyTavern's own delete call (`/api/characters/delete` with `delete_chats`) removes the card and every chat file. This cannot be undone.

## Phone / Termux (touch)

No setup needed. st-cli notices it runs on Android:

- **Tap a message** to edit it, like a click on desktop. Swipes scroll the chat.
- **Swipe gesture on Termux:** swipe sideways across the chat with a slight slant (a thumb's natural arc is enough; a perfectly flat swipe sends nothing at all). Left = next swipe, right = previous. One drag flips exactly one swipe. Only in touch mode and only when the last reply can be swiped. This works because Termux's scroll events carry the finger's column, so a sideways drag is recognised from them.
- **Tap the `‹2/3›` counter** in the last reply's header to change swipes: the left half goes to the previous one, the right half to the next (past the last generates a new one, like →). A horizontal drag on the chat does the same where the terminal reports drags (swipe left = next, right = previous), also with a mouse on desktop. ←/→ still work.
- **Tap the text box** to open the keyboard. For that moment st-cli stops asking the terminal for mouse events (like `vim.opt.mouse = ""` in Neovim), and it turns them back on by itself as soon as the keyboard is up. If the keyboard doesn't show on the first tap, tap the box once more.

Enter sends; for a new line use the Termux extra-keys row: ALT then Enter, or CTRL then J. ESC on that row leaves INSERT mode, as does typing the NORMAL-mode keys (`qq` by default) quickly.

## Build

```
cargo build --release
cp target/release/st-cli ~/.local/bin/
```

Requires a recent stable Rust toolchain.

## Configuration

Settings live in `~/.config/st-cli/config.yaml`. The first run creates it with the defaults, fully commented; every key is optional. A `config.toml` with the same keys is still read when there is no `config.yaml`.

```yaml
# SillyTavern server (default http://127.0.0.1:8000)
# url: http://127.0.0.1:8000

# Rough chars-per-token used to trim old history to the context window
# chars_per_token: 3.2

# Auto-close quotes, *asterisks* and brackets while typing in the input box
# autopair: true

# The menu of /commands above the input box (see "Command menu")
# command_menu: true

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

- `command_menu: true` (default) opens the `/command` menu above the input box while you type a slash (see *Command menu*). `command_menu: false` turns it off: nothing opens, Tab / Up / Down / Esc keep their old meaning (Up / Down scroll the chat), and Enter runs exactly what you typed.
- `autopair: true` (default) closes `"`, `'`, `*` and `(` `[` `{` as you type them, in INSERT mode and in the `/char` window: `"` gives `"|"`, `'` gives `'|'`, `*` gives `*|*`, a second `*` inside an empty pair gives `**|**`. Typing the closing character steps over the one already there, and Backspace between an empty pair removes both. It stays out of the way next to words (`2*3`, `5"`, and apostrophes like `don't` or `Sofia's`), when the paragraph already has an unclosed one (so the second `"` closes), and when the next character is a letter. Apostrophes inside a word are not counted as quotes, so `'I don't know'` types out as written. An apostrophe at the start of a word (`'em`, `'til`) opens a pair like any quote: Backspace removes it while it is still empty. Pasted text is never changed. Set `autopair: false` to turn it off.
- `vim.enabled: false` removes the modes and the mode tag under the box; the box is plain text editing. Left/Right arrows move the cursor once the box has a draft (on an empty box they switch swipes), and Home/End and Alt+Left/Right (word jump) move the cursor, and Up/Down move through a multi-line draft (with a one-line draft they scroll the chat as before).
- `vim.normal_mode_keys` can be any sequence of two or more characters, for example `jk`. It is typed quickly in INSERT mode (within about 0.6 s) and the keys are removed again.
- `vim.leader` is the `<leader>` key (default `space`; one character that is not a letter or digit, such as `,` or `\`, or the word `space`). See `<leader>y` below.
- `mouse.open_keyboard_on_tap: false` stops st-cli from switching mouse reporting off when you tap the box, so the keyboard does not open by itself on a phone. `mouse.tap_to_move_cursor: false` keeps taps from moving the cursor. The two are independent.

st-cli talks to a SillyTavern server that needs no login (the default single-user setup). Accounts and basic auth are not supported.

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
| Ctrl+S, or the **Card save** button | only while editing the character's greeting (the very first message, at a swipe that came from the card): also writes the edit into the character card itself, as `first_mes` or the matching alternate greeting, so it is permanent and new chats get it. **Save** / Enter only changes this chat |
| Ctrl+R or `/regen [nudge]` | regenerate the last reply (`/regen make her more guarded` steers just this reply, see below): the new reply replaces it and wipes all its swipes (it ends up with a single one). Swipes are only added by actually swiping: ←/→ past the last swipe, a tap on the `‹n/m›` counter, or a sideways drag |
| Ctrl+P | edit the character's very last reply (opens in the input box like a click on it; Enter saves, Esc cancels) |
| Ctrl+O | edit your own very last message the same way |
| ←/→ | previous / next swipe (→ past the last generates a new one). Only while the input box is empty and no message is being edited; with a draft in the box they move the text cursor. In a new chat, while the greeting is the only message, they switch between the card's alternate greetings instead (wrapping around). Older chats get the card's alternate greetings too |
| PgUp/PgDn, mouse wheel, ↑/↓ | scroll the chat (↑/↓ move the cursor while editing). The wheel (or a swipe) over the text box scrolls only the box, anywhere else only the chat; typing or moving the cursor snaps the box back to the cursor |
| `/undo [n]` or `/u [n]` | delete the last message, or the last `n` (`/undo 3`); stops at the first message of a branched chat |
| `/new` | new chat with this character (keeps the current one) |
| `/newchat` or `/nc` | delete this chat on the server and start a new one |
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
| `p` `P` `u` | paste after / before, undo |

`y` only fills vim's own register (for `p` / `P`). `<leader>y` also sends the text to the system clipboard through whatever the machine has (`wl-copy`, `xclip`, `xsel`, `pbcopy`, `termux-clipboard-set`, `clip.exe`) and falls back to the terminal's OSC 52 escape, which also works over SSH (in tmux: `set -g set-clipboard on`). The status line says what was copied and how. A linewise yank (`yy`, `V`) ends in a newline, like vim's `"+yy`.

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

SillyTavern only supplies characters, chats, personas and the context / instruct templates; the prompt is built client side and the reply comes straight from your API connection (`/api`). Before every reply st-cli re-reads both SillyTavern's settings and `api.yaml`, so a change to a template, persona or sampler value applies to the next message.

1. `GET /csrf-token` (+ optional login), then `POST /api/settings/get`.
2. Character card from `/api/characters/get`; chats via `/api/chats/get` and `/api/chats/save`.
3. `src/textgen.rs` renders your context template (story string), instruct template, example dialogue and stop strings. `src/conn/` sends that prompt with the connection's own sampler values to the backend (llama.cpp's `/completion`).
4. The stream is rendered live and the chat is saved back to the same `.jsonl` file.

## Limitations

- Only llama.cpp is supported, through a standalone connection (`/api`). SillyTavern's own API selection is never used for replies or token counts.
- No World Info/lorebooks, Author's Note, summaries, vector memory, group chats, or tool calling. The prompt is therefore close to, but not byte-identical to, the web UI's when you use those.
- Token counting for trimming history to the context window is an estimate (`chars_per_token`), not a real tokenizer. The totals in the status row are exact once the connection's tokenizer has answered (see Keys).
- Unfinished prose is closed for you while a reply streams: an open `*narration`, `**bold` or `"speech` is shown with its closing marker added, and the model's own closer simply takes its place. Nothing is added for a lone trailing `*`, `2 * 3`, bullets, a `*` inside a word, or past a blank line. When a reply ends with markers still open they are closed in the saved text too (Ctrl+K takes them off again before continuing).
- Markdown is minimal: `*narration*` is bold and a neutral grey, `**bold**` is bold and "quoted speech" is yellow. Styles end at a blank line, and a `*` that has no partner (`2 * 3`, a `* ` bullet, an unclosed one) is shown as a plain asterisk.
- The prose colours come from your terminal's theme (see *Prose colours*).
- Mouse capture is on (for the Send button and click-to-edit); hold Shift to select text with the terminal's own selection.
