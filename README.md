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
st-cli persona          # pick which persona (who you are) to chat as
```

That is the whole command line: `[character] [chat]`, plus `-n` (new chat) and `-l` (list). The server address is set with `url` in `~/.config/st-cli/config.yaml`.

Everything else happens inside the chat:

| Command | What it does |
| --- | --- |
| `/chats` | back to this character's chat menu: open one (`2`), start a new one (`n`), or delete one (`d2`) |
| `/characters` | back to the character menu to switch to someone else |
| `/character` | open this character's card in a window: every field, editable, with **Cancel** and **Save** (see below) |
| `/new` | new chat with this character right away (keeps the current one) |
| `/newchat` or `/nc` | delete this chat and start a new one |

`<character>` is matched case-insensitively against the name or card file name; a unique partial match works. New chats are numbered per character (`Sofia01`, `Sofia02`, ...). A chat can be given as a number, an exact name, or a unique partial name.

## Personas

```
st-cli persona          # menu: pick one, it is saved and the command exits
st-cli persona alice    # switch straight to the persona matching "alice"
st-cli persona -l       # list personas and exit
```

The menu lists every persona saved in SillyTavern as `Name (Title)` with a short preview of its description; the active one is marked `(current)`. Typing part of a name narrows the list. The choice is written to SillyTavern's settings (`user_avatar`, name, description, position), the same fields the web UI changes when you click a persona, so st-cli and the web UI both use it from then on. A web UI tab that is already open needs a reload to show the change.

The chat's top bar reads ` st-cli <version> ` (white badge) `<character>/<chat file> • <persona> (<title>) • <api>@host:port`, e.g. `Mirei/Mire01`. The API type is what SillyTavern is set to; `host:port` is the `url` from `config.yaml`. Message names are black on white, with a grey rule between messages (none under the last one).

## Card save

While you edit the chat's first message, Ctrl+S (or the **Card save** button) writes the edit into the character card itself. st-cli works out which greeting you are looking at by matching its text against the card's first message and alternate greetings, so it updates the right one (`first_mes` or the matching alternate greeting) even when the swipes are in a different order. The status line says which one was saved. `{{char}}` / `{{user}}` (or `<BOT>` / `<USER>`) are put back in the card text if the original greeting used them.

## Character window

`/character` opens the whole card of the current character in a window, like `/help`: name, card file, description, personality, scenario, first message, example dialogue, system prompt, post-history instructions, every alternate greeting and the depth prompt. The text is the raw card text (`{{char}}` / `{{user}}` stay as they are).

All fields except the name, the card file and the depth prompt (shown read-only) can be edited. The field with the cursor has a cyan title; the window scrolls to follow it.

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

Only fields that changed are sent. The alternate greetings are saved as one list (a greeting you empty is dropped). If a save fails, the window stays open with the error in the status line. Chats already started keep their own messages; new chats use the saved card. Note that `/character` used to be an alias of `/characters`; `/characters`, `/chars`, `/char` and `/switch` still open the character menu.

## Small screens

The layout adapts to the terminal size, down to about 10 columns by 6 rows (below that it just says `too small`):

- Under 44 columns the buttons shrink to `Send` / `Stop` / `Save` / `Card`, and the top bar drops the API, then the persona, then the version number, to fit.
- Under 16 rows the two stacked buttons while editing a greeting become two one-row buttons, and the input box never grows so tall that fewer than two message rows are left.
- The help window and the `/character` window take the whole screen when it is small; help lines wrap.
- The menus printed before the chat (characters, chats, personas) fit one terminal row per entry where they can; narrow terminals show `msgs` and leave out the file size.

## Placeholders and tidy text

`{{user}}`, `{{char}}` and the old `<USER>` / `<BOT>` / `<CHAR>` / `<GROUP>` are replaced in greetings (when a chat is created) and in every message on screen. When you send a message, macros such as `{{roll:1d6}}`, `{{random::a::b}}`, `{{time}}`, `{{date}}`, `{{time_UTC+2}}`, `{{newline}}`, `{{trim}}` and `{{//comment}}` are filled in like in SillyTavern, and unknown ones are left as typed. Messages you send or edit (and replies) are trimmed: no spaces or empty lines at the start or end, no trailing spaces on lines.

## Importing characters

```
st-cli import sofia.png                                    # V2/V3 card PNG
st-cli import sofia.json                                   # card JSON (V1, V2, V3, Pygmalion)
st-cli import '{"name": "Sofia", ...}'                     # JSON given directly
st-cli import https://chub.ai/characters/<creator>/<name>  # chub.ai (also characterhub.org)
```

st-cli does not convert anything itself. The card is sent to SillyTavern's own import endpoint (`/api/characters/import`, what the web UI's import button uses), so the server reads V1/V2/V3/Pygmalion JSON and PNG cards and saves it the same way. URLs are downloaded by the server too (`/api/content/importURL`). Before sending, st-cli checks that the input really is a card (a PNG with an embedded `chara`/`ccv3` chunk, or JSON with `spec`, `name` or `char_name`) and gives a clear error if not. Supported site: chub.ai; lorebook links are refused.

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

- `vim.enabled: false` removes the modes and the mode tag under the box; the box is plain text editing. Left/Right arrows move the cursor once the box has a draft (on an empty box they switch swipes), and Home/End and Alt+Left/Right (word jump) move the cursor, and Up/Down move through a multi-line draft (with a one-line draft they scroll the chat as before).
- `vim.normal_mode_keys` can be any sequence of two or more characters, for example `jk`. It is typed quickly in INSERT mode (within about 0.6 s) and the keys are removed again.
- `vim.leader` is the `<leader>` key (default `space`; one character that is not a letter or digit, such as `,` or `\`, or the word `space`). See `<leader>y` below.
- `mouse.open_keyboard_on_tap: false` stops st-cli from switching mouse reporting off when you tap the box, so the keyboard does not open by itself on a phone. `mouse.tap_to_move_cursor: false` keeps taps from moving the cursor. The two are independent.

st-cli talks to a SillyTavern server that needs no login (the default single-user setup). Accounts and basic auth are not supported.

## Keys

The row under the input box shows the totals on the right: the number of messages and the token count of the whole prompt, that is the system prompt, character card, persona, example dialogue, instruct formatting and every message, not trimmed to the context window (compare it with your context size to see when old messages start to drop out). With a Text Completion API (llama.cpp, KoboldCpp, ooba, ...) the count is exact: llama.cpp is asked directly (`/tokenize` at the address SillyTavern has for it), anything else, or llama.cpp when it can't be reached from st-cli, goes through SillyTavern's tokenizer endpoint (the one the web UI uses). The backend address is the one SillyTavern uses, so when SillyTavern runs on another machine (e.g. st-cli on a phone, SillyTavern and llama.cpp on a PC) and that address is `localhost` / `127.0.0.1`, st-cli does not try it directly (on the phone it would mean the phone itself) and lets SillyTavern count. The prompt is counted in the background whenever the chat or your SillyTavern settings change. Until the answer arrives, or when nothing can count (Chat Completion APIs, backend not running), the number is an estimate (characters / `chars_per_token`) and shown as `~1.2k`.

| Key | Action |
| --- | --- |
| Shift+Enter (also Alt+Enter, Ctrl+J) | new line. Shift+Enter needs a terminal with the kitty keyboard protocol (kitty, WezTerm, foot, Ghostty, Alacritty, recent iTerm2); elsewhere use Alt+Enter or Ctrl+J |
| Enter, or click **Send** | send. An empty box adds no message; it just asks the character to reply (like the web UI) |
| Esc | stop generating (partial reply is kept) / cancel an edit |
| Ctrl+K, `/continue` or `/c` | continue the last reply (appends to it) |
| Click in the input box | move the cursor there (also while editing a message) |
| Click a message | edit that message in the input box, in INSERT mode (Enter / **Save** saves, Shift+Enter adds a line, Esc cancels; the cursor starts at the end of the text; your unsent draft comes back) |
| Ctrl+S, or the **Card save** button | only while editing the character's greeting (the very first message, at a swipe that came from the card): also writes the edit into the character card itself, as `first_mes` or the matching alternate greeting, so it is permanent and new chats get it. **Save** / Enter only changes this chat |
| Ctrl+R or `/regen` | regenerate last reply as a new swipe |
| Ctrl+P | edit the character's very last reply (opens in the input box like a click on it; Enter saves, Esc cancels) |
| Ctrl+O | edit your own very last message the same way |
| ←/→ | previous / next swipe (→ past the last generates a new one). Only while the input box is empty and no message is being edited; with a draft in the box they move the text cursor. In a new chat, while the greeting is the only message, they switch between the card's alternate greetings instead (wrapping around). Older chats get the card's alternate greetings too |
| PgUp/PgDn, mouse wheel, ↑/↓ | scroll the chat (↑/↓ move the cursor while editing). The wheel (or a swipe) over the text box scrolls only the box, anywhere else only the chat; typing or moving the cursor snaps the box back to the cursor |
| `/undo` or `/u` | delete last message |
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

The server only proxies requests, so the prompt is built client side, as the web UI does. Nothing is configured in st-cli itself: before every reply it re-reads your settings from SillyTavern, so whatever you change in the web UI (API, model, sampler preset, instruct/context templates, system prompt, persona, context size, response length) applies to the next message.

1. `GET /csrf-token` (+ optional login), then `POST /api/settings/get`.
2. Character card from `/api/characters/get`; chats via `/api/chats/get` and `/api/chats/save`.
3. Depending on the API selected in the web UI (`main_api`):
   - **Chat Completion** (OpenAI, Claude, OpenRouter, Gemini, custom, ...): `src/prompt.rs` builds the message list from your prompt manager order and sends it to `/api/backends/chat-completions/generate`.
   - **Text Completion** (llama.cpp, KoboldCpp, Ooba, TabbyAPI, vLLM, Ollama, generic OpenAI-compatible): `src/textgen.rs` renders your context template (story string), instruct template, example dialogue and stop strings, adds the sampler values from the active preset, and sends it to `/api/backends/text-completions/generate`.
4. The SSE stream is rendered live and the chat is saved back to the same `.jsonl` file.

## Limitations

- Supported APIs: Chat Completion and Text Completion with a local/self-hosted server URL. KoboldAI classic, NovelAI, Horde and hosted Text Completion services (OpenRouter-as-textgen, Together, Mancer, ...) are refused with a clear message.
- No World Info/lorebooks, Author's Note, summaries, vector memory, group chats, or tool calling. The prompt is therefore close to, but not byte-identical to, the web UI's when you use those.
- Token counting for trimming history to the context window is an estimate (`chars_per_token`), not a real tokenizer. The totals in the status row are exact only with a Text Completion API (see Keys). Banned plain-text tokens (which need a tokenizer) are not sent; token-id lists and quoted banned strings are.
- Unfinished prose is closed for you while a reply streams: an open `*narration`, `**bold` or `"speech` is shown with its closing marker added, and the model's own closer simply takes its place. Nothing is added for a lone trailing `*`, `2 * 3`, bullets, a `*` inside a word, or past a blank line. When a reply ends with markers still open they are closed in the saved text too (Ctrl+K takes them off again before continuing).
- Markdown is minimal: `*narration*` is grey and bold, `**bold**` is bold and "quoted speech" is yellow. Styles end at a blank line, and a `*` that has no partner (`2 * 3`, a `* ` bullet, an unclosed one) is shown as a plain asterisk.
- Mouse capture is on (for the Send button and click-to-edit); hold Shift to select text with the terminal's own selection.
