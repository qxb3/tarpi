# tarpi reference

The full manual, written to be easy to follow. For a short introduction and a
quick start, read the [README](../README.md) first.

**How to read this page:** each section starts with a short "in plain words"
line. Read that first. The details below it are there when you need them.

## Contents

- [Command line and chat commands](#command-line-and-chat-commands)
  - [Choosing a character at start-up](#choosing-a-character-at-start-up)
  - [Command menu](#command-menu)
  - [Path menu (`/import`)](#path-menu-import)
  - [Command history](#command-history)
- [Sharing between devices (`serve` and `connect`)](#sharing-between-devices-serve-and-connect)
- [API connection (`/api`)](#api-connection-api)
  - [OpenAI-compatible connections](#openai-compatible-connections)
  - [OpenRouter connections](#openrouter-connections)
  - [Ollama connections](#ollama-connections)
- [Formatting](#formatting)
- [Personas](#personas)
- [Prompt window](#prompt-window)
- [Author's Note](#authors-note)
- [System prompts](#system-prompts)
  - [Importing from SillyTavern](#importing-from-sillytavern)
- [Card save](#card-save)
- [Character window](#character-window)
- [Small screens](#small-screens)
- [Nudging a regeneration](#nudging-a-regeneration)
- [Prose colours](#prose-colours)
- [Summarizing a long chat](#summarizing-a-long-chat)
- [Placeholders and tidy text](#placeholders-and-tidy-text)
- [Chats](#chats)
- [Characters](#characters)
- [Lorebooks](#lorebooks)
  - [Your own lorebooks (`/lore`)](#your-own-lorebooks-lore)
- [Vector memory](#vector-memory)
- [Importing characters](#importing-characters)
  - [Importing and deleting while connected](#importing-and-deleting-while-connected)
  - [Copying everything from SillyTavern](#copying-everything-from-sillytavern)
- [Deleting characters](#deleting-characters)
- [Phone / Termux (touch)](#phone--termux-touch)
- [Build](#build)
- [Configuration](#configuration)
- [Keys](#keys)
  - [Vim motions in the input box](#vim-motions-in-the-input-box)
  - [Command line (`:`) and find / replace](#command-line--and-find--replace)
- [How it works](#how-it-works)
- [Limitations](#limitations)

## Words used in this manual

| word | what it means |
| --- | --- |
| **character / card** | The file that describes who you are talking to (name, personality, first message). Usually a `.png` or `.json` from a site like chub.ai. |
| **backend** | The program that runs the AI model (llama.cpp, Ollama, ...). tarpi talks to it. |
| **persona** | Who *you* are in the chat (your name and a short description). |
| **swipe** | A different version of the same reply. Flip between them with Left / Right. |
| **prompt** | All the text tarpi sends to the model for one reply. |
| **lorebook** | A set of notes that are added to the prompt only when certain words come up in the chat. |
| **token** | A piece of text, roughly three or four characters. Models measure length in tokens. |
| **context size** | How many tokens the model can read at once. |
| **window** | A box that opens over the chat (like `/chars` or `/api`). Esc closes it. |

---

## Command line and chat commands

*In plain words: you start tarpi from your terminal, then do everything else by typing `/commands` in the chat.*

tarpi is a chat-only terminal program for character cards. Everything it needs is its own, kept in `~/.config/tarpi/`: characters, personas, system prompts, formatting, API connections and chats. The only thing it needs from the outside is your model backend.

### Starting tarpi

```
tarpi                  # open the character window, pick one, then chat
tarpi tarpi            # continue the latest chat with Tarpi
tarpi tarpi 3          # open Tarpi's chat number 3 (or by name: Tarpi03)
tarpi tarpi -n         # start a new chat (named Tarpi01, Tarpi02, ...)
tarpi -l               # list characters
tarpi tarpi -l         # list Tarpi's chats
tarpi import-st <url>  # copy the personas, system prompts and characters of a SillyTavern server
tarpi serve            # share everything with other devices (see "Sharing between devices"); short: tarpi s
tarpi connect [token]  # use the data of the machine named by serve_url; short: tarpi c
```

That is the whole command line: `[character] [chat]`, plus `-n` (new chat) and `-l` (list), and the three commands `import-st`, `serve` and `connect`.

Good to know:

- `s` and `c` are short for `serve` and `connect`. So a character named `s` or `c` can't be given by name on the command line.
- The character name is matched ignoring capital letters, against the name or the card file name. A unique part of the name is enough.
- Chats are numbered per character (`Tarpi01`, `Tarpi02`, ...). You can give a chat as a number, an exact name, or a unique part of a name.
- You add and delete characters *inside* tarpi: `/import`, and `d` in the `/chars` window.

### Chat commands

Type these in the message box. Type `/` to open a menu of them.

| Command | What it does |
| --- | --- |
| `/chats` | Opens a window with this character's chats. Pick one (↑/↓ then Enter, or click / tap), choose `+ new chat`, or press `d` on one to delete it (it asks `y`). Esc closes. The `_before-summary` copies are not listed. The chat you have open can't be deleted here (use `/newchat`), except when it is the last one: then it is deleted and tarpi goes back to the `choose a character` window. |
| `/one-shot` | **Locks this chat for good.** No editing, no `/undo`, no regenerating, no `/continue`, no swipes (greeting switching too), no summary branches. tarpi asks you to type `/one-shot` a second time; typing anything else cancels. A grey `ONE SHOT MODE` tag then stays at the start of the status row. A reply that is being written can't be stopped either (no Esc, no Stop button, and `/quit`, `/chats` and `/chars` wait until it is done). You can still send new messages. Other chats are not affected. It is stored in the chat file (`tarpi_one_shot`) and **there is no command to undo it.** |
| `/lore` | Opens the lorebook manager (see [Lorebooks](#lorebooks)). |
| `/memory` | Opens the vector memory settings (see [Vector memory](#vector-memory)). |
| `/persona` | Choose who you are in the chat. `+ new persona` makes one, `e` edits, `p` sets where its description goes in the prompt, `d` deletes (see [Personas](#personas)). |
| `/system` | Choose the system prompt. `+ new system prompt` makes one, `e` edits, `d` deletes (see [System prompts](#system-prompts)). |
| `/prompt` | Shows the exact prompt the next reply will be sent with (see [Prompt window](#prompt-window)). |
| `/note` or `/an` | This chat's Author's Note (see [Author's Note](#authors-note)). |
| `/chars` | Every character (★ = favourite). Pick one to switch to it, or press `d` to delete one with all its chats (it asks `y`; see [Deleting characters](#deleting-characters)). |
| `/import <card>` | Adds a character: a card `.png` / `.json` file, a chub.ai or pygmalion.chat link, a direct link to a card file, or the card's JSON text (see [Importing characters](#importing-characters)). |
| `/api` | The API settings: server address, model and every sampler setting. When you quit (`q` / Esc) you are back in the same chat, and the very next reply uses what you saved. |
| `/formatting` | The formatting editor: context template, instruct template and reply cleanup (see [Formatting](#formatting)). |
| `/config` | Opens `config.yaml` in a window (details below). |
| `/char` | Opens this character's card in a window to read and edit (see [Character window](#character-window)). |
| `/char-img` | Shows the card's picture in a square window (details below). |
| `/new` | New chat with this character right away (keeps the current one). |
| `/newchat` or `/nc` | Deletes this chat and starts a new one. |
| `/regen [nudge]` | Writes the last reply again, optionally steered (see [Nudging a regeneration](#nudging-a-regeneration)). |
| `/continue` or `/c` | Keeps writing the last reply. |
| `/undo [n]` or `/u [n]` | Deletes the last message, or the last `n`. |
| `/summarize` or `/summ` | Writes a summary of a long chat as a new branch (see [Summarizing a long chat](#summarizing-a-long-chat)). |
| `/deletebranch` or `/db` | Deletes the summary branch that is showing. |
| `/help` or `/h` | Every key and command. |
| `/quit` or `/q` | Exit. |

More about two of them:

- **`/config`** opens `config.yaml` with line numbers on the left (a long line that wraps is numbered once; the numbers are left out on a very narrow screen). It uses the same vim modes as the message box when `vim.enabled` is on. Tab types two spaces. The spell checker is always off here. **Ctrl+S** (or `:w`, or the Save button) checks the YAML, writes the file and applies it to the running chat at once, with no restart; the window stays open (`:wq` saves and closes). If the YAML has a mistake, the window tells you and nothing is written. Esc cancels (it asks twice when you have unsaved changes). Two settings, `serve_url` and `serve_trust_all`, are only read when `tarpi serve` / `tarpi connect` start.
- **`/char-img`** shows the card's picture. In terminals with the Kitty graphics protocol, iTerm2 images or Sixel (kitty, WezTerm, Ghostty, iTerm2, foot, Windows Terminal, mlterm) it is full resolution. Everywhere else, including Termux and tmux, it is drawn with coloured half-block cells, which is coarse. You can force a mode with `TARPI_IMAGE=blocks|kitty|iterm|sixel` (`sixel` is never auto-detected beyond the terminals listed). Any key or click closes it.

### Choosing a character at start-up

*In plain words: if you run `tarpi` with no name, a window asks who you want to talk to.*

`tarpi` with no character name opens the chat screen with the `/chars` window over an empty screen, titled `choose a character`.

- ↑/↓ then Enter (or click / tap) picks one. Enter opens that character's latest chat (a new one with `tarpi -n`).
- `d` deletes one (it asks `y`, as in `/chars`).
- Esc, or a click outside the window, quits tarpi. Nothing is opened or saved until a character is chosen.

While the only character is the default `Tarpi`, the window shows a yellow guide at the bottom: press Enter on Tarpi, then type `/import` inside the chat. It goes away once you have another character.

With no character cards at all, tarpi first writes a default one, `Tarpi` (`~/.config/tarpi/characters/Tarpi.json`: an unhinged, funny and blunt assistant who is secretly helpful), so the window is never empty. You can edit it with `/char`. It is only made when the characters folder is empty (delete every character and it comes back on the next start). `tarpi -l` does not make it.

### Command menu

*In plain words: type `/` and a list of commands opens above the box.*

The menu narrows as you type (`/ch` lists `/chats`, `/char`, `/chars`, `/char-img`). The top entry is highlighted from the start. It only shows while the box holds a single `/word` in INSERT mode (and not while you are editing a message). Once there is a space after the command (`/regen softer`), it closes.

| Key / click | What it does |
| --- | --- |
| Tab or Down | Highlight the next command and fill it in. A space is added only for a command that takes an argument (`/import `, `/regen `, `/undo `). The top one is highlighted from the start, so the first press already goes to the second. With `/import ` filled in, Tab opens the path menu instead (see below). |
| Shift+Tab | Highlight the previous command and fill it in. Wraps around. |
| Up | On a bare `/` once you have run a command: brings back the command history (see [Command history](#command-history)). Otherwise the same as Shift+Tab. |
| Esc or Ctrl+C | Hide the menu and nothing else. What is in the box stays. |
| click / tap | Fill in that command and close the menu. A command that takes nothing (`/chats`) runs at once; one that takes an argument (`/import `) waits for it. |
| Enter | Chooses the highlighted command when what you typed is only the start of one. A command that takes nothing runs at once (`/su` runs `/summarize`). One that takes an argument is filled in with a space and not run yet (`/imp` becomes `/import `). A full name or short form (`/c`, `/nc`) runs as typed. |

Only `/import`, `/regen` and `/undo` take arguments.

Esc or Ctrl+C while the command menu, the path menu or the spelling menu is showing **only hides that menu**: tarpi does not quit, stop a reply, cancel an edit or leave a mode. The menus stay hidden until the next key. With no menu showing, Esc and Ctrl+C do what they always do (see [Keys](#keys)).

The menu lists the short names `/char` and `/chars` and the short forms `/summ`, `/nc`, `/db`, `/c`, `/u`, `/q` and `/h` (shown in brackets). The longer spellings `/character`, `/characters` and `/summarise` still work when typed in full (`/switch` also opens the character window). While the menu is open, Up / Down move through it instead of scrolling the chat.

### Path menu (`/import`)

*In plain words: after `/import ` press Tab and tarpi lets you browse your folders.*

The menu lists the folders and the card files (`.png` / `.json`) of the folder you are typing in. Each file shows its size. Folders come first, and everything is sorted by name, ignoring case.

- It **never opens by itself**. Press Tab after `/import `.
- What you type after the last `/` narrows the list (any case).
- With just `/import `, it lists the folder tarpi was started in.
- `~` is your home folder.
- Hidden files and folders only show once the name you type starts with a `.`.
- A path in quotes (`/import "~/My Cards/so`) or with `\ ` before its spaces is understood.
- Nothing opens for a link or JSON text (`/import https://...`, `/import {...`), or when nothing matches.
- It works even with `command_menu: false`.

| Key / click | What it does |
| --- | --- |
| Tab | With the menu closed, after `/import `: open it and fill in the first entry right away. |
| Tab or Down | With it open: fill in the next entry. Wraps around. |
| Shift+Tab or Up | Fill in the previous entry (from the first one: the last). Wraps around. |
| Enter | Chooses the highlighted entry. A folder is filled in and opened, so the menu lists what is inside. A file is filled in and **imported at once**. |
| Esc or Ctrl+C | Hide the menu and nothing else. Tab opens it again. |
| click / tap | A folder is opened. A file closes the menu. |

A folder is filled in as `folder/` and a space in a name as `\ `, so what ends up in the box is always something `/import` can read back. Typing after an entry has been filled in carries on from it. The menu closes when the box stops holding an `/import` path.

### Command history

*In plain words: press Up on an empty `/` to bring back commands you ran before.*

tarpi remembers every command it understood, with what you typed after it (`/regen she hesitates, then refuses`, `/undo 3`, `/chats`).

- With only a `/` in the box, **Up** puts the newest one there. Up again goes further back. **Down** comes forward, and Down past the newest puts the `/` back.
- Enter runs the command shown.
- Typing or editing while a command is shown ends the walk, and the keys go back to what they did before.
- It only starts in INSERT mode with a box holding just `/`.
- It works with `command_menu: false` too.

The last 200 different commands are kept in `~/.config/tarpi/command_history` (one per line, oldest first). It survives quitting and carries over when you switch chat or character. Several terminals share it. Unknown commands and ordinary messages are not kept. Delete the file to clear it.

---

## Sharing between devices (`serve` and `connect`)

*In plain words: one computer keeps all your characters and chats. Your phone or laptop connects to it and sees the same things.*

### Setting it up

**1. On the computer that keeps the data**, run:

```
tarpi serve
```

It prints an address, a port and a **token** (a password). Leave it running.

**2. On the other device**, open `~/.config/tarpi/config.yaml` and add the server's address:

```yaml
sync:
  serve_url: 192.168.1.20:7878   # host:port; the port may be left out for 7878
```

**3. Still on the other device**, run (first time only, with the token):

```
tarpi connect <token>
```

The token is remembered. Next time, just run `tarpi connect`.

### All the options

```
tarpi serve                  # share on the network, port 7878
tarpi serve --local          # only this machine (e.g. through an SSH tunnel)
tarpi serve -p 9000          # another port
tarpi serve --new-token      # replace the token

tarpi connect <token>        # first time (the token is remembered)
tarpi connect                # afterwards
tarpi connect --local-api    # keep this device's own API connection (see below)
```

You can also give the token in the `TARPI_TOKEN` environment variable. `serve_url` (under `sync:`) is `host:port` (an old-style `http://` in front is ignored). Without it, `connect` tells you it is missing.

### What `tarpi serve` does

- It does not open the chat screen. It prints the address, the port and the token, then answers requests until you press Ctrl+C.
- By default it listens on every network interface. `--local` limits it to `127.0.0.1`.
- It shares `characters/`, `chats/`, `persona/`, `lorebooks/`, `system-prompt.jsonl`, `vector-memory.yaml` and `api.yaml` from `~/.config/tarpi/` **and nothing else**. `config.yaml`, `formatting.yaml`, `command_history`, `dictionary.txt` and the token stay on each machine.
- The token is made the first time and kept in `~/.config/tarpi/serve-token`.
- You can keep using tarpi on the serving machine as usual.

**The log.** While it runs, `tarpi serve` prints one line per thing the connected devices do, with the time and the device's address:

```
14:02:11  192.168.1.31  first sync: 3 downloaded, 1 uploaded
14:02:11  192.168.1.31  connected
14:02:13  192.168.1.31  opened chat Tarpi03 with Tarpi
14:05:40  192.168.1.31  updated chat Tarpi03 with Tarpi (uploaded)
14:06:02  192.168.1.31  switched to chat Tarpi01 with Tarpi
14:09:12  192.168.1.31  added character Tarpi (uploaded)
14:09:12  192.168.1.31  synced: 1 downloaded
14:11:48  192.168.1.31  left chat Tarpi01 with Tarpi
14:11:49  192.168.1.31  disconnected after 9m 38s
```

The server sees by itself: the live connection, every upload (`added` for a new file, `updated` for an existing one), every delete, uploads it refused because the file had changed there meanwhile, and wrong tokens. The device tells it the rest (a chat opened, switched or left, a new chat, a switch of character, and the result of each sync, including a failed one) in a short note over the same connection. Nothing is printed on the device's own screen, and nothing is written to a file.

### What `tarpi connect` does

It keeps a **copy** of the server's data in `~/.config/tarpi/remote/<host>-<port>/`. You pick a character as in plain `tarpi`, and the normal chat screen opens on that copy. Everything works as usual, even when the connection drops for a while.

- **Syncing.** The copy is brought up to date before the screen opens, and synced once more when you quit. While it is open, a change here (checked every 0.2 seconds) or on the server (the server pushes a note) is synced at once. A slow check every 30 seconds is only a safety net. The last result is in `sync.log` in the copy's folder.
- **Who wins.** A file only the server changed is downloaded. A file only this device changed or created is uploaded. Deletes are passed on. Downloaded files keep the server's modification time, so "latest chat" is the same on every device.
- **Both sides changed the same file.** The newer version wins, and the older one is kept beside it as `<file>.conflict-<time>` (not listed by tarpi, not shared). A file that one side changed and the other deleted is never deleted. A server that suddenly lists no files at all is never treated as "delete everything", and neither is a copy here that suddenly lists none: its files are downloaded again.
- **Files that cannot be read.** A file or folder that is there but cannot be read (a permission error, a program holding the file, a storage that is not mounted) is not "deleted": that round leaves it alone on both sides, and a server tells the device which files it could not read. A file that was changed here while the sync was downloading or deleting it is left as it is; the next round settles it like any other change on both sides.
- **Token counts** (in the status row) are asked from the server's own backend through `tarpi serve`, so the address in the shared connection does not have to be reachable from this device. For replies and the connection check, a shared address of `127.0.0.1`, `localhost` or `0.0.0.0` is read as the server's host (same port). So llama.cpp must listen on a network address there (`--host 0.0.0.0`).
- **One persistent connection.** `tarpi connect` opens a single TCP connection to `tarpi serve` and keeps it for the whole session: the sync, the token counts, the notes for the server's log and the server's "a file changed" notes all travel over it (no HTTP, no new connection per request). While it is quiet, both sides send a tiny beat every 50 ms, and any data counts as a beat, so a long upload does not look like a dead link. If `tarpi serve` stops, the machine goes away, or this device loses its network, `tarpi connect` quits at once; a link that goes silent is noticed after 0.3 seconds when idle, and after 5 seconds while a request or a big file is in flight (a closed or reset connection is always noticed at once). The server lets go of a silent device after 5 seconds. A busy or slow link can close it by mistake; nothing is lost when that happens. Your changes stay in the local copy and are sent the next time you connect. Both machines must run the same version of tarpi: an older `tarpi serve` (HTTP) is refused with a message saying so.
- **`--local-api`** keeps `api.yaml` on this device. A phone usually can't reach a model at the server's `127.0.0.1:8080`, so give it its own connection with `/api`. Without it, API connections are shared like everything else.
- **Characters** are added with `/import` and deleted with `d` in `/chars` on the connected device just like on a single machine; the sync carries both (see [Importing and deleting while connected](#importing-and-deleting-while-connected)).
- **Lorebooks** (`lorebooks/<name>-<id>.json`, one file each) are shared like everything else. A book written on the phone is on the server, and the other way round; its on / off switch and its *On for* list travel with it. `/lore` reads a book when you open it. If another device changed or deleted that book meanwhile, saving is refused (`not saved: this lorebook was changed ... somewhere else`) and nothing is lost: leave without saving (`q`, then `n`) and open it again. A new book's file name ends in six random characters, so two devices that create a book with the same name never write the same file.
- **The same chat open on two devices** behaves like two terminals on one file. When another device has changed the open chat (its file here is no longer the one tarpi loaded or last wrote, even if the other device only added messages), saving is refused (`save refused ... Reopen tarpi`) and nothing is lost. Reopen the chat to load the other version.

### Security (please read)

- `serve_trust_all` (under `sync:` in `config.yaml`, default `false`) decides whether a token is needed. With `false`, `tarpi serve` makes and requires the token, and `tarpi connect` asks for it when it has none.
- With `true`, **anyone who can reach the port gets in without a token** (`tarpi serve` says so when it starts). Only do that on a network you fully trust. `tarpi connect` does not need the setting: it first asks the server whether a token is wanted.
- **`encrypted`** (under `sync:`, default `true`) encrypts the whole link: every device gets its own fresh keys for each connection (X25519 key exchange, ChaCha20-Poly1305), so nobody on the network can read the chats, characters or API keys, and a message that is changed, dropped or repeated on the way ends the link. The token is not sent either: each side proves it knows it, tied to that connection's keys, so someone sitting in the middle can neither listen in nor pass the token check, and `tarpi connect` refuses a server that cannot prove it knows the token. **Set it the same on both devices.** When they differ, nothing connects, and both sides say so: `tarpi connect` prints `encryption settings do not match` with the value on each side and what to change (the setting is read when `tarpi serve` starts, so restart it after changing it), and `tarpi serve` logs `refused: encryption mismatch` with the device's address. A tarpi from before this setting does not connect to an encrypted one: update both.
- With `serve_trust_all: true` there is no token, so the link is protected against listening in, but nothing tells the real server from an impostor on the network. Use a token on any network you do not fully trust.
- With `encrypted: false` the connection is **plain TCP**: use it only on a network you trust, or use `--local` and an SSH tunnel: `ssh -L 7878:127.0.0.1:7878 server`, then `serve_url: 127.0.0.1:7878` and `tarpi connect`.
- Anyone who has the token can read and change every shared file, **including any API key in `api.yaml`**.
- `--new-token` locks everyone out until they are given the new one.
- If the port does not answer from another device, check the firewall of the serving machine.

---

## API connection (`/api`)

*In plain words: `/api` is where you tell tarpi which AI model to use and how it should behave.*

Type `/api` in any chat. A full-screen manager opens. The chat is not closed, so when you quit it (`q` / Esc) you are back exactly where you were: same chat, same scroll position, unsent draft and swipes untouched. What you saved applies at once (the title bar and token count update) and to the next reply. On a phone, mouse reporting is switched on while the manager is open (so sliders can be tapped) and goes back afterwards.

Connections are stored in `~/.config/tarpi/api.yaml`. Replies go **straight to the backend**: llama.cpp's `/completion`, `/v1/completions` for an OpenAI-compatible server, or `/api/generate` for Ollama. The token count in the status row comes from the backend's own `/tokenize` (an OpenAI-compatible connection only asks for it when *Count tokens on the server* is on). There is no fallback to any other API. If `api.yaml` has no active connection, sending a message shows `no API connection` and nothing is generated.

The first time you open it, a connection named `local` (llama.cpp at `http://127.0.0.1:8080`) is created for you to fill in. Press `n` to make another one; it asks for its kind first: **llama.cpp**, **OpenAI-compatible**, **OpenRouter** or **Ollama** (see below).

**The screen:**

- **Left:** your connections. A ★ marks the one chats use.
- **Right:** the server address, an optional API key (only for servers started with `--api-key`), and every setting of the selected connection, grouped as Generation, Sampling, Repetition, DRY, XTC, Mirostat, Dynamic temperature and Advanced.
- Numbers are sliders. Settings that differ from their default are yellow.

| Key / click | What it does |
| --- | --- |
| Up / Down (or `j` / `k`) | Select a setting (in the list: select a connection). |
| Left / Right (or `h` / `l`) | Move a slider by one step, flip a switch. Shift+arrows, `H` / `L`: 10 steps. |
| Home / End | Slider to its minimum / maximum. |
| Enter | Type an exact value (sliders, numbers, text, address, key). Flips a switch. |
| click / drag on a slider, wheel | Set the value with the mouse. The wheel scrolls. |
| `r` (or Backspace) | This setting back to its default. `R` resets every setting of the connection. |
| `t` | **Test** the connection. Shows the loaded model and the server's context size. |
| `c` | Set *Context size* to the one the server reported. |
| `p` | Presets: pick **Normal**, **Creative** or **Crazy** and press Enter to apply it. `*` marks the one in use, `1`-`3` jump to one, Esc cancels. |
| `a` | Use this connection for chats (★). |
| `n` / `d` | New connection / delete the selected one. |
| `[` `]`, Tab | Previous / next connection. Tab moves between the list and the settings. |
| `s` | **Save** to `api.yaml` (asks on quit if there are unsaved changes). |
| `q` / Esc | Quit. |

**Presets.** A preset sets every sampler setting (Sampling, Repetition, DRY, XTC, Mirostat, Dynamic temperature) in one go, so anything it does not name goes back to its default. Generation and Advanced settings (reply length, context size, stop strings, sampler order, grammar) and the address are left alone. Applying one only changes the settings in the window: **press `s` to save**. You can fine-tune with the sliders afterwards.

- **Normal** is balanced (temperature 0.8, min-p 0.05).
- **Creative** raises temperature to 1.15 and turns DRY on.
- **Crazy** goes to temperature 1.8 with XTC, DRY and dynamic temperature.

**Good to know:**

- Chats re-read `api.yaml` before every reply, so saving in `/api` (even from another terminal's chat) applies to the next message.
- *Context size* is how much history tarpi sends. **Keep it at or below the server's `-c`.**
- `stop strings`, `DRY breakers` and `Sampler order` are text fields (the first two a JSON list). A value that is not valid is refused when you press Enter.

### OpenAI-compatible connections

*In plain words: use this for any server that offers the OpenAI "text completions" API.*

This kind talks to `POST /v1/completions`: vLLM, LM Studio, KoboldCpp, TabbyAPI, text-generation-webui, llama.cpp's `/v1` routes, OpenRouter and similar.

- **Address:** for example `http://127.0.0.1:8000`. A trailing `/v1` is fine; it is added for you if missing.
- **API key:** only if the server wants one (sent as `Authorization: Bearer`).
- **Model name:** sent as `model` with every request. OpenAI, vLLM, OpenRouter and LM Studio need it; many local servers ignore it.
- **`t` (test)** uses `GET /v1/models` and shows the first model it lists (and vLLM's context length, which `c` can copy).

tarpi builds the whole prompt itself (your context and instruct templates), so it sends it as one text `prompt`. The **chat completions** endpoint (`/v1/chat/completions`) is not used, so a model that is only offered as a chat model there will not work.

**Which settings are sent.** Temperature, Top P, *Max reply tokens*, the stops and *Stream reply* are always sent. Settings only some servers know (Top K, Min P, Typical P, the repetition penalty (sent as both `repetition_penalty` and `repeat_penalty`), the frequency and presence penalties, a seed, *Ban EOS token*) are left out while they hold their "off" value (0, 1 or -1), so a strict server never gets a field it would refuse.

For anything else a server accepts (XTC, DRY, `top_a`, ...), type a JSON object into *Extra request fields*, for example `{"xtc_probability": 0.5, "xtc_threshold": 0.1}`. It is merged into every request and wins over the settings above. The Normal / Creative / Crazy presets set Sampling and Repetition only; they switch Top K, Min P and the repetition penalty on, so use them with servers that know those fields.

**Token count.** OpenAI itself has no tokenizer route. Where the server has `POST /tokenize` (vLLM and llama.cpp do) and *Count tokens on the server* is on, the count in the status row is exact. Otherwise tarpi uses what the server reports with every reply: a streamed request carries `stream_options: {"include_usage": true}`, and the `usage` that comes back (prompt plus reply tokens) is shown as the chat's token count, exact, until you change the chat (a new message, an edit, an undo). Then it is the `chars_per_token` estimate (`~`) again until the next reply. A server that rejects `stream_options` can be told to leave it out with `{"stream_options": null}` in *Extra body*.

### OpenRouter connections

*In plain words: use this to reach models over the internet through openrouter.ai.*

This is the OpenAI-compatible kind already set up for [openrouter.ai](https://openrouter.ai). The address is `https://openrouter.ai/api`, and your API key (from openrouter.ai/keys) goes in *API key*. Replies use the same `/v1/completions` text route, so **only models that still offer plain text completion work**. A chat-only model answers with an error that tarpi shows as it is.

**Picking a model.** Select **Model** and press Enter. tarpi fetches OpenRouter's model list (no key needed) and opens a window:

- Type to filter. Every word has to appear in the model's id or name, any case (`llama 70b`, `claude sonnet`, `free`).
- Up / Down, PageUp / PageDown, Home / End or the wheel move the highlight.
- Enter or a click picks it. Esc cancels.
- Ctrl+U clears the filter. Ctrl+R fetches the list again.
- Each line shows the context length and the price per million tokens (input/output, or `free`). `*` marks the model in use.
- If nothing matches, Enter uses what you typed as the model name, so an id missing from the list still works.

After picking, press `c` to set *Context size* to that model's context length, then `s` to save.

`t` tests the connection with `/v1/key`, so a missing or wrong API key shows up there. OpenRouter has no `/tokenize` route and no EOS ban, so those two settings are not listed. The token count in the status row is the `usage` OpenRouter sends with every reply: exact right after a reply, an estimate (`~`) once the chat has changed.

### Ollama connections

*In plain words: use this for Ollama running on your computer.*

This kind uses Ollama's own API, `POST /api/generate` (default address `http://127.0.0.1:11434`). An API key is only for a proxy or hosted Ollama in front of it. It exists next to the OpenAI-compatible kind because tarpi builds the whole prompt itself, so the request is sent **raw**: Ollama does not wrap it in the model's own chat template a second time.

- **Picking a model.** Select **Model** and press Enter. tarpi fetches the models installed in Ollama (`GET /api/tags`) and opens the same filterable window as for OpenRouter, with each model's size, quantization and file size. If nothing matches, Enter uses what you typed. Ollama needs a model: without one it answers `model is required`.
- **Sampler settings** are sent under Ollama's own names in the request's `options`.
- **Context size** is also sent as `num_ctx` with every request. Ollama's own default window is small, and it cuts the *start* of a longer prompt without telling you. Keep it within what the model and your memory can hold. A different value than the one the model is loaded with makes Ollama load it again; `t` and `c` help with that (below).
- **Keep model loaded** is Ollama's `keep_alive` (`5m`, `1h`, `-1` for ever, `0` to unload at once; empty = Ollama's own, 5 minutes).
- **Extra options** is a JSON object merged into `options` (for example `{"num_gpu": 20, "mirostat": 2}`). **Extra request fields** is merged into the request itself (for example `{"think": false}`; `prompt` and `options` cannot be replaced there).
- Presets set Sampling and Repetition only, as for the other kinds.

`t` tests with `GET /api/ps`: it shows the model that is loaded and the context length it was loaded with. `c` copies that length into *Context size*, so the next reply does not make Ollama load the model again. With nothing loaded it just says `connected`. An older Ollama without `/api/ps` is tested with `/api/tags`.

**Token count.** Ollama has no tokenizer route, and having the model read the whole prompt just to count would be slow. So, as with OpenRouter, there is no *Count tokens on the server* setting. Instead the last object of every reply carries `prompt_eval_count` and `eval_count`, and tarpi shows their sum: exact right after a reply, the `chars_per_token` estimate (`~`) once the chat has changed. An old Ollama that leaves `prompt_eval_count` out when the whole prompt was cached gives no count for that reply, and the estimate stays.

**For programmers.** A connection kind is one small file: implement `conn::Provider` (its settings list, request body and stream parser; see `src/conn/llamacpp.rs` and `src/conn/openai.rs`; `src/conn/ollama.rs` is one whose reply stream is not `data:` lines) and add it to `conn::providers()`. The manager, the file format, token counting and generation pick it up without further changes.

---

## Formatting

*In plain words: this controls how tarpi lays out the text it sends to the model. The defaults work for most models. Only change it if your model needs a specific template (ChatML, Llama 3, ...).*

`/formatting` opens a full-screen editor. The settings are tarpi's own, kept in `~/.config/tarpi/formatting.yaml` (every setting is written out; hand edits are fine). It is **local to each device**: `tarpi serve` and `tarpi connect` do not share it. Until you save, the built-in defaults apply, and the editor starts from them. The next reply uses what you saved; nothing is sent to the server by the editor.

**The three parts:**

- **Context template.** The "story string": `{{#if description}}...{{/if}}`, `{{description}}`, `{{personality}}`, `{{scenario}}`, `{{persona}}`, `{{system}}`, `{{char}}`, `{{user}}`. Also: where the story string goes (`prompt`: top of the prompt, or `chat`: into the history at a depth and as a system / user / assistant message), *Always add the name* (end a non-instruct prompt with `Name:`), *Example messages* (`gradual push-out`, `always include` or `never include`), the example separator and chat start, and the stop-on-separators / stop-on-names switches.
- **Instruct template.** Instruct mode on/off, names (`none` / `force` / `always`), wrap and macros, the prefix and suffix of user, assistant and system messages, first / last assistant prefix, first / last user prefix, last system prefix, system-same-as-user, skip examples, the stop sequence, sequences as stop strings, and the story string prefix / suffix.
- **Reply cleanup.** Trim unfinished sentences, collapse blank lines, single line, and custom stop strings (one per line, with or without macros).

**Presets (press `p`).** They are starting points for the part the cursor is in.

- **Instruct template:** Off, ChatML, Llama 3, Llama 2, Mistral, Mistral V7, Phi-3, Phi-4, Gemma, Zephyr, Command R, DeepSeek V3, OpenChat, Granite, Vicuna, Alpaca and Metharme (roughly the chat templates llama.cpp knows), and more after SillyTavern's own: ChatML Names, Llama 3 Names, Llama 4, Llama 2 Chat, Gemma 2, GLM-4, Kimi K2, Alpaca Roleplay, Vicuna 1.0, Koala, Synthia, Tulu and OpenOrca OpenChat. They replace the instruct template and keep the story string.
- **Context template:** Default, Short, Labeled, Pygmalion, Story, Old Default, Simple Proxy, Alpaca, Metharme, Roleplay and No separators. They replace the story string, example separator and chat start and keep the stop switches. Every other setting stays as it is.
- Reply cleanup has no presets.

| Key | What it does |
| --- | --- |
| ↑ / ↓ (`k` / `j`), PgUp / PgDn, Home / End | Select a setting (click works too; a second click acts on it). |
| Enter | Edit a text (in NORMAL mode when `vim.enabled`; Enter accepts a one-line text), or flip / step the setting. |
| ← / → (`h` / `l`) | Flip a switch or step through the choices. |
| Tab / Shift+Tab | Jump to the next / previous section. |
| `r` / `R` | Reset the setting / every setting to its default. |
| `u` | Undo the last change (as many times as you like). |
| `p` | Presets of the part the cursor is in (↑ / ↓, Enter applies; `*` marks the one in use). |
| `s` | **Save.** |
| `q` / Esc | Quit (asks to save when there are unsaved changes). |

**Text windows.** Enter on a text setting opens it in the same editor as the chat box, in a window as big as the text needs.

- A one-line text (a prefix, a suffix, the separator, ...) gets a small window that grows if the text wraps. Type `\n` for a line break and `\\` for a backslash. Enter accepts it. Quotes and brackets are not auto-closed.
- The **story string** has real lines. Enter is a new line, and you accept it with **Ctrl+S**.
- With `vim.enabled`, the window opens in NORMAL mode (press `i`, `a`, `A`, ... to type) and has INSERT / NORMAL / VISUAL modes, every motion and operator, `:` commands such as `:%s/find/new/g`, and the Ctrl+A / E / W keys of the other windows. The current mode shows as a coloured tag on the bottom edge. Without vim modes, it opens ready to type with plain editing.
- Ctrl+S or `:w` / `:wq` accepts the text. Esc leaves INSERT mode and then cancels (when something changed, the first Esc only warns and a second one drops the change).
- Below the list, a text setting is shown in full, wrapped, with its line breaks as `\\n`.

**The editor checks your work.** *Story string depth* has to be a whole number from 0 to 9999 (the window stays open until it is). A story string with an `{{#if}}` that is never closed, a stray `{{/if}}` or `{{else}}`, or an `{{#if name}}` that tests a name that is never filled in (a typo such as `{{#if descripton}}`) is kept as typed, with a warning in the status line. *Unsaved changes* is shown only while the settings really differ from what is saved.

**Editing `formatting.yaml` by hand.** A number where a text goes (`story_string_depth: 3`), an empty value (`chat_start:` means no text), `wrap: no` / `on` / `off` for switches, and a choice in any case (`names_behavior: Force`) all mean what they say. If the file cannot be read at all (a YAML mistake), `/formatting` still opens on the defaults and says why. Saving first keeps the old file as `formatting.yaml.bad`.

---

## Personas

*In plain words: a persona is "you". It has a name and a short description that the character sees.*

Personas belong to tarpi. Each one is a small YAML file in `~/.config/tarpi/persona/` (`<id>.yaml` with `name`, `title`, `description`, `position`, `depth`, `role`, `lorebook`; hand edits are fine). The file `persona/active` holds the id of the one in use. With no persona saved yet, the list shows a plain `User`, which is written to disk once you edit it.

**Using `/persona`.** It opens a window with every persona as `Name (Title)` with a short preview of its description. The active one is marked `(current)`.

- Pick one with ↑/↓ then Enter (or click / tap). Esc closes.
- The choice applies from the next message on (and to chats open in other terminals, which re-read the folder before every reply).
- The first row, `+ new persona`, opens a form (name, title, description). **Ctrl+S saves, Esc cancels.**
- `e` edits the selected persona. `d` deletes it (asks `y`; the persona in use can't be deleted).
- `p` sets where its description goes in the prompt (below).

**Where the description goes (`p`).** This is the `position`, `depth` and `role` of its file:

| Row | Values |
| --- | --- |
| Position | **In the prompt** (`position: 0`, the default: with the character card, in the context template's story string). **Top of the Author's Note** (`2`) or **Bottom of the Author's Note** (`3`): added above / below the text of the chat's Author's Note and sent wherever that goes (see [Author's Note](#authors-note)). **In the chat, at a depth** (`4`: added to the chat history). **Not sent** (`9`: the description is left out). |
| Depth | Only for the chat position: how many messages from the end. `0` is after the last message (default `2`). |
| Role | Only for the chat position: whether the injected message is a `system`, `user` or `assistant` message. |

Keys: ↑/↓ (or `j` / `k`, Tab) pick a row. ←/→ (or `h` / `l`) change it (Shift+arrows, PgUp / PgDn, `H` / `L`: ten at a time on Depth). Digits type the depth. Enter or Ctrl+S saves, Esc cancels; both bring the `/persona` window back. With the mouse or a finger, a click on the left half of a row steps back and on the right half steps on. A click outside closes it. The change applies from the next message on.

A persona imported by `tarpi import-st` with its old *After the character* position (`1`) shows it as *not supported, not sent*. Stepping to another position replaces it.

---

## Prompt window

*In plain words: `/prompt` shows you exactly what the model will be sent for the next reply. Useful for finding out why a character acts a certain way.*

`/prompt` shows the prompt as tarpi would send it, built right then from the current settings, persona, system prompt, character card and chat: the system prompt and character card (the context template's story string), the example dialogue, the instruct formatting, and the messages that fit into the *Context size*. Older messages that would be cut are not shown, like in the real request. **Nothing is sent or saved by opening it.** On a summary branch it is the prompt of the branch that is showing.

It has two pages, shown as tabs in the title:

1. **pre-prompt**: everything before the chat, with no messages in it (system prompt, character card, persona, examples).
2. **post-prompt**: the latest 20 messages (an Author's Note, post-history instructions or a `/regen` nudge sent among them count as messages) and the character cue at the very end.

When more than 20 messages fit into the context, the older ones are in the request but on neither page; the post-prompt starts with a line saying how many. The title shows the size of the whole prompt in characters and an estimated token count (`~`).

| Key / click | What it does |
| --- | --- |
| Up / Down (or `k` / `j`), wheel | Scroll one row (the wheel three). |
| PgUp / PgDn (or Space) | Scroll a page. |
| Ctrl+U / Ctrl+D | Half a page up / down. |
| Home / End (or `g` / `G`) | Top / bottom. |
| Tab / Right / `l`, Shift+Tab / Left / `h`, `1` / `2` | Post-prompt page / pre-prompt page (each page opens at its top). |
| Esc, `q`, Enter, Ctrl+C or a click | Close. |

Long lines are wrapped; tabs are shown as four spaces. The window is read-only, and a typed draft in the box is not part of it.

---

## Author's Note

*In plain words: a short note to the AI that you attach to one chat, like "Keep the pace slow." or "[Style: terse, present tense]".*

It is kept in the chat file (`note_prompt`, `note_position`, `note_depth`, `note_role`, `note_interval`). It is **per chat**: a new chat starts without one, and `/newchat` does not carry it over.

`/note` (or `/an`) opens a window with the note's text and four rows under it. With vim modes on it opens in NORMAL mode (press `i`, `a`, ... to type). **Ctrl+S saves, Esc cancels.** The text works like the fields of `/char`, including the vim modes and the spell checker. The rows are never typed: with the cursor on one, ←/→ (or `h` / `l`) change it, ↑/↓ (or `j` / `k`, Tab) move between rows, and a click on the left or right half of a row steps back or on.

| Row | Choices |
| --- | --- |
| Author's note | The text. `{{char}}`, `{{user}}` and the other macros work. **Empty = nothing is sent** (there is no on/off switch). |
| Position | *In the chat* (default): into the chat history. *After the card*: with the system prompt and character card, after them. *Before the card*: the same, before them. |
| Depth | Only used with *In the chat* (always shown). The note goes before the last N messages: `4` reads *before the last 4 messages*, `1` *before the last message*, `0` *after the last message* (default `4`). ←/→ step one, PgUp / PgDn ten, or type digits (0-999). |
| Role | Only used with *In the chat* (always shown): whether the injected message is a `system` (default), `user` or `assistant` message. |
| Interval | The note is sent with every N-th message of yours: `1` (default) every reply, `3` every third, `0` never (0-9999, same keys as Depth). |

`before` and `after` fill `{{anchorBefore}}` and `{{anchorAfter}}` of the context template's story string. A template that has neither still gets the note, before or after the story string. The change applies from the next message on (the token count and `/prompt` change at once).

A persona whose position is **Top** or **Bottom of the Author's Note** (see [Personas](#personas)) is added above or below the note's text, on its own line, and travels with the note: the note's Position, Depth and Role decide where it goes. It is sent even when the note has no text, and also when the Interval says the text is skipped for this reply.

---

## System prompts

*In plain words: the system prompt is the standing instruction at the top ("You are {{char}}. ..."). You can keep several and switch between them.*

They are kept in one file, `~/.config/tarpi/system-prompt.jsonl`, one JSON object per line (hand edits are fine):

```
{"id":"roleplay-simple","name":"Roleplay - Simple","content":"You are {{char}}. ...","post_history":"","active":true}
```

- `id`: a short file-name-like id.
- `name`: what the window shows.
- `content`: the system prompt itself.
- `post_history`: post-history instructions.
- `"active": true`: marks the prompt in use (only one line has it; the key is left out on the others).

The text of the prompt in use goes into the `{{system}}` part of the context template for every reply. With none saved or chosen, the system prompt is empty (the character card's own system prompt and post-history instructions, if it has them, still apply). `{{char}}`, `{{user}}` and the other macros work in a system prompt. A character card's own system prompt still replaces the prompt in use (`{{original}}` in it stands for the prompt in use).

**Using `/system`.** It opens a window with every system prompt by name and a short preview. The one in use is marked `(current)`.

- Pick one with ↑/↓ then Enter (or click / tap). Esc closes. The choice applies from the next message on (and to chats open in other terminals).
- `+ new system prompt` opens a form (name, system prompt, post-history instructions). Ctrl+S saves, Esc cancels. The fields work like those of `/char`, including the vim modes and the spell checker. **A new prompt is not switched on by itself: pick it in the window.**
- `e` edits one. `d` deletes it (asks `y`; the one in use can't be deleted).

### Importing from SillyTavern

```
tarpi import-st http://127.0.0.1:8000
tarpi import-st http://127.0.0.1:8000 --overwrite
```

This copies every persona of that SillyTavern server (name, title, description, position, depth, role, lorebook) into the persona folder. The server is only read, never changed, and the personas' pictures are not copied. `host:port` works without `http://`.

- Personas may share a name: each gets its own file (`alex.yaml`, `alex-2.yaml`, ...).
- A persona identical to one you already have (same name, title and description) is skipped, so running it again is safe.
- `--overwrite` / `-o` replaces the one with the same name and title instead of adding another.
- If you have not chosen a persona here yet, the one the server had active becomes yours too.

The server's system prompts (its presets, plus the one it has switched on) go into `system-prompt.jsonl` the same way: identical ones are skipped, `--overwrite` replaces one with the same name instead of adding `name-2`, and if you have not chosen one yet, the server's active one becomes yours.

**The top bar.** In a chat, the bar at the top reads ` tarpi <version> ` (white badge) `<character>/<chat file> • <persona> (<title>) • <api>@host:port`, for example `Tarpi/Tarpi01`. The API type and `host:port` are those of the active connection. **The connection turns red when the backend does not answer** (checked in the background every 5 seconds) or when no connection is set. Message names are black on white, with a grey rule between messages (none under the last one; see `msg_seperator` in [Configuration](#configuration)).

---

## Card save

*In plain words: while you edit the first message of a chat, you can also write your edit back into the character card.*

While you edit the chat's first message, **Ctrl+S** (or the **Card save** button) writes the edit into the character card itself. tarpi finds which greeting you are looking at by matching its text against the card's first message and alternate greetings, so it updates the right one (`first_mes` or the matching alternate greeting) even when the swipes are in a different order. The status line says which one was saved. `{{char}}` / `{{user}}` (or `<BOT>` / `<USER>`) are put back in the card text if the original greeting used them.

(**Save** / Enter only changes this one chat. **Card save** changes the card for every future chat.)

---

## Character window

*In plain words: `/char` lets you read and edit everything on the character's card.*

`/char` opens the whole card in a window: card file, name, description, personality, scenario, first message, example dialogue, system prompt, post-history instructions, every alternate greeting and the Character's note. The text is the raw card text (`{{char}}` / `{{user}}` stay as they are).

All fields except the card file (read-only) can be edited, including the name. Renaming writes the new name to the card (the card file and your chats keep their names). The name is one line and can't be empty. The field with the cursor has a cyan title, and the window scrolls to follow it. It opens with the cursor at the top of the first editable field (Name), in NORMAL mode (with vim modes on; press `i` to type).

The **Character's note** (SillyTavern's depth prompt, kept in the card as `extensions.depth_prompt`) is a text like the others, with two rows under it: **Depth** (*before the last N messages*, `0` is after the last one) and **Role** (`system`, `user` or `assistant`). Change them with ←/→ (or `h` / `l`), digits for the depth, ↑/↓ to move, or a click on the left or right half. It is sent with every reply, independently of the chat's Author's Note. An empty text sends nothing.

**Comments.** The card's text fields (description, personality, scenario, the first message, example dialogue, system prompt, post-history instructions, the alternate greetings and the Character's note) take `//` and `/* */` comments, for notes to yourself that the character should never say. `//` comments out the rest of the line; it counts at the start of a line or right after a space, so a URL such as `https://example.com` stays part of the text. `/* ... */` may span several lines, and an unclosed `/*` comments out the rest of the field. Comments stay in the card file and are drawn gray in this window, but they are cut out before the prompt is built: nothing of a comment is sent to the model, and a field holding only a comment counts as empty. Greetings are made without their notes too. The Name is the one text field without comments (there a `//` is part of the name).

| Key / click | What it does |
| --- | --- |
| Tab | With `spellcheck: true`: spelling suggestions for the misspelled word at the cursor (see [Configuration](#configuration)). It does not jump to the next field. |
| Up / Down | Move a row. Past the first / last row of a field they go on to the previous / next field. |
| Enter | New line inside the field. |
| click / tap on a field | Puts the cursor there. The vim mode stays as it was. |
| `gg` / `G` (NORMAL mode) | First line of the first field / last line of the last field of the whole window. |
| Left, Right, Home, End, Ctrl+A / Ctrl+E, Backspace, Delete, PgUp / PgDn | As in a text box. |
| Ctrl+U / Ctrl+D (NORMAL / VISUAL mode) | Half a window up / down. On reaching the edge of a field, move into the previous / next field. |
| mouse wheel | Scroll the window. |
| Ctrl+S, `:w` (NORMAL mode, vim modes on), or the **Save** button | Write the changed fields to the card file in `~/.config/tarpi/characters/` (as Card save does). The window stays open. |
| `:wq` (or `:x`) | The same, then close the window. |
| Esc, or the **Cancel** button | Close without saving. |

The other windows with this form (`/persona`, `/system`, `/note`, `/config`) work the same way: Ctrl+S / `:w` / Save only save, `:wq` saves and closes (a new persona or system prompt is created by the first save; later saves update it).

The focused field is edited with the same editor as the chat box, so the vim modes work there too (unless `vim.enabled: false`). `qq` typed quickly or Esc goes to NORMAL mode, `i a I A o O` go back to INSERT, and all the motions, operators, VISUAL mode and `:%s/find/new/` work. Find / replace only ever touches the field you are in, never the rest of the card. While you type it the matches are highlighted and, once you type the replacement, the field shows the result with the new text in green, with the field name and match count on the row above the buttons. In NORMAL mode, `j` / `k` on the last / first row of a field carry on to the next / previous field. With vim modes on, Esc in INSERT mode goes to NORMAL mode, and Esc in NORMAL mode closes the window (with unsaved changes it asks for a second Esc first).

**Saving details.** Only fields that changed are sent. The alternate greetings are saved as one list (a greeting you empty is dropped). If a save fails, the window stays open with the error in the status line. Chats already started keep their own messages; new chats use the saved card.

---

## Small screens

*In plain words: tarpi shrinks to fit small terminals, down to about 10 columns by 6 rows. Below that it just says `too small`.*

- **Under 44 columns:** the buttons shrink to `Send` / `Stop` / `Save` / `Card`, and the top bar drops the API, then the persona, then the version number, to fit.
- **Under 16 rows:** the two stacked buttons while editing a greeting become two one-row buttons, and the input box never grows so tall that fewer than two message rows are left.
- **Windows** (help, `/char`, `/chars`, `/chats`, persona windows and `/char-img`) are drawn between the top bar and the input box, with a margin of one column on each side (and one row above and below when there is room), so both stay visible. On a small terminal they take that whole area, and help lines wrap. Only under 6 rows by 12 columns of such space do they take the whole screen.
- The `/chars`, `/chats` and persona windows take the full width (less the margin) on small screens and cut long lines with `…` (narrow terminals show `msgs`).

---

## Nudging a regeneration

*In plain words: `/regen` with a few words tells the model which way to go this time.*

`/regen <text>` writes the last reply again with a direction for the model, for example `/regen she hesitates, then refuses` or `/regen shorter, more dialogue`.

- The nudge is added to that one prompt as the very last system message, right before the character's turn, as `[Direction for <char>'s next reply: <text>]`. Macros like `{{user}}` work in it.
- It is **never saved** to the chat.
- The new reply replaces the old one and its swipes, as usual.
- A plain `/regen` or Ctrl+R has no nudge.

---

## Prose colours

*In plain words: tarpi uses your terminal's own colours, so it looks right on light and dark themes.*

At start-up tarpi asks the terminal for its foreground, background and yellow (`OSC 10`, `OSC 11`, `OSC 4;3`).

- Plain text keeps the terminal's default colour.
- `*narration*` is bold and a neutral grey between the theme's foreground and background brightness (readable on light and dark themes).
- `"speech"` is the theme's yellow.
- Speech inside narration (`*he said "hi"*`) is just the plain speech colour (no blending).
- Narration inside speech (`"well *sigh* ok"`) is bold and a lighter shade of the speech colour (the yellow lifted 45% of the way towards your terminal's text colour), so it stands out from the quote around it. The rest of the quote is not bold.
- `` `code` `` has its backticks hidden and sits on a background 5% lighter than your terminal's background (5% darker on a light theme, where it cannot get lighter). Inside it everything is literal: `*`, `**` and quotes are not styled. It keeps the colour of the narration or speech around it. Use more backticks (``` ``a ` b`` ```) to put a backtick inside. A `` ` `` with no closing one in the same paragraph is shown as a plain backtick; while a reply streams, an open `` `code `` is closed for you like the other markers.

Terminals that don't answer cost at most 300 ms once. Then `COLORFGBG` (light / dark) and xterm-like colours are used.

---

## Summarizing a long chat

*In plain words: when a chat gets too long and slow, `/summarize` makes a short version as a new "branch". Your original chat is kept untouched.*

`/summarize` reads the whole chat and writes an in-depth summary: who is who and how they relate, the events in order, every fact the story may depend on later (names, places, objects, injuries, promises, secrets, decisions, feelings), what was said that matters, the open plot threads, and, in the last paragraph, the situation right now.

The summary is not a swipe. It is a **summary branch**, a separate kind of view next to swipes and greetings. Every `/summarize` adds a new one (there is no limit):

- **branch 1** is the actual conversation, exactly as it was.
- **branch 2, 3, ...** are the summaries, oldest first. A summary branch shows just the summary as its first message (about **a quarter** of the tokens, so -75%; aimed at 25% of the text's length, but never more than a quarter of the *Context size* of your API connection). Everything you say from there on continues from it.

`/summarize` summarizes the branch that is showing (the conversation, or an earlier summary together with what came after it) and adds the result as a new branch at the end, which then shows. **Nothing is replaced.** The first message shows a cyan `summary branch 1/3` counter. Each branch keeps its own messages, and the model only sees the branch that is showing.

While it is being written, nothing streams into the chat: the status row says `summarizing…` (with `part 2/4` for long chats), and when it is finished it appears whole. **Esc stops it, and then nothing is changed.**

**Switching branches** (empty input box, not while editing): **Ctrl+Left** (previous) and **Ctrl+Right** (next), wrapping around, or tap / click the left / right half of the `summary branch n/m` counter. Plain Left / Right keep browsing swipes and greetings, and on a lone summary they generate nothing.

**Deleting a branch.** `/deletebranch` (or `/db`) deletes the branch that is showing and shows the one before it (the one after it when the first was deleted). With only one branch left the chat is a normal chat again; in a chat without branches it just says so. It also deletes the `_before-summary` copy `/summarize` saved (the newest one; all that are left when no branch remains). It works on branch 1, the conversation, too. **Deleting is permanent.**

**Details:**

- The summary is written with the active API connection (`/api`), with its own settings: the reply length is raised as needed and sampling is made calm (low temperature, no DRY / XTC / penalties / dynamic temperature, no extra stop strings). Your saved connection is not changed. The character card, system prompt and persona are not part of that request.
- A chat that does not fit into the context together with its summary is read in parts: each request gets the summary so far plus the next part of the chat. If the *Context size* is too small even for that, `/summarize` says so and does nothing.
- Before anything is changed, the full chat is also saved as `<chat>_before-summary` (then `...2`, `...3`). It is a normal chat file, but `/chats` does not list it, and `/deletebranch` removes it again. If that copy cannot be saved, the chat is left alone.
- The branches are stored in the chat file, inside the first message (`tarpi_branch`), so the file stays a normal `.jsonl`.
- `Ctrl+S` / **Card save** is not offered on the summary: it is never written into the character card. Branched chats no longer get the card's alternate greetings added as swipes. `/undo` will not delete the first message of a branched chat.
- It needs at least three messages. Summaries are written by the model, so **read the result**; branch 1 and the saved copy have everything.
- Chats summarized by an older version (summary as a second swipe of the first message) are left as they are and behave like a normal chat; run `/summarize` again to get branches. Chats with the earlier two-branch format load as they are.

---

## Placeholders and tidy text

*In plain words: `{{user}}` and `{{char}}` become the real names, and a few special tags do small tricks.*

- `{{user}}`, `{{char}}` and the old `<USER>` / `<BOT>` / `<CHAR>` / `<GROUP>` are replaced in greetings (when a chat is created) and in every message on screen.
- When you send a message, these macros are filled in: `{{roll:1d6}}`, `{{random::a::b}}`, `{{time}}`, `{{date}}`, `{{time_UTC+2}}`, `{{newline}}`, `{{trim}}` and `{{//comment}}`. Unknown ones are left as typed.
- Messages you send or edit (and replies) are trimmed: no spaces or empty lines at the start or end, no trailing spaces on lines.

---

## Chats

*In plain words: each chat is one plain text file you can open, copy or back up.*

Chats are kept at `~/.config/tarpi/chats/<Name>/<chat>.jsonl`, one folder per character (named like the card file without `.png`). Each chat is a `.jsonl` file: a header line, then one message per line, unknown fields kept.

Saving refuses to overwrite a file that another terminal changed in the meantime (`chat_metadata.integrity`). Reopen tarpi to reload it. `tarpi import-st <url>` copies the chats of every character of a server here (a chat that already exists here is kept; `--overwrite` replaces it).

---

## Characters

*In plain words: each character is one file in `~/.config/tarpi/characters/`.*

- `Name.png`: a card PNG with the card embedded (the `chara` / `ccv3` text chunks chub.ai and other sites use). The picture is what `/char-img` shows.
- `Name.json`: a card that is only JSON (V1, V2, V3 or Pygmalion). It has no picture. V1 and Pygmalion cards are read as V2; if you edit one with `/char` or Card save it is saved back as V2.

A character is identified by `Name.png` whichever of the two it is stored as. That name is also the folder its chats live in (see [Chats](#chats)). `/char` and Card save edit the card file right here (a PNG keeps its picture; only the embedded card data is rewritten). Cards you drop into the folder by hand are picked up the next time the list is read.

---

## Lorebooks

*In plain words: a lorebook is a set of notes that tarpi adds to the prompt only when certain words come up in the chat. For example, an entry about "the Silver Tower" is only sent when someone mentions the tower. This keeps the prompt short.*

A card's own lorebook (the `character_book` that SillyTavern and chub.ai put in V2 / V3 cards) works as it does there, in the small part of it that cards use. **Nothing to set up:** import the card and the entries are used.

Before every reply, the latest messages (the book's `scan_depth`, 4 when it has none) are searched for each entry's keys, and the entries that match are added to the prompt.

- **Keys.** An entry is on when one of its `keys` is in those messages, ignoring case unless the entry is case sensitive. A `constant` entry is always on. A selective entry (`selective`) with `secondary_keys` also looks at those, as its *secondary logic* says (SillyTavern's `selectiveLogic`, in `extensions`): **needs any one** of them (the default), **needs every one**, **blocked if any appears**, or **blocked if all appear** (it still fires when only some do). In `/lore` this is the *Secondary logic* row; off means the secondary keys are ignored. Keys are plain text: `/regex/` keys are not evaluated.
- **Where it goes.** `before_char` fills `{{wiBefore}}` of the story string and `after_char` fills `{{wiAfter}}` (the default templates have both; a template without them leaves lore out, as in SillyTavern). An entry set to "at depth" goes into the chat like the Author's Note, with its depth and role. The Author's Note and example-message positions of SillyTavern count as before / after; an entry that came with one of those slots keeps its own number when it is saved or exported, until you change its position.
- **Matching.** Curly quotes (`’` `“ ”`) count as straight ones, and any run of spaces or line breaks counts as one space, so the key `Jack's` finds `Jack’s` and `Silver Tower` finds `silver\ntower`.
- **Order.** Entries go in by `insertion_order`, lowest first. With a `token_budget`, the highest orders are tried first and kept while they fit; an entry that no longer fits is skipped and the smaller ones after it still get their turn (token counts are the `chars_per_token` estimate). An entry bigger than the whole budget is never used: `/lore` says so when you save such a book. A book with no `token_budget` (0) is still held to a quarter of the room the prompt has (*Context size* minus the reply length), shared by all such books, so lore cannot push the chat out of the context; set a budget to choose your own. The same text in the same place is sent only once, even when two books carry it. The token count at the bottom follows the same budget.
- **Text.** `{{char}}`, `{{user}}` and the other macros work inside an entry.
- **Seeing it.** `/prompt` shows what was added to the next reply.
- **Editing a card's book.** `/lore` does not change a card. Edit the card in SillyTavern, or the `character_book` in the card's file. The card is read when the chat is opened. `/char` and Card save keep the book as it is. To work on it in tarpi, copy it into a lorebook of your own with `c` in `/lore` (below).

**Asked on import.** When `/import` adds a card that has a lorebook, a small window asks whether to import it and link it to the character.

- **Yes** (Enter or `y`) copies the book into your own lorebooks (`/lore`) and puts the character in its *On for* list, so it is on for that character right away.
- **No** (Esc, `n` or `q`) leaves it. `c` in `/lore` copies it later.
- Left / Right, Tab or `h` / `l` move between the buttons, and a click on a button answers.
- It is not asked when you already have a copy of this card's lorebook. A copy remembers its character, so a book of the same name that came from another character does not count (only older copies, which remember no character, are recognised by their name).
- If another window is open when the import finishes, the question waits until it is closed.
- While a copy of a character's own book (one made this way or with `c`) is on for that character, the card's own copy is left out of the prompt, so the entries are not sent twice.
- A copy keeps being used as it is even when the card's own book changes later (the card is imported again, or its book edited). `/lore` marks such a copy `(card changed)` and `/import` says so when it imports the card again. To take over the card's version, delete the copy (`d`) and copy the card's book again (`c`).

### Your own lorebooks (`/lore`)

`/lore` opens the lorebook manager: books of tarpi's own, kept as one file each in `~/.config/tarpi/lorebooks/` (a V2 `character_book` plus a small `extensions.tarpi` block, so other tools can read them). They work like a card's book, with the same keys, positions, order and budget as above, and they are used in the prompt together with the card's. Saved books apply from the next reply.

There are three screens: the list of books, a book (its settings and entries), and an entry.

- **Books.**
  - `n` new
  - `i` import (SillyTavern World Info, a V2 lorebook, or a card's `.json` / `.png`; Tab completes the path)
  - `c` copy the lorebook inside one of your characters (the list leaves out the ones you already copied)
  - Space switches the whole book `[x]` / `[ ]` (off, it is never used, whatever its own settings say; "On for every chat" is a setting inside the book)
  - `D` copy (the copy starts on for nobody: it does not keep the original's characters or *On for every chat*, and it is not a copy of any card)
  - `x` / `X` export (V2 / SillyTavern); a folder gets `<name of the book>.json`, and an existing file is never overwritten
  - `d` delete
- **Book settings.** *On for every chat* (Space or Left / Right switches it), *On for*, *Scan depth* and *Token budget*. **On for**: Enter opens a list of all your characters with a switch each (Enter or Space switches, Esc closes), so a book can be on for chosen characters only (the row is greyed out and does nothing while *On for every chat* is on). Left / Right adds or removes the character of the current chat. A character that no longer has a card stays in the list ("no such card") so it can be removed.
- **Entries.** `n` adds one (it opens empty: Enter on *Keys* or *Text* fills them in), Space switches it on / off, `D` copies, `d` deletes. An entry has a Title, Keys, Secondary keys, *Secondary logic*, *Always on*, *Case sensitive* (off by default), Position, Depth and Role (only for an entry in the chat), Order and Text.
- **Keys.** Keys are separated by commas. A key that has a comma in it is shown, and typed, in quotes: `"Smith, John", dragon` is two keys (a quote inside a quoted key is doubled). Spaces inside a key are kept as they are, so opening a key window and accepting it changes nothing.
- **File names.** A book is any `*.json` file in the folder, whatever its name (spaces and dots are fine). A file whose name cannot be used (it starts with a dot, or has a `/`, `\` or `:` in it) is listed as a problem at the top of the list instead of failing when you open it.
- **Changed elsewhere.** `/lore` works on the file as it is on disk. A book that another device deleted cannot be opened or switched, and a save is refused with a message instead of bringing the book back.
- **Changing a value.** Rows shown as `‹ value ›` (Scan depth, Token budget, Position, Depth, Role, Order) change with Left / Right (Scan depth and Depth by 1, Token budget by 100, Order by 10). Enter on a number lets you type one instead.
- **Saving.** `s` saves, `u` undoes the last change, `q` goes back (asking about unsaved changes).
- **Spelling.** With `spellcheck: true`, the Description of a book and the Text of an entry are spell checked like the chat box (red underline, **Tab** on a marked word opens the suggestions; see [Configuration](#configuration)). Titles and keys are not. The words of a book's name, its entry titles and keys, and the names of the character and your personas count as spelled right, so the names and terms of your world are not marked.

**Intentionally minimal.** The number fields (Scan depth, Token budget, Depth, Order) only take digits, and Order a leading `-`. Any other character you type or paste is dropped, so there is no way to type a wrong value. The same limits as a card's book apply on purpose: keys are plain text, one scan depth and token budget for the whole book, no per-entry scan depth, and no regex, recursion, probability, sticky / cooldown / delay or inclusion groups. These are left out to keep the manager small, not forgotten. When a SillyTavern World Info file is imported, the entries that use any of these (and per-entry whole-word matching, a character filter, triggers, "ignore budget" or the recursion flags) are counted in the notes shown after the import, for example "3 limited to certain characters (now on for every character that uses the book)". The settings themselves stay in the book file and go back out with `X`.

---

## Vector memory

*In plain words: when a chat gets long, tarpi can dig out old messages that match what you just said and add them to the prompt. `/memory` is where you control it, and the only place.*

Off by default. Type `/memory` to open a window of rows; change a row with Left / Right (or type digits on a number row), then Ctrl+S to save (`:wq` saves and closes, Esc cancels).

| row | what it does | default |
| --- | --- | --- |
| Memory | on / off | off |
| Results | how many old messages come back (0 to 20) | 3 |
| Skip | the latest this-many messages are not searched, because they are normally still in the prompt (the two messages your search is made of are never searched, so 0 and 1 act as 2) | 12 |
| Match | how alike a message must be to come back, in percent: higher is stricter | 15 |
| Depth | where the memories go: this many messages from the end of the chat (0 = after the last message) | 4 |
| Role | who the memory note is said by: system, user or assistant | system |
| Template | the text that is put into the prompt; `{{result}}` is where the recalled messages go, one per line (macros like `{{char}}` and `{{user}}` work too) | `[Relevant memories from earlier in this chat:` `{{result}}]` |

For example, this template:

```
MEMORY START

{{result}}

MEMORY END
```

If the template has no `{{result}}`, the messages are added on a new line after it. An empty template is the default one.

There is no setting for it in `config.yaml`, not even the on / off switch. The window saves everything in `~/.config/tarpi/vector-memory.yaml` (shared between devices by `tarpi serve` / `tarpi connect` like your other data). Deleting that file turns it off and restores the defaults.

Before each reply tarpi turns every message of the chat (except the latest *Skip* ones) into a vector: a hashed bag of words, so it works offline with no model and no extra download. It compares them with your latest two messages (cosine similarity) and adds the best matches, in story order, as one note built from the Template. Each memory is cut at 600 characters. Nothing is stored: the chat file is the memory, and the `/prompt` window shows what was added.

Only the text of a message is compared, not who said it, so a question that names a character does not pull in everything that character said. A curly apostrophe counts as a straight one and `dragon's` counts as `dragon`.

Limit: it matches words, not meaning, so "car" does not find "automobile".

## Importing characters

*In plain words: type `/import` and then a file, a link, or press Tab to browse. Then `/chars` to switch to the new character.*

`/import` adds a character card. Type it in the input box of a chat, followed by what to import:

```
/import tarpi.png                                    # V2/V3 card PNG
/import ~/Downloads/tarpi.json                       # card JSON (V1, V2, V3, Pygmalion)
/import /path with spaces/tarpi.png                  # quotes or \ before spaces also work
/import https://chub.ai/characters/<creator>/<name>  # chub.ai (also characterhub.org)
/import https://pygmalion.chat/character/<id>        # pygmalion.chat
/import https://example.com/cards/tarpi.png          # direct link to a card .png or .json
/import {"name": "Tarpi", ...}                       # JSON given directly
```

**Paths** are read like a shell would: `~` is your home folder, a relative path starts from the folder tarpi was started in, and quotes, `\ ` (a backslash before a space, as a terminal pastes a dragged file) and `file://` links are understood. `/import` with nothing after it shows what it takes. After `/import ` press Tab to browse (see [Path menu](#path-menu-import)).

**Where it goes.** The card is saved as it came into `~/.config/tarpi/characters/`, named after the character (`Tarpi.png`, or `Tarpi-2.png` when that name is taken; a PNG stays a PNG, JSON stays JSON).

**Checked first.** Before saving, tarpi checks that the input really is a card (a PNG with an embedded `chara`/`ccv3` chunk, or JSON with `spec`, `name` or `char_name`) and says so if not.

**Links.**

- Any other http(s) link is treated as a direct link to a card file. tarpi downloads it itself (up to 50 MB; the file type is detected from its content, not the URL) and runs the same check.
- chub.ai (also characterhub.org) and pygmalion.chat links (`/character/<id>`) are downloaded by tarpi straight from those sites. For chub.ai it tries the card PNG on its image host, then its download endpoints one after the other (chub changes them now and then). For pygmalion.chat it uses its export endpoint.
- When every attempt fails, each one's reason is listed. **Downloading the card PNG in a browser and importing the file always works.** No other program is needed.
- Lorebook links are refused.

**While it runs.** The import runs in the background: the status row shows `importing… 5s` while a download is going, and you can keep chatting. When it is done, the status row says `imported "Tarpi" (Tarpi.png)` (or why it failed, on one row). Then `/chars` lists the new character. One import runs at a time, and leaving the chat (another character, quitting) cancels one that is still downloading.

Before a chat is open, `/import` is not available. With no character yet, `tarpi` makes the default `Tarpi` character, so there is always a character to open `/import` from.

### Importing and deleting while connected

*In plain words: if you use `tarpi connect`, nothing is different. Import and delete work the same, and the sync passes the change on.*

On a device that uses `tarpi connect`, the characters folder *is* the local copy of the server's data (see [Sharing between devices](#sharing-between-devices-serve-and-connect)). So `/import` and the delete in `/chars` change that copy, and the running sync passes it on at once (a final sync also runs when you quit).

- The server's log shows `added character Tarpi (uploaded)`, `deleted ...` lines and, from the device itself, `imported character Tarpi (Tarpi.png)` / `deleted character Tarpi and its 3 chats`. Every other connected device has the change at its next sync.
- A card name that the server already has is noticed, because the copy is brought up to date before the screen opens (you get `Tarpi-2.png`).
- Two devices importing the same new name at the same moment end up as a conflict: the newer file wins and the older one is kept beside it.
- Without `tarpi connect`, `/import` and the delete change this device's own `~/.config/tarpi/` only.

### Copying everything from SillyTavern

`tarpi import-st <url>` (see [Importing from SillyTavern](#importing-from-sillytavern)) also copies every character of that server into the characters folder, as its card PNG with the same file name, and copies its chats into the chats folder (see [Chats](#chats)). A character that is already here is skipped (`--overwrite` replaces it).

---

## Deleting characters

*In plain words: `/chars`, then `d`, then `y`. This deletes the character and all its chats, and it can't be undone.*

`/chars` opens the window with every character. Press `d` on one and it asks `Delete 'Tarpi' and its chats? y = yes, other = no`. **Only `y` says yes; Enter does not.** Then the card file is deleted from the characters folder, and the character's folder in `chats/` goes with it (the `_before-summary` copies too). The window stays open with a note saying what was deleted (`deleted Tarpi and its 3 chats`), and the list is shorter.

When the character you delete is the one you are chatting with, the question adds `(quits tarpi)`. After the `y`, tarpi stops the reply that is being written, deletes everything and exits, printing `Deleted "Tarpi" and its 3 chats.` In a `/one-shot` chat a reply that is being written can't be stopped, so the open character can't be deleted until it is done.

**This cannot be undone.** With `tarpi connect`, the files go from the local copy and the sync deletes them on the server too (see [Importing and deleting while connected](#importing-and-deleting-while-connected)). A file that was changed on the other side in the meantime is never deleted, so a chat another device wrote just before is kept in `chats/` (not shown while the card is gone; importing a card of the same name shows it again).

---

## Phone / Termux (touch)

*In plain words: tarpi works on a phone in Termux with touch. No setup needed.*

tarpi notices it runs on Android.

- **Tap a message** to edit it, like a click on desktop.
- **Tap the `‹2/3›` counter** in the last reply's header to change swipes: the left half goes to the previous one, the right half to the next (past the last generates a new one, like →). ←/→ still work.
- **Tap the text box** to open the keyboard. For that moment tarpi stops asking the terminal for mouse events, and turns them back on by itself as soon as the keyboard is up. If the keyboard doesn't show on the first tap, tap the box once more.

Enter sends. For a new line, use the Termux extra-keys row: ALT then Enter, or CTRL then J. ESC on that row leaves INSERT mode, as does typing the NORMAL-mode keys (`qq` by default) quickly.

---

## Build

```
cargo build --release
cp target/release/tarpi ~/.local/bin/
```

Requires a recent stable Rust toolchain (get it from [rustup.rs](https://rustup.rs)). The first build takes a few minutes.

---

## Configuration

*In plain words: settings live in one file, `~/.config/tarpi/config.yaml`. Type `/config` in a chat to edit it. Every setting is optional.*

The first run creates the file with the defaults, fully commented. A `config.toml` with the same keys is still read when there is no `config.yaml`.

```yaml
# Rough chars-per-token used to trim old history to the context window
# chars_per_token: 3.2

# Auto-close quotes, *asterisks* and brackets while typing in the input box
# autopair: true

# The menu of /commands above the input box (see "Command menu")
# command_menu: true

# Spell checking in the input box (off by default)
# spellcheck: false

# Sharing between devices (see "Sharing between devices")
sync:
  # serve_url: 192.168.1.20:7878
  serve_trust_all: false
  encrypted: true

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

msg_seperator:
  # true: a faint line under every message. false: only an empty row
  enabled: true
  # The character the line is made of: exactly one character, in quotes
  char: "╌"
```

**What each setting does:**

- **`chars_per_token`**: how many characters tarpi assumes make one token when it has no exact count. It uses this to decide how much old history fits.
- **`sync:`** holds the sharing settings. **`serve_url`** is the address `tarpi connect` uses (`host:port`). **`serve_trust_all: true`** (read by `tarpi serve` only) means no token is needed (default `false`: the token is required); `tarpi connect` asks the server and adapts. **`encrypted`** (default `true`) encrypts the link; set it the same on both devices. See [Sharing between devices](#sharing-between-devices-serve-and-connect).
- **`command_menu: true`** (default) opens the `/command` menu above the input box while you type a slash (see [Command menu](#command-menu)). `false` turns it off: nothing opens, Tab / Up / Down / Esc keep their old meaning (Up / Down scroll the chat), and Enter runs exactly what you typed. The command history (Up on a bare `/`) still works.
- **`autopair: true`** (default) closes `"`, `'`, `*`, `` ` `` and `(` `[` `{` as you type them, in INSERT mode and in the `/char` window. `"` gives `"|"`, `'` gives `'|'`, `*` gives `*|*`, `` ` `` gives `` `|` ``, and a second `*` inside an empty pair gives `**|**`. Typing the closing character steps over the one already there, and Backspace between an empty pair removes both. It stays out of the way next to words (`2*3`, `5"`, a backtick after a word, apostrophes like `don't` or `Tarpi's`), when the paragraph already has an unclosed one (so the second `"` closes), and when the next character is a letter. An apostrophe at the start of a word (`'em`, `'til`) opens a pair like any quote; Backspace removes it while it is still empty. **Pasted text is never changed.** Set `autopair: false` to turn it off.
- **`spellcheck: true`** turns on offline spell checking in the chat input box and in the `/char`, persona and `/lore` windows (default `false`). The Name fields are not checked, and in `/lore` only a book's Description and an entry's Text are. Misspelled words get a red underline (the red of the terminal's theme; the text keeps its colour). Words of one or two letters are checked too (`ho` and `x` are marked; `a`, `I`, `of`, `up` are not). The word you are still typing is not marked until the cursor leaves it. The word list is English and built into the program, so no internet or system dictionary is needed. It includes common roleplay and chat words (`yandere`, `waifu`, `tiefling`, `smirked`, `ooc`, `npc`, ...), kept in `tools/gen_dict.py`. **Extra words** go in `~/.config/tarpi/dictionary.txt` (one per line, `#` starts a comment). The character's name and your persona's name are always accepted. Details: `src/spell.rs`.
- **Fixing a misspelling.** With spell checking on, press **Tab** with the cursor in a marked word to open a suggestion menu: above the input box in the chat (like the command menu), and at the text cursor in the `/char`, persona and `/lore` windows. The menu never opens by itself. It lists up to six spellings, best first (a swapped pair such as `teh` → `the`, then a missing letter such as `helo` → `hello`, then other close words), and keeps a capital first letter. It works in INSERT and NORMAL mode. Tab / Down go to the next entry, Shift+Tab / Up to the previous one, Enter puts the highlighted one in, Esc or Ctrl+C hides the menu (and does nothing else), and a click or tap on an entry puts it in. Any other key closes it.
- **`vim.enabled: false`** removes the modes and the mode tag under the box; the box is plain text editing. Left/Right arrows move the cursor once the box has a draft (on an empty box they switch swipes). Home/End and Alt+Left/Right (word jump) move the cursor, and Up/Down move through a multi-line draft (with a one-line draft they scroll the chat as before).
- **`vim.normal_mode_keys`** can be any sequence of two or more characters, for example `jk`. It is typed quickly in INSERT mode (within about 0.6 s) and the keys are removed again.
- **`vim.leader`** is the `<leader>` key (default `space`; one character that is not a letter or digit, such as `,` or `\`, or the word `space`). See `<leader>y` below.
- **`msg_seperator`** (the spelling `msg_separator` works too) is the row between two messages. With `enabled: true` (default) it is a faint line made of `char` (default `╌`). With `enabled: false` it is an empty row, so messages keep the same spacing and only the line goes. `char` must be exactly one visible character (a full-width one such as `｜` is fine and takes two columns). **Put it in quotes**, since YAML reads a bare `-`, `#`, `*` or `'` as something else. It is not checked while `enabled` is `false`. There is no line under the very last message. `/config` applies a change to the open chat at once. An existing `config.yaml` is not rewritten: add the section by hand (the first-run file already has it).
- **`mouse.open_keyboard_on_tap: false`** stops tarpi from switching mouse reporting off when you tap the box, so the keyboard does not open by itself on a phone. **`mouse.tap_to_move_cursor: false`** keeps taps from moving the cursor. The two are independent.

`tarpi import-st` talks to a SillyTavern server that needs no login (the default single-user setup). Accounts and basic auth are not supported. It is the only command that contacts one.

---

## Keys

*In plain words: the keys for everyday chatting. The vim keys come after.*

The row under the input box shows the totals on the right: the number of messages and the token count of the whole prompt (system prompt, character card, persona, example dialogue, instruct formatting and every message, **not** trimmed to the context window). Compare it with your context size to see when old messages start to drop out.

The count is exact when the API connection's own tokenizer (llama.cpp's `/tokenize`) has answered. It is asked in the background whenever the chat or your settings change. Until the answer arrives, or when the backend can't be reached or there is no connection, the number is an estimate (characters / `chars_per_token`) and shown as `~1.2k`. While a reply (or a summary) is being written, the row is left as it was and the count is updated once the reply is done. Backends with no tokenizer route (OpenRouter, OpenAI) can't be asked; after each reply the row shows the `usage` they reported, until the chat changes.

| Key | What it does |
| --- | --- |
| Shift+Enter (also Alt+Enter, Ctrl+J) | New line. Shift+Enter needs a terminal with the kitty keyboard protocol (kitty, WezTerm, foot, Ghostty, Alacritty, recent iTerm2). Elsewhere use Alt+Enter or Ctrl+J. |
| Enter, or click **Send** | Send. An empty box adds no message; it just asks the character to reply. |
| Esc | Hide the menu above the input box when one is open (command, path or spelling menu). Otherwise: stop generating (the partial reply is kept) / cancel an edit. |
| Ctrl+K, `/continue` or `/c` | Continue the last reply (appends to it). |
| Click in the input box | Move the cursor there (also while editing a message). |
| Click a message | Edit that message in the input box, in INSERT mode (Enter / **Save** saves, Shift+Enter adds a line, Esc cancels; the cursor starts at the end of the text; your unsent draft comes back). |
| Ctrl+S, `:w` (vim modes on), or the **Card save** button | Only while editing the character's greeting (the very first message, at a swipe that came from the card): also writes the edit into the character card itself, as `first_mes` or the matching alternate greeting. It is permanent and new chats get it. **Save** / Enter only changes this chat. |
| Ctrl+R or `/regen [nudge]` | Regenerate the last reply (`/regen make her more guarded` steers just this reply). The new reply replaces it and wipes all its swipes (it ends up with a single one). Swipes are only added by actually swiping: ←/→ past the last swipe, a tap on the `‹n/m›` counter, or a sideways drag. |
| Ctrl+P | Edit the character's very last reply (opens in the input box like a click on it; Enter saves, Esc cancels). |
| Ctrl+O | Edit your own very last message the same way. |
| ←/→ | Previous / next swipe (→ past the last generates a new one). Only while the input box is empty and no message is being edited; with a draft in the box they move the text cursor. In a new chat, while the greeting is the only message, they switch between the card's alternate greetings instead (wrapping around). Older chats get the card's alternate greetings too. |
| PgUp/PgDn, mouse wheel, ↑/↓ | Scroll the chat (↑/↓ move the cursor while editing). The wheel over the text box scrolls only the box, anywhere else only the chat. Typing or moving the cursor snaps the box back to the cursor. |
| `/undo [n]` or `/u [n]` | Delete the last message, or the last `n` (`/undo 3`). Stops at the first message of a branched chat. |
| `/new` | New chat with this character (keeps the current one). |
| `/newchat` or `/nc` | Delete this chat and start a new one. |
| F1 or `/help` | Help window. Scroll it with ↑/↓ (or j/k), PgUp/PgDn, Home/End or the mouse wheel. Any other key closes it. |
| Ctrl+C / Ctrl+D | Quit (Ctrl+C only hides the menu above the input box when one is open). |

### Vim motions in the input box

*In plain words: the message box can work like vim. If you don't know vim, you can ignore all of this, or turn it off with `vim.enabled: false`.*

The box starts in INSERT mode (normal typing), also when you open a message to edit. Type `qq` quickly (configurable) to go to NORMAL mode. The mode shows under the box. Enter sends in INSERT and NORMAL mode. Shift+Enter adds a new line in INSERT mode.

| Keys | What they do |
| --- | --- |
| `i` `a` `I` `A` `o` `O` | Back to INSERT (at / after cursor, line start / end, new line below / above). |
| `h` `j` `k` `l` `w` `W` `b` `B` `e` `E` `0` `^` `$` `gg` `G` | Move (counts work: `3w`; capitals are WORD motions). |
| `x` `X` `r<c>` | Delete char / char before / replace char. |
| `dd` `cc` `yy` `D` `C` `S` `s` | Line and to-end-of-line operations. |
| `d` `c` `y` + `w` `W` `e` `E` `b` `B` `$` `0` `^` `h` `l` | Operator + motion (`dw`, `cW`, `y$`, ...). |
| `d` `c` `y` + `i` / `a` + an object | Text objects: `iw` `aw` `iW` `aW` (word), `i"` `a"` `i'` `a'` ``i` `` ``a` `` (quotes, on the line), `i*` `a*` (the `*narration*` around the cursor, within the paragraph), `i(` `a(` `ib` `ab` / `i[` `a[` / `i{` `a{` `iB` `aB` / `i<` `a<` (brackets, nested, over several lines). `ci"` changes the text inside the quotes, `da(` deletes a bracket pair with its brackets, `yiw` yanks a word. `i` is the inside, `a` also takes the marks (and the spaces after a word or quote). When the cursor is before a quote pair on the line, the next pair is used. |
| `Y` | Yank the line (same as `yy`). |
| `gg` `G` (in the `/char`, persona, system prompt and note windows) | First line of the first field / last line of the last field of the whole window, not just of the field being edited. |
| `<leader>y` + motion, `<leader>yy`, `<leader>Y` | The same yanks, but the text also goes to the **system clipboard** (a count goes before the leader: `3<leader>yy`). |
| `v` `V` | VISUAL mode (characters / whole lines). |
| `p` `P` | Paste after / before. |
| `u` | Undo. A count goes back that many steps (`3u`). |

`y` only fills vim's own register (for `p` / `P`). `<leader>y` also sends the text to the system clipboard through whatever the machine has (`wl-copy`, `xclip`, `xsel`, `pbcopy`, `termux-clipboard-set`, `clip.exe`) and falls back to the terminal's OSC 52 escape, which also works over SSH (in tmux: `set -g set-clipboard on`). The status line says what was copied and how. A linewise yank (`yy`, `V`) ends in a newline, like vim's `"+yy`.

**Undo steps.** Everything typed in one INSERT session is one step (like in vim), and every NORMAL-mode change (`x`, `dd`, `p`, `:s`, ...) is one. These are steps of their own, so `u` takes back just that: picking a spelling suggestion (Tab menu), pasting text, emptying the box with Ctrl+U, and Backspace / Delete / Ctrl+W / Ctrl+J typed in NORMAL mode. Text typed after one of them is the next step. `u` skips steps that changed nothing and does nothing once the history is used up. The history is cleared when the box is emptied by sending a message or when a different message is opened for editing. In the `/char` and persona windows every field keeps its own history while you move between fields.

**VISUAL mode.** Every motion above extends the selection (shown in reverse video). Then:

| Keys | What they do |
| --- | --- |
| `d` `x` | Delete the selection (`D` `X`: whole lines). |
| `y` `Y` | Yank the selection (`Y`: whole lines). |
| `<leader>y` `<leader>Y` | Same, and copy to the system clipboard. |
| `c` `s` | Change the selection (`C` `S` `R`: whole lines). |
| `p` `P` | Replace the selection with the register (`p` keeps the replaced text in the register). |
| `i` / `a` + an object | Select a text object (`viw`, `vi"`, `va(`, `vi*`, ... as in the table above). |
| `o` | Jump to the other end of the selection. |
| `j` `k` (after `V`) | Move by whole lines, as in vim, however many rows a long line is wrapped over (so one `j` always adds the next line). With `v` and in NORMAL mode they follow the wrapped rows. |
| `v` `V` | Switch between character / line selection. Pressing the active one leaves VISUAL. |
| Esc | Leave VISUAL mode (Esc again cancels an edit, as before). |

### Command line (`:`) and find / replace

`:` in NORMAL mode opens a vim-style command line on the bottom row (the box shows `COMMAND`). In VISUAL mode it starts with `'<,'>`, the selected lines. Enter runs it, Esc (or Backspace on an empty line) leaves, Ctrl+U clears it, Ctrl+W deletes a word.

| Command | What it does |
| --- | --- |
| `:%s/find/new/` | Replace the first `find` on every line. |
| `:%s/find/new/g` | Replace every `find`. |
| `:s/find/new/` | The current line only. |
| `:3,5s/...` `:.,$s/...` `:'<,'>s/...` | Line ranges (`N`, `.` current line, `$` last line, `%` everything). |
| `:5` | Go to line 5. |

- **Flags:** `g` all matches on a line, `i` ignore case, `I` match case (default).
- **Delimiter:** any character works (`:%s#a/b#c#`). `\/` is a literal `/`.
- **Replacement:** `&` is the found text (`\&` a literal `&`) and `\n` a line break. A missing replacement deletes the match (`:%s/foo`).
- **Preview:** while you type the pattern, every match that would be replaced is highlighted in the input box (black on yellow) and the bottom row shows how many there are (`no match` in red). As soon as you type the second delimiter (`:%s/foo/`), the box shows the text as it would look after the replacement, with the new text in green, and updates on every key. **Nothing is really changed until you press Enter;** Esc puts the original text back.
- **After:** the status line says `N substitutions on M lines`, or `E486: Pattern not found`. `u` undoes a whole `:s`.

The pattern is matched as plain text (no regular expressions yet), so `.` and `*` mean themselves.

---

## How it works

*In plain words: tarpi builds the prompt itself and sends it straight to your model. Nothing goes through any other server.*

Everything tarpi needs is its own: cards in `~/.config/tarpi/characters/`, personas in `persona/`, system prompts in `system-prompt.jsonl`, formatting in `formatting.yaml`, connections in `api.yaml` and chats in `chats/`. **Before every reply tarpi re-reads those files**, so a change to a template, persona, system prompt or sampler value applies to the next message.

1. Settings are built in memory from the files above. No server is asked.
2. The character card and chat are read from, and saved to, tarpi's own folders.
3. `src/textgen.rs` renders your context template (story string), instruct template, example dialogue and stop strings. `src/conn/` sends that prompt with the connection's own sampler values to the backend (llama.cpp's `/completion`, an OpenAI-compatible `/v1/completions`, or Ollama's `/api/generate`).
4. The stream is shown live, and the chat is saved back to the same `.jsonl` file.

---

## Limitations

- **Backends:** only text completion is supported, through a standalone connection (`/api`): llama.cpp, any OpenAI-compatible `/v1/completions` server (OpenRouter included) and Ollama's `/api/generate`. The chat completions endpoint is not used. No other server is contacted (apart from `tarpi serve` when you use `tarpi connect`, a SillyTavern server for `import-st`, and the link you give to `/import`).
- **Sync:** `tarpi connect` syncs whole files. There is no merging inside a chat, the link is encrypted by default (`sync: encrypted`, but without a token a server can't be told from an impostor), and an open chat is not reloaded when another device changes it.
- **Lorebooks** (a card's own, or your own in `/lore`) use plain-text keys and are deliberately minimal (see [Lorebooks](#lorebooks)). Vector memory is only a simple word match (see [Vector memory](#vector-memory)); no group chats or tool calling.
- **Token counts** used to trim history to the context window are an estimate (`chars_per_token`), not a real tokenizer. The totals in the status row are exact once the connection's tokenizer has answered (see [Keys](#keys)). Without a tokenizer route (OpenRouter, OpenAI) they are the `usage` the backend reports with each reply (prompt plus reply), exact until the chat changes.
- **Unfinished prose** is closed for you while a reply streams: an open `*narration`, `**bold` or `"speech` is shown with its closing marker added, and the model's own closer simply takes its place. Nothing is added for a lone trailing `*`, `2 * 3`, bullets, a `*` inside a word, or past a blank line. When a reply ends with markers still open, they are closed in the saved text too (Ctrl+K takes them off again before continuing).
- **Markdown** is minimal: `*narration*` is bold and a neutral grey, `**bold**` is bold, "quoted speech" is yellow and `` `code` `` has a slightly lighter background. Styles end at a blank line, and a `*` that has no partner (`2 * 3`, a `* ` bullet, an unclosed one) is shown as a plain asterisk.
- **Colours** come from your terminal's theme (see [Prose colours](#prose-colours)).
- **Mouse capture** is on (for the Send button and click-to-edit). Hold Shift to select text with the terminal's own selection.
