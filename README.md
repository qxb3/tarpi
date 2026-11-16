```
 _                         _
| |_   __ _  _ __   _ __  (_)
| __| / _` || '__| | '_ \ | |
| |_ | (_| || |    | |_) || |
 \__| \__,_||_|    | .__/ |_|
                   |_|
```

**Roleplay from the command line.**

```
 tarpi 0.9.8  Tarpi/Tarpi01 • User • llamacpp@127.0.0.1:8080
────────────────────────────────────────────────────────────
 Tarpi
 *Spins around in the chair.* "Finally. I was about to start
 narrating my own life."

 User
 *Drops a bag on the desk.* "Can you help me plan a week of dinners?"

 Tarpi
 *Cracks knuckles.* "Fine. But you're eating a vegetable.
 Monday: pasta. Yes, tomatoes count."
────────────────────────────────────────────────────────────
 ┌──────────────────────────────────────────────────────────┐
 │ ❚                                                        │
 └ INSERT ──────────────────────────────────────────────────┘
                                      3 messages · 412 tokens
```

---

## what is this?

tarpi is a chat program that runs inside your terminal. You talk to a
**character** (a "character card", the kind you download from sites like
chub.ai), and an AI model writes the character's replies.

tarpi does **not** include an AI model. You bring your own: one running on your
computer (llama.cpp, Ollama, LM Studio, ...) or one you reach over the internet
(OpenRouter, ...). tarpi just talks to it.

## what you need

1. **A terminal.** Any one works. On a phone, use [Termux](https://termux.dev).
2. **Rust**, to build the program. Get it from [rustup.rs](https://rustup.rs) (one command, takes a minute).
3. **An AI model that tarpi can talk to.** The easiest way to start is
   [llama.cpp](https://github.com/ggml-org/llama.cpp) or [Ollama](https://ollama.com) on your own computer.

> **Important:** tarpi only works with backends that offer *text completion*.
> A model that is only offered through "chat completions" will not work.

## install

Open a terminal in the tarpi folder and run these two commands:

```
$ cargo build --release
$ cp target/release/tarpi ~/.local/bin/
```

The first one builds the program (it takes a few minutes the first time). The
second one copies it to a folder where your terminal can find it.

Check that it worked:

```
$ tarpi --version
```

If it says `command not found`, `~/.local/bin` is not on your PATH. Either add
it, or run the program straight from the build folder: `./target/release/tarpi`.

## quick start

**Step 1.** Start your AI model. For example, with llama.cpp:

```
$ llama-server -m model.gguf
```

(Using Ollama, LM Studio or OpenRouter instead? Skip to the
[backends table](#backends).)

**Step 2.** Start tarpi in another terminal window:

```
$ tarpi
```

**Step 3.** A window called `choose a character` opens. Press **Enter** on the
default character, `Tarpi`. A chat opens.

**Step 4.** Tell tarpi where your model is. Type `/api` and press Enter.

- A connection named `local` already exists. It points to `http://127.0.0.1:8080`
  (where llama.cpp listens by default).
- If your model is somewhere else, change the address.
- Press `t` to test the connection. You should see the model's name.
- Press `s` to save, then `q` to go back to the chat.

**Step 5.** Say something. Type a message and press **Enter**. The reply
appears word by word.

**Step 6 (optional).** Add your own character:

```
/import ~/Downloads/card.png
```

You can also paste a chub.ai link instead of a file path. Press **Tab** after
`/import ` to browse your folders. When it says `imported`, type `/chars` and
pick the new character.

## the commands you will use most

Type `/` in the chat and a menu opens. Use Tab or the arrow keys to pick one,
and Enter to run it.

```
/api            tell tarpi where your AI model is
/import <card>  add a character
/chars          switch to another character
/chats          open, create or delete chats with this character
/regen          get a different reply to your last message
/swipe [nudge]  get a new swipe of the last reply, optionally steered
/undo           delete the last message
/help           every key and command
/quit           exit
```

Everything else is in the [full command list](#all-commands) below.

## keys you will use most

```
Enter             send your message
Shift+Enter       new line in your message (or Alt+Enter, or Ctrl+J)
Esc               stop the reply that is being written
Ctrl+R            get a different reply
Ctrl+K            keep writing the last reply (continue)
Left / Right      flip between different replies (when the box is empty)
click a message   edit it
F1                help
Ctrl+C or Ctrl+D  quit
```

> **Vim fans:** the message box has vim modes. Type `qq` quickly to switch to
> NORMAL mode. If you don't know vim and this gets in your way, turn it off:
> put `vim: {enabled: false}` in your config (type `/config` to open it).

## start tarpi from the command line

```
tarpi                  # open the character window, pick one, then chat
tarpi tarpi            # continue the latest chat with Tarpi
tarpi tarpi 3          # open chat number 3 (or by name: Tarpi03)
tarpi tarpi -n         # start a new chat
tarpi -l               # list your characters         (tarpi tarpi -l: its chats)
tarpi import-st <url>  # copy characters from a SillyTavern server
tarpi serve            # share your data with other devices    (short: s)
tarpi connect [token]  # use the data of another machine       (short: c)
```

## what it can do

```
 characters   card PNG / JSON (V1, V2, V3, Pygmalion); import from a file,
              chub.ai, pygmalion.chat or any link; the card's lorebook is used
 backends     llama.cpp · OpenAI-compatible text completion · OpenRouter · Ollama
 prompt       built on your side: context + instruct templates (ChatML, Llama 3,
              Mistral, Gemma, ...), persona, system prompt, Author's Note
 chat         streaming, swipes, /regen with a nudge, /continue, /undo,
              click any message to edit it
 long chats   /summarize writes a summary branch; the original is kept
 editing      vim modes, :s find/replace, optional offline spell checker
 sync         tarpi serve / tarpi connect: same chats, characters and lorebooks on every device
 anywhere     desktop terminals, and Termux on a phone (touch)
 files        chats are plain .jsonl, one message per line: nothing is locked in
 migrate      tarpi import-st copies a SillyTavern server's characters, personas
              and system prompts
```

## all commands

Type these in the chat box. Type `/` to see the menu.

```
/chats          open, create or delete chats
/chars          switch character (d deletes one)
/char           view and edit this character's card
/char-img       show the card's picture
/import <card>  add a character: file, chub.ai / pygmalion.chat link
/persona        choose who you are in the chat
/system         choose, create or edit the system prompt
/lore           lorebooks: write, import, choose which characters use them
/memory         vector memory: recall older messages
/prompt         the exact prompt of the next reply
/note           this chat's Author's Note  (/an)
/api            backend address, model and sampler settings
/formatting     context and instruct templates
/config         edit config.yaml (applied when saved)
/new            new chat, keeping this one
/newchat        delete this chat and start a new one  (/nc)
/regen [nudge]  regenerate the last reply, optionally steered
/swipe [nudge]  new swipe of the last reply, optionally steered
/continue       continue the last reply  (/c)
/undo [n]       delete the last message, or the last n  (/u)
/summarize      summary of the chat as a new branch  (/summ)
/deletebranch   delete the branch that is showing  (/db)
/one-shot       lock this chat (permanent!)
/help           every key and command  (/h)
/quit           exit  (/q)
```

## backends

A "backend" is the program that runs the AI model. Pick the row that matches yours.

| kind | where it talks to | good for |
| --- | --- | --- |
| llama.cpp | `/completion` | llama-server on your computer. Gives exact token counts. |
| OpenAI-compatible | `/v1/completions` | vLLM, LM Studio, KoboldCpp, TabbyAPI, text-generation-webui, ... |
| OpenRouter | `/v1/completions` | models on the internet. You pick from a searchable list. |
| Ollama | `/api/generate` | Ollama on your computer. Sent raw, so your instruct template is used as is. |

To set one up: type `/api`, press `n` for a new connection, pick the kind, fill
in the address (and an API key if the server wants one), press `t` to test it,
`a` to use it for chats, and `s` to save.

Samplers (temperature and friends), presets (Normal / Creative / Crazy), DRY,
XTC, Mirostat and more are all in `/api`.

## where tarpi keeps your stuff

Everything is a plain file in `~/.config/tarpi/`. Nothing is locked away, and
you can back it up by copying the folder.

```
~/.config/tarpi/
├── config.yaml            settings (commented on first run; /config edits it)
├── api.yaml               backend connections
├── formatting.yaml        context and instruct templates
├── system-prompt.jsonl    system prompts
├── vector-memory.yaml     vector memory settings (/memory)
├── persona/               personas (who you are)
├── characters/            Name.png / Name.json
├── lorebooks/             your lorebooks
└── chats/<Name>/*.jsonl   one folder per character
```

## use it on more than one device

One computer keeps the data; your other devices connect to it.

```
# on the computer that keeps the data
$ tarpi serve

# on another device: put  sync: {serve_url: 192.168.1.20:7878}  in config.yaml, then
$ tarpi connect <token>
```

`tarpi serve` prints the token. The link is encrypted by default (`sync: encrypted: true`,
set it the same on both devices); with `encrypted: false` it is plain TCP, so use it only on
a network you trust. Whichever copy of a file is newer wins, and the
older one is kept as `<file>.conflict-<time>`.

## something is wrong?

| what you see | what to do |
| --- | --- |
| `no API connection` | Type `/api`, set the address, press `s` to save. |
| the top bar shows the connection in **red** | tarpi can't reach the model. Is the backend running? Press `t` in `/api` to test. |
| an error mentioning chat completions | Your model is only offered as a chat model. tarpi needs text completion. |
| replies are cut short or forget things | Raise *Context size* in `/api` (press `c` to copy the server's size). |
| `command not found: tarpi` | See [install](#install) above. |
| `encryption settings do not match` (on `tarpi connect`; `tarpi serve` logs `refused: encryption mismatch`) | `sync: encrypted` is set differently on the two machines. Set it the same (`true` is the default and the safe one) and restart `tarpi serve`. |
| can't select text with the mouse | Hold **Shift** while selecting. |
| Shift+Enter does nothing | Your terminal is old. Use Alt+Enter or Ctrl+J. |

## more help

The full manual is [`docs/REFERENCE.md`](docs/REFERENCE.md). Good places to start:

- [API connection](docs/REFERENCE.md#api-connection-api) and its [OpenAI-compatible](docs/REFERENCE.md#openai-compatible-connections), [OpenRouter](docs/REFERENCE.md#openrouter-connections) and [Ollama](docs/REFERENCE.md#ollama-connections) kinds
- [Lorebooks](docs/REFERENCE.md#lorebooks), [Formatting](docs/REFERENCE.md#formatting), [Personas](docs/REFERENCE.md#personas), [Author's Note](docs/REFERENCE.md#authors-note), [System prompts](docs/REFERENCE.md#system-prompts)
- [Importing characters](docs/REFERENCE.md#importing-characters) and [Summarizing a long chat](docs/REFERENCE.md#summarizing-a-long-chat)
- [Sharing between devices](docs/REFERENCE.md#sharing-between-devices-serve-and-connect)
- [Keys](docs/REFERENCE.md#keys), [Vim motions](docs/REFERENCE.md#vim-motions-in-the-input-box) and [Configuration](docs/REFERENCE.md#configuration)
- [Phone / Termux](docs/REFERENCE.md#phone--termux-touch) and [Small screens](docs/REFERENCE.md#small-screens)

## limits (things tarpi does not do)

- Text completion backends only (see above). No tool calling.
- Lorebooks are simple on purpose: plain-text keys, no recursion. Vector memory (`/memory`) is a simple word match, not real embeddings. No group chats.
- Token counts used to trim old messages are an estimate until the backend reports an exact number.
- Mouse capture is on. Hold Shift to select text with the terminal's own selection.

## license

MIT
