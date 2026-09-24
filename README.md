# 🧵 weftd

**A local-first scaffolding runtime for LLMs.** 
You control exactly what tokens go where.

![Status: alpha](https://img.shields.io/badge/status-alpha-orange.svg)

<!-- TODO: add with demo.gif — 15s, ~800px wide: send a message, fire a whisper, retrieval hits, metrics line 
![weftd demo](docs/assets/demo.gif)
-->

---

## ✨ Why weftd exists

LLMs respond to a prompt by taking the history of your chat and writing a response. `weftd` is about controlling the full prompt the LLM actually gets.

Most local-LLM frontends have: 
- write your message here
- write system-prompts here

`weftd` is about taking the prompt building process seriously.
It spawns llama.cpp's `llama-server` and talks to its native `/completion` endpoint, rendering the raw prompt string itself. 

The prompt is built from *sections* whose order you declare in TOML, including "retrieved memories" (from conversation history). How the prompt is formed affects both the answer of the LLM and the KV cache usage.

The configuration and living history of a character are one file. This includes the "current summary" and all "memories". The idea is to have a screenshot that you can read, diff, and correct anytime.

Two audiences:
- **Builders** who want a collaborative-fiction or roleplay setup they can reason about.
- **Researchers** who want to measure how retrieval, ordering, and compaction change what a model delivers.

---

## 🧩 Key features

### 🪡 **Raw prompt control**

- Native `/completion` endpoint (no chat templates, no Jinja, no silent reformatting)
- Per-family renderers: **ChatML** and **Mistral V7 Tekken** (strict `[INST]` alternation, zero-whitespace token rules)
- Multiple system messages at arbitrary depths
- `log_prompt = true` writes the exact string sent to the model, so you can verify every token

### 🧱 **Prompt assembly as a reorderable pipeline**

```toml
prompt_order = ["start_wrappers", "summary", "recent_history", "context", "directive", "end_wrappers", "whispers"]
```

- Each name is a slot filled at runtime; reordering is a config change
- Sections that happen to be empty are skipped
- Sections with content but unlisted warn (catching typos)
- Future task outputs register under their own name (add it to `prompt_order` and it lands where you put it)

### 📚 **Curated memory with retrieval pools**

- Memories are **hand-written TOML entries**, not scraped transcript chunks (no model errors compounding into your corpus)
- Dual BM25 index (one over content, one over keywords) with tunable weights: slide from classic keyword lorebook to pure semantic-ish content matching
- YAKE keyword extraction from recent turns builds the query
- **Named pools**, each with independent thresholds, result caps, and weights (behavioral directives don't compete against historical facts for the same slots)

### 🌬️ **Whispers and cues**

- `/whisper 3 <text>` injects an instruction for the next 3 turns, at the highest-influence position, then it disappears
- Never included in summarization
- **Cues** are whispers predefined in config and rendered as tappable buttons

### 🗜️ **Summarization that can't lose messages**

- Summary carries an `up_to` coverage marker; the recent-history window extends back to meet it
- Invariant: every message is either covered by the summary or present in the window
- Triggered by measured prompt-to-context ratio, not turn count
- Summary text and marker are editable live in the UI

### 🎚️ **The full sampler surface**

Temperature, min_p, top_k/top_p, DRY, XTC, explicit sampler ordering, stop sequences. Every field is optional so llama.cpp uses its own defaults.

### ⚡ **KV cache persistence**

- Slot save/restore via llama-server's HTTP API (skips prefill on session resume)
- Fingerprint file guards against model/context/quantization mismatch corrupting state
- Checkpointed around summarization so the next turn doesn't pay a cold-slot penalty
- Every cache operation is best-effort: stale prefill beats a corrupted cache

### 📊 **Per-turn observability**

`log_metrics = true` writes one line per turn: prompt-eval tokens and tokens/sec, generation timings, client-side time-to-first-token, wall-clock per pipeline stage, and `tokens_cached` / `tokens_evaluated` so you can see how much prefix the cache actually reused.

### ✏️ **Full transcript surgery**

Edit any message inline. Delete any message by index. `/retry`, `/continue`, and direct insertion of `system` / `user` / `assistant` turns. The backend is the single source of truth; the frontend is a pure view driven by one `sync` event.

### 📄 **The config file is the session file**

One TOML struct at every stage. A fresh config has a few opening messages; a saved session is the same file with hundreds. No separate session format, no export step, and it diffs cleanly in git.

### 📦 **One binary, no build step**

Rust backend, vanilla HTML/CSS/JS frontend embedded via `include_str!`. No npm, no bundler, no framework. `--dev` serves from disk for hot reload.

### 📱 **Reachable from your phone**

Binds `0.0.0.0` and prints Tailscale URLs at startup if the daemon is up. Optional pinned port for a stable bookmark.

---

## 🛠️ Tech stack

| **Backend** | **Frontend** | **Inference** |
| --- | --- | --- |
| Rust (tokio, axum) | Vanilla HTML/CSS/JS | llama.cpp (`llama-server`) |
| reqwest + SSE streaming | WebSocket, no build tools | Native `/completion` API |
| serde / TOML config | Embedded via `include_str!` | GGUF models, CPU or GPU |
| bm25 + yake-rust | | Slot save/restore for KV cache |
| clap, tracing | | |

---

<!-- TODO: capture these from a session using the neutral demo config 

## 🖼️ Screenshots

### 💬 **Conversation view**

![Conversation](docs/assets/screenshot-chat.png)

### 🧶 **Whispers, cues, and the summary panel**

![Steering](docs/assets/screenshot-steering.png)

### 📊 **Per-turn metrics**

![Metrics](docs/assets/screenshot-metrics.png)

---
-->

## 🚀 Quick start

### ⚠️ Requirements

- **Rust** to build, or grab a binary from [Releases](https://github.com/saona-raimundo/weftd/releases)
- **llama.cpp** on your `PATH` — `brew install llama.cpp`, or build it yourself
- **A GGUF model** Anything llama.cpp loads. weftd ships renderers for ChatML and Mistral V7 Tekken families.

### Install

```sh
git clone https://github.com/saona-raimundo/weftd.git
cd weftd
cargo install --path .
```

### Run

```sh
weftd models/demo.toml                   # start from a model config
weftd sessions/2026-01-15_14-30-28.toml     # resume a saved session
weftd --dev models/demo.toml             # serve static/ from disk for hot reload
weftd --no-open models/demo.toml         # skip the browser tab (phone access)
```

weftd spawns `llama-server`, polls until the model is loaded, then opens a browser tab. `Ctrl+C` saves the session and shuts down cleanly.

The repo ships `models/demo.toml`, a small generic scenario that exercises both retrieval pools, whispers, and cues, so you can see the whole pipeline working before writing your own config.

> **Note on the binary name:** an unrelated macOS window manager also installs a binary called `weftd`. If you have both, rename one on your `PATH`.

---

## 🧶 How the prompt is assembled

This is the core of the tool, so it's worth seeing plainly. Given the `prompt_order` above:

```
┌─────────────────────────────────────────┐
│  Start wrappers                         │  persona, rules, static lore
│  Summary                                │  compacted older history
│  Conversation history (recent window)   │  ← stable prefix, cached
├─────────────────────────────────────────┤
│  Retrieved memories (context pool)      │  changes every turn
│  Retrieved directives (directive pool)  │  behavioral, situational
│  End wrappers                           │  craft + behavioral rules
│  Whispers                               │  ephemeral, strongest position
└─────────────────────────────────────────┘
        ↓ render::render()
   raw prompt string (ChatML or Mistral V7 Tekken)
        ↓
   POST /completion
```

Everything above the line is identical between consecutive turns, so llama.cpp reuses it from the KV cache. Everything below changes and gets re-evaluated. Put dynamic content *before* history instead and you invalidate the prefix at that boundary every single turn — on CPU that's the difference between a constant-cost session and one that gets slower forever.

Content near the end also has more influence on generation. Those two facts point the same way.

---

## ⚙️ Configuration

A minimal config:

```toml
model = "/home/user/.local/share/models/example-model.gguf"
prompt_structure = "chatml"        # or "mistral_v7_tekken"
ctx_size = 8192
recent_turns = 5
kv_cache = true
log_metrics = false

[sampler]
temperature = 1.0
min_p = 0.05
max_tokens = 300

[[wrapper]]
position = "start"
content = "You are a collaborative fiction writer. Write vivid, restrained prose."

[[wrapper]]
position = "end"
content = "Two to four paragraphs. Stay in character."

[[memory]]
pool = "context"
content = "The market in Mossford is run by Pip, who knows everyone's secrets."
keywords = ["Mossford", "Pip", "market"]
priority = 2.0

[retrieval.context]
max_results = 3
keyword_weight = 0.5
content_weight = 0.5

[[cue]]
label = "Raise stakes"
content = "Something goes wrong in this scene. Don't resolve it."

[[message]]
role = "system"
content = "The tavern door creaks open. Rain hammers the cobblestones outside."

[[message]]
role = "assistant"
content = "A figure steps through, rain dripping from a heavy cloak."
```

---

## ⌨️ Commands

Typed with a `/` prefix, or triggered by UI buttons. Commands travel as ordinary text — the backend parses them, so new commands need no protocol changes.

| Command | What it does |
| --- | --- |
| `/whisper [N] <text>` | Inject ephemeral steering for the next N turns (default 1) |
| `/cancel <index>` | Cancel an active whisper early |
| `/system <text>` | Insert a visible, editable, persisted system message |
| `/user <text>` / `/assistant <text>` | Insert a turn without generating |
| `/retry [text]` | Discard the last response and regenerate; optional text becomes a 1-turn whisper |
| `/continue [text]` | Generate without a new user message |
| `/delete [index]` | Remove a message by index, or the last one |

---

## 🔒 Security model

**weftd has no authentication layer, by design.** It binds all interfaces so that Tailscale peers can reach it, and the tailnet is the trust boundary. Do not expose it to the open internet. If you don't use Tailscale, only localhost traffic reaches it on a typical home network — but that's a property of your network, not of weftd.

---

## 🚧 Not Yet Implemented

Stated plainly so you can decide whether this is usable for you:

- **Stop button** — generation streams but cannot be cancelled mid-flight
- **Session auto-save** — state is written on clean shutdown only
- **WebSocket reconnection** — a browser refresh drops the connection (backend state survives)
- **Entity/state tracking** — a structured world-state ledger; designed, not built
- **Generalized task DAG** — retrieval/summary/generation are hardcoded stages

---

## 🔮 Roadmap

- 🛑 Cancellable generation
- 🧮 Entity/state tracking as a per-turn async task
- 🕸️ TOML-declared task graph with dependency resolution and scheduling
- 🪞 Reflection task — patterns across recent exchanges, not just facts
- 💾 Periodic session autosave and WebSocket reconnection
- 📏 Benchmark harness for scaffolding ablations (ordering, budgets, retrieval configs)

---

## 🤝 Contributing

Issues, questions, and PRs welcome. This is a one-person project, so responses may be slow, and there are no stability guarantees before 1.0.

Please open an issue before a large PR.

```sh
cargo fmt
cargo clippy -- -D warnings
cargo test
```

Adding support for a new model family is the most self-contained contribution: add a `PromptStructure` variant and a rendering function.

---

### 🧵 On the name

In weaving, the *warp* is the fixed set of threads held under tension on the loom; the *weft* is the thread drawn across them, over and under, that actually makes the cloth. The model weights are the warp. Everything weftd does (assembling, retrieving, compacting, steering) is the weft.

The trailing `d` is for daemon: it's a long-running process that owns a child `llama-server`.

---

### Similar projects

- [PersonAi](https://github.com/0xAdafang/PersonAi) A local AI assistant to embody, chat with, and remember your characters.
