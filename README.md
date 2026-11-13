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
 tarpi 0.7.4  Tarpi/Tarpi01 • User • llamacpp@127.0.0.1:8080
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
<sub>illustrative</sub>

---

## features

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

## install

Needs a recent stable [Rust toolchain](https://rustup.rs).

```
$ cargo build --release
$ cp target/release/tarpi ~/.local/bin/
```

## quick start

```
$ llama-server -m model.gguf      # or Ollama, LM Studio, vLLM, OpenRouter ...
$ tarpi
```

1. **Enter** on the default `Tarpi` character to open a chat.
2. Type `/api`. A connection named `local` (llama.cpp at `http://127.0.0.1:8080`) is
   created for you. Set the address, press `t` to test it, `s` to save, `q` to go back.
3. Type `/import ~/Downloads/card.png` (or a chub.ai link) to add a character.
   Press **Tab** after `/import ` to browse folders.
4. `/chars` to switch to it. Write a message, press **Enter**.

## usage

```
tarpi                  # pick a character in a window, then chat
tarpi tarpi            # continue the latest chat with Tarpi
tarpi tarpi 3          # open chat number 3 (or by name: Tarpi03)
tarpi tarpi -n         # start a new chat
tarpi -l               # list characters         (tarpi tarpi -l: its chats)
tarpi import-st <url>  # copy personas, system prompts, characters from SillyTavern
tarpi serve            # share everything with other devices    (s)
tarpi connect [token]  # chat with the data of another machine  (c)
```

## commands

Type `/` in the chat for a menu; Tab / arrows pick, Enter runs.

```
/chats          open, create or delete chats
/chars          switch character (d deletes one)
/char           view and edit this character's card
/char-img       show the card's picture
/import <card>  add a character: file, chub.ai / pygmalion.chat link
/persona        choose who you are in the chat
/system         choose, create or edit the system prompt
/lore           lorebooks: write, import, choose which characters use them
/prompt         the exact prompt of the next reply
/note           this chat's Author's Note  (/an)
/api            backend address, model and sampler settings
/formatting     context and instruct templates
/config         edit config.yaml (applied when saved)
/new            new chat, keeping this one
/newchat        delete this chat and start a new one  (/nc)
/regen [nudge]  regenerate the last reply, optionally steered
/continue       continue the last reply  (/c)
/undo [n]       delete the last message, or the last n  (/u)
/summarize      summary of the chat as a new branch  (/summ)
/deletebranch   delete the branch that is showing  (/db)
/one-shot       lock this chat (permanent)
/help           every key and command  (/h)
/quit           exit  (/q)
```

## keys

```
Enter             send (an empty box asks the character to reply)
Shift+Enter       new line (also Alt+Enter, Ctrl+J)
Esc               stop generating / cancel an edit / close a menu
Ctrl+R            regenerate         Ctrl+K   continue the last reply
Ctrl+P / Ctrl+O   edit the last reply / your last message
Left / Right      previous / next swipe (empty box)
click a message   edit it            F1       help
qq                INSERT -> NORMAL (vim modes; off with vim.enabled: false)
Ctrl+C / Ctrl+D   quit
```

## backends

| kind | endpoint | notes |
| --- | --- | --- |
| llama.cpp | `/completion` | exact token counts from `/tokenize` |
| OpenAI-compatible | `/v1/completions` | vLLM, LM Studio, KoboldCpp, TabbyAPI, text-generation-webui, ... |
| OpenRouter | `/v1/completions` | pick a model from a filterable list |
| Ollama | `/api/generate` | sent raw, so your instruct template is used as is |

Text completion only: a model offered only through chat completions will not work.
Samplers, presets (Normal / Creative / Crazy), DRY, XTC, Mirostat and more are in `/api`.

## where things live

```
~/.config/tarpi/
├── config.yaml            settings (commented on first run; /config edits it)
├── api.yaml               backend connections
├── formatting.yaml        context and instruct templates
├── system-prompt.jsonl    system prompts
├── persona/               personas
├── characters/            Name.png / Name.json
└── chats/<Name>/*.jsonl   one folder per character
```

## sync between devices

```
# the machine that keeps the data
$ tarpi serve

# another device: put  serve_url: 192.168.1.20:7878  in config.yaml, then
$ tarpi connect <token>
```

Whole-file sync with a token over plain HTTP, so keep it on a network you trust.
Newer versions win; the older one is kept as `<file>.conflict-<time>`.

## docs

The full manual is [`docs/REFERENCE.md`](docs/REFERENCE.md). Good places to start:

- [API connection](docs/REFERENCE.md#api-connection-api) and its [OpenAI-compatible](docs/REFERENCE.md#openai-compatible-connections), [OpenRouter](docs/REFERENCE.md#openrouter-connections) and [Ollama](docs/REFERENCE.md#ollama-connections) kinds
- [Lorebooks](docs/REFERENCE.md#lorebooks), [Formatting](docs/REFERENCE.md#formatting), [Personas](docs/REFERENCE.md#personas), [Author's Note](docs/REFERENCE.md#authors-note), [System prompts](docs/REFERENCE.md#system-prompts)
- [Importing characters](docs/REFERENCE.md#importing-characters) and [Summarizing a long chat](docs/REFERENCE.md#summarizing-a-long-chat)
- [Sharing between devices](docs/REFERENCE.md#sharing-between-devices-serve-and-connect)
- [Keys](docs/REFERENCE.md#keys), [Vim motions](docs/REFERENCE.md#vim-motions-in-the-input-box) and [Configuration](docs/REFERENCE.md#configuration)
- [Phone / Termux](docs/REFERENCE.md#phone--termux-touch) and [Small screens](docs/REFERENCE.md#small-screens)

## limitations

- Text completion backends only (see above); no tool calling.
- Lorebooks: plain-text keys, no recursion, deliberately minimal (see `/lore` in the reference). No vector memory or group chats.
- Token counts used to trim history are an estimate (`chars_per_token`) until the
  backend's tokenizer or a reply's `usage` gives an exact number.
- Mouse capture is on; hold Shift to select text with the terminal's own selection.

## license

MIT
