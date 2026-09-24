<!-- DESIGN.md -->

# Design Notes

Internal reference for architecture decisions, rationale, and implementation context. For usage and configuration, see README.md. For planned features, see the entity/state tracking design doc.

---

## Core Principle: Backend as Single Source of Truth

The backend owns all conversation state. The frontend is a pure view that renders whatever the backend sends and never maintains its own copy of the truth.

This wasn't the original design. The first implementation had two event paths: `"message"` (append a single message) and `"undo"` (pop two messages). The frontend maintained its own `messages` array, optimistically rendering user messages before the backend confirmed them. This caused several bugs:

- `/system` messages were invisible: the frontend had no local render path for system-role messages, and `generate()` only sent a `"message"` event for the assistant reply, not the pushed input.
- `/undo` assumed the last two messages were always a user+assistant pair, which broke after `/system` or `/continue`.
- The frontend's local state could desync from the backend if events arrived in unexpected order.

The fix was to collapse everything into a single `"sync"` event that sends the full message list. Every backend mutation (new message, edit, delete, retry) ends with `send_sync()`. The frontend replaces its entire state on every sync. The `"message"` event was removed entirely.

**Tradeoff:** the user's own message doesn't appear until the backend processes it. For fiction input, the backend pushes the user message to conversation and sends a sync *before* starting generation, so the user sees their message immediately, followed by the "Typing..." placeholder. For commands that trigger generation (`/system`, `/retry`), the same pattern applies — sync before generate, sync after.

**The `"thinking"` event** is the one exception to "sync is the only state mutation." It's a UI-only signal that shows a placeholder in the message list. It doesn't touch the `messages` array. The placeholder is removed by the next `sync` event.

## Command System

### Why Commands Are Text, Not Protocol

Commands travel as regular `{ "action": "submit", "content": "/delete 3" }` messages. The backend parses the `/` prefix inside the `Submit` handler. The frontend doesn't know commands exist — it just sends text, and buttons send the same strings.

This means zero protocol changes for new commands. The backend is the sole authority on what's a command vs. fiction input. Frontend buttons are cosmetic shortcuts.

### Routing: Connection → Execute → Effect

Command execution is split into two layers:

**`execute.rs`** contains pure logic. Each command handler (`execute_delete`, `execute_retry`, `execute_system`, `execute_cancel`, etc.) takes `&mut Conversation` and/or `&mut WhisperStack`, performs the mutation, and returns an `Effect` describing what the caller should do. No async, no socket, no LLM — fully unit-testable.

**`connection.rs`** is the thin async router. It locks the shared state, calls `execute::dispatch()`, drops the locks, then interprets the returned `Effect` by calling `send_sync()`, `generate_turn()`, `send_whispers()`, or `send_error()`.

The `Effect` enum:

```
Effect::Sync              — send full message list to frontend
Effect::Generate          — run LLM generation (with optional message to push to conversation first)
Effect::WhispersChanged   — send whisper list to frontend
Effect::Error(String)     — send error to frontend
Effect::Multi(Vec)        — sequence of the above (flattened, never nested)
```

`Multi` is flattened into a vec and iterated — no recursion. Rust's async fn can't be recursive without boxing, and our effects never nest, so this is simpler.

### `/delete` vs the old `/undo`

The original `/undo` called `pop_exchange()` which always removed two messages (assumed user+assistant pair). This broke when:

- `/system` produced a system+assistant pair
- `/continue` produced an assistant message with no preceding user message
- Any manual message deletion left an odd number of messages

`/delete [index]` is role-agnostic. It removes a single message at the given index (or the last message if no index). The underlying `Conversation::remove(index)` is a thin wrapper around `Vec::remove`. Per-message delete buttons in the frontend send `/delete {index}` directly.

### `/retry` Semantics

The old `/retry` popped both messages (user+assistant), then re-pushed the user message and regenerated. This had the same pairing assumption as `/undo`.

The new `/retry`:

1. Validates that the last message is `Role::Assistant` — errors otherwise.
2. Pops only the assistant message. The trigger (whatever is now last) stays in history.
3. Sends `sync` so the frontend reflects the removal.
4. Optionally adds a 1-turn whisper for steering.
5. Calls `generate()` — the trigger is already in history.

This correctly handles triggers of any role: user messages, system messages (from `/system`), or even assistant messages (if someone deletes a user message and retries).

### `/system` Visibility

System messages are pushed to conversation history, persisted in session files, and rendered in the frontend with distinct styling (italic, left border, dimmed). They're editable like any other message. This was a bug fix — the original implementation passed `None` as `user_message` to `generate()`, so the system message was used for prompt building but never stored.

Session persistence works automatically: `Role::System` has serde support, and `config.message` is `Vec<Message>` with no role filtering.

### Cues

Cues are pre-loaded whisper buttons defined as `[[cue]]` entries in the model TOML. They produce whispers through the existing `/whisper` command — no new backend actions, no new protocol verbs beyond the `"cues"` event sent on connect.

The frontend receives the cue list on connect, renders pill buttons, and compares each cue's content against the active whisper list to determine visual state. Tapping a cue sends `/whisper {turns} {content}` as a regular `Submit` action. If the cue's content matches an active whisper, the button dims and the tap is ignored.

Deduplication is purely frontend. The backend sees a normal `/whisper` command and has no concept of cues at runtime. `config.cue` is read-only after load — cues are config, not state.

## Turn Pipeline

Each turn is orchestrated by `generate_turn()` in `connection.rs`:

1. Snapshot conversation history.
2. Run pre-generation tasks (retrieval).
3. Call `generate()` with the history and task outputs.
4. Send `sync`.
5. Run post-generation tasks (summary threshold check).

`generate()` receives the pre-computed history and task outputs. Its job:

1. Send `"thinking"` event.
2. Build the prompt via `prompt::build()` — sections are assembled in `prompt_order` from the TOML config.
3. Render the prompt into a raw string via `render::render()` based on the model's `PromptStructure`.
4. Send the rendered string to the LLM via `/completion` (native llama.cpp endpoint).
5. Push the assistant reply to conversation.
6. Tick whispers.

Callers are responsible for:
- Pushing any input message to conversation before calling `generate_turn()`.
- Sending `sync` before `generate_turn()` (so the user sees their message).

This separation means `generate()` doesn't need to know which tasks ran or how their outputs were produced. It assembles a prompt from whatever it receives. Adding a new pre-generation task means adding one call in `generate_turn()` and handling the output in `prompt::build()`.

Summary monitoring lives in `summary.rs`. After each generation, `check_summary_thresholds()` computes the prompt-to-context ratio from the completion result's `tokens_cached` and `tokens_evaluated` fields. If `summary_config.alert` is set and the ratio exceeds the threshold, a `summary_needed` event is sent to the frontend. The `run()` function builds a summarization prompt, calls the LLM (non-streaming), and returns a `SummaryState` — reserved for future auto-summarization when `summary_config.prompt` is also set.

## LLM Client

### Why reqwest Instead of async-openai

The original client used `async-openai` with BYOT (Bring Your Own Transport) to call `/v1/completions`. This constrained the request body to OpenAI-spec fields and the response to OpenAI-spec format — no access to llama.cpp-specific sampler parameters (DRY, XTC, sampler ordering) or response fields (`tokens_cached`, `tokens_evaluated`).

weftd now uses a direct `reqwest` client hitting llama.cpp's native `/completion` endpoint. This gives full control over the request body (including `n_predict` instead of OpenAI's `max_tokens`) and returns llama.cpp-specific response fields needed for cache observability. The replacement is contained entirely in `client.rs`.

### Serving model

weftd spawns `llama-server` directly with `--offline --no-webui`, bypassing ramalama's container layer. 
The CPU-only setup gains nothing from containerization, and direct spawning gives full access to llama-server features like `--slot-save-path` for KV cache persistence. 
GGUF models are stored in `~/.local/share/models/` and referenced by absolute path in the TOML config.
`wait_until_ready()` polls `/health` rather than TCP connectivity — the port opens before the model finishes loading, and downstream operations (KV cache restore, generation) need the model fully loaded.

llama-server is given a random port at startup: weftd binds a `TcpListener` to `127.0.0.1:0`, reads the assigned port, drops the listener, and passes the port to `llama-server --port`. The bind-drop-pass pattern has a theoretical race window where another process could grab the port before llama-server binds it, but on a single-user laptop with ~28k ephemeral ports it's a non-issue — and llama-server would surface a clear "port in use" error if it ever happened. The chosen port is stored on `ServeHandle` and threaded through to `LlmClient` and `KvCacheManager`. There is no TOML field or CLI flag for the llama-server port; it is implementation detail.

### Streaming via SSE

Streaming uses `reqwest-eventsource`, which wraps a `reqwest::RequestBuilder` into a typed async stream of SSE events. The client builds a `CompletionRequest`, POSTs it with `stream: true`, and iterates the event stream:

- `Event::Open` — connection established, logged.
- `Event::Message` — an SSE `data:` line. If the data is `[DONE]`, the stream ends. Otherwise it's parsed as a `CompletionChunk` and the token text (`content`) is accumulated. Malformed chunks are logged and skipped.
- `StreamEnded` — the server closed the connection. Normal termination.
- Any other error — the stream is closed and the error propagated.

The accumulated text is sent to `connection.rs` via an mpsc channel as progressive `"token"` events, identical to the previous implementation. The client interface (`complete` and `complete_stream`) changed only in that both now take `&SamplerConfig` to build the request body.

Generation runs to completion — no cancellation. This avoids the entire partial message state machine: no half-written messages in history, no need to handle `/undo` mid-stream, no changes to whisper tick timing. The final `sync` event replaces everything with the authoritative state, same as before.

The `"token"` event is purely additive. A frontend that doesn't handle it still works — it ignores the events and renders the final `sync` normally.

### CompletionRequest and SamplerConfig

`CompletionRequest` contains `prompt`, `stream`, and a flattened `SamplerConfig`. The `#[serde(flatten)]` attribute inlines all sampler fields into the top-level JSON object, so llama.cpp sees a flat request body rather than a nested `"sampler"` key.

`SamplerConfig` uses `#[serde(skip_serializing_if = "Option::is_none")]` on every field. When serialized to JSON for the request body, `None` fields are omitted entirely — llama.cpp only receives parameters that were explicitly set in the TOML. This same struct deserializes from the `[sampler]` TOML section with `#[serde(default)]` on every field, so an empty or missing `[sampler]` section produces all `None` values.

This dual-purpose design means the sampler config flows directly from TOML → struct → JSON request body with no intermediate mapping or builder logic.

`CompletionRequest` also carries an `n_predict` field, translated from `SamplerConfig::max_tokens` at construction time. The native `/completion` endpoint uses `n_predict` (not `max_tokens`), so `CompletionRequest::new()` moves the value out of the sampler clone — `max_tokens` serializes as `None` (omitted) while `n_predict` carries the value. The TOML config retains `max_tokens` as the user-facing name.

### Profiling Infrastructure

`complete()` and `complete_stream()` return `CompletionResult` instead of `String`. This struct carries the response text, llama.cpp's `Timings` (prompt eval and generation speed, parsed from the `timings` object in the final response/chunk), client-side `time_to_first_token_ms` (measured via `Instant` in the streaming loop), and cache metrics (`tokens_cached`, `tokens_evaluated`) from the native `/completion` response. Together, `tokens_cached + tokens_evaluated ≈ total prompt tokens`, giving visibility into how much of the prompt prefix was reused from the KV cache.

Timings capture is always-on — serde parses a field that's already in the response body, and one `Instant` per stream is negligible. Pipeline instrumentation (`Instant` captures around retrieval, generation, and post-generation stages in `generate_turn()`) is gated behind `config.log_metrics`.

When `log_metrics = true`, `init_tracing()` adds a second tracing layer filtered to `target: "metrics"` at TRACE level, writing to `debug/metrics_{timestamp}.log`. The console layer never sees these events. When `log_metrics = false`, no file is created and no `Instant` captures execute. `TurnMetrics::log()` emits a single `tracing::trace!` per turn with structured fields covering both server-side decomposition (from llama.cpp) and client-side wall-clock timing.

## Network and Ports

### Per-Task Port Ownership

There is no global `port` field. Each task that needs an HTTP endpoint owns its own port. Today this means two ports: llama-server (always random, internal) and the frontend axum server (random by default, optionally pinned via `frontend_port` in the model TOML). When future tasks need their own model servers or HTTP endpoints, the same pattern applies — each task's config block owns its port.

This split solves a concrete problem with the previous single `port` field: it conflated two unrelated concerns and required a manual config edit to run two weftd instances simultaneously. With per-task random allocation, two instances on the same machine just work.

### Frontend Port Pinning

`frontend_port` in the model TOML is optional. When omitted, weftd binds to `0.0.0.0:0` and uses whatever port the kernel assigns. When set, weftd binds to that exact port — and exits with a clear error if it's occupied. There is no auto-fallback because the entire reason to pin a port is stability (e.g. a phone bookmark); silently choosing a different port would defeat that purpose.

Frontend binds to `0.0.0.0` (not `127.0.0.1`) so Tailscale peers can reach it. Tailscale's tailnet is the trust boundary — there is no auth layer in weftd itself, by design. If you don't have Tailscale, the listener is still bound to all interfaces, but practically only `localhost` traffic reaches it on a typical home network.

### Tailscale Detection

`tailscale.rs` is a thin wrapper around `tailscale status --json`. It returns one of three states:

- `Ok(Some(info))` — Tailscale running, IP and DNS name available
- `Ok(None)` — `tailscale` binary not found, silent skip
- `Err(message)` — Tailscale installed but unusable (daemon down, not authenticated, etc.)

The error path includes the specific fix command (`sudo systemctl start tailscaled`, `tailscale up`) rather than a generic failure message. The detection is best-effort: any failure prints a warning and proceeds with the local URL. There is no daemon management, no waiting loops, no sudo. Users who want Tailscale start it themselves; weftd just reports status.

## Opening Messages

Opening messages are `[[message]]` entries in the TOML config. 
In a fresh config, these are the opening messages — typically a scene-setting system message and the assistant's first response. 
In a saved session, the full conversation history lives in the same `[[message]]` array.

There is no injection logic in `connection.rs`. The conversation is populated from `config.message` in `main.rs` before `Connection` is created. On connect, `run()` sends `send_sync()` and `send_whispers()` so the frontend receives whatever state exists — two messages for a fresh config, hundreds for a resumed session. Same code path either way.

## Prompt Assembly

### Ordering and Influence

Position in the prompt determines influence. Content closer to the end has more weight with most models. The assembly order is configured via `prompt_order` in the TOML:

```toml
prompt_order = ["start_wrappers", "summary", "recent_history", "context", "directive", "end_wrappers", "whispers"]
```

`prompt::build()` populates a `HashMap<String, Vec<Message>>` keyed by section name, then iterates `prompt_order` to assemble the final `Vec<Message>`. Each section is consumed during assembly — sections not listed in `prompt_order` are logged as warnings and dropped. Sections listed but empty for a given turn are skipped silently.

The built-in section names are populated from existing config and runtime state. Future task outputs register under their task name — adding the name to `prompt_order` is the only config change needed to inject a new task's output.

This design decouples prompt ordering from the code. Reordering sections (e.g., moving memories before or after history for KV cache optimization) is a TOML change, not a code change. The same mechanism supports future task DAG outputs without modification.

Whispers are typically last because they're ephemeral steering that should override everything else. End wrappers reinforce persistent rules. Start wrappers set the overall persona. The current input is the last message in the recent history window — it is not a separate assembly step.

### Summarization

Lives in `summary.rs`. The summary replaces older messages in the prompt — they're still in the full conversation history, just not in the context window. The summary is stored as `SummaryState { text, up_to }` where `up_to` tracks how far into the conversation the summary covers.

Summary updates are triggered by prompt-to-context ratio, not turn count. After each generation, the backend computes `(tokens_cached + tokens_evaluated) / ctx_size` from the completion response. When `summary_config.alert` is set and the ratio exceeds the threshold, the frontend is notified. 
The summary panel in the UI exposes the summary text and `up_to` marker as editable fields — changes are sent via `summary_edit` and take effect immediately. The Autogenerate button triggers `summary_trigger`, which calls `summary::run()` directly against the current history. If the Automatic checkbox is enabled, autogeneration fires whenever `summary_needed` is received instead of showing the banner.

Whispers are excluded from the summarization prompt. They're steering, not narrative content. Wrappers can opt in via `in_summarize = true`.

### Recent History Window

`recent_turns` controls the minimum number of recent messages in the prompt. After the first summary, the recent window extends back to the summary boundary (`summary.up_to`) if that's further than `recent_turns` — ensuring no messages fall into a gap between what the summary covers and what the recent window shows. Before the first summary, the `recent_turns` cap applies normally.

This fixed a bug where messages between `summary.up_to` and `len - recent_turns` were invisible to the model — not covered by the summary and not in the recent window. 
The gap grew with each turn until the next summary update.

No token counting yet — the memory retrieval layer has its own token budget, and future entity state tracking will add another injection layer with its own budget.

### Prompt Rendering

After assembly, the `Vec<Message>` is rendered into a raw prompt string by `render::render()`. The `PromptStructure` enum (set in the model TOML) determines the format:

**ChatML** (`chatml`, default) — system messages can appear anywhere. Each message is wrapped in `<|im_start|>{role}\n{content}<|im_end|>\n`. The prompt ends with `<|im_start|>assistant\n` to trigger generation. Used by Qwen, Mag-Mell, and other ChatML-trained models.

**Mistral V7 Tekken** (`mistral_v7_tekken`) — single `[SYSTEM_PROMPT]` block at the top, strict `[INST]`/`[/INST]` user/assistant alternation. No whitespace between control tokens and content (Tekken tokenizer rule). System messages are handled by zone:
- Leading system messages (wrappers, summary) → consolidated into the top `[SYSTEM_PROMPT]...[/SYSTEM_PROMPT]` block, separated by `\n\n`.
- Trailing system messages (retrieved memories, end wrappers, whispers) → injected as `[OOC: ...]` annotations inside the final `[INST]` block, after the user's input.
- Mid-conversation system messages (from `/system` in history) → `[OOC: ...]` inside the nearest `[INST]` block.

Used by Magidonia, Cydonia, Magistral, and other Mistral Nemo/Small derivatives. The `[OOC: ...]` convention works because these models were fine-tuned on SillyTavern data where OOC annotations are used extensively for Author's Notes and steering.

Adding a new model family means adding a variant to `PromptStructure` and a rendering function in `render.rs`. Nothing else changes.

### Why weftd Renders Prompts Instead of Using Chat Templates

llama.cpp applies a Jinja chat template when using `/v1/chat/completions`. For ChatML models this works fine — system messages can appear anywhere. But for Mistral V7 Tekken models, the template only captures `messages[0]` as the system prompt, expects strict user/assistant alternation, and silently produces malformed prompts when given multiple system messages scattered throughout.

weftd's prompt architecture relies heavily on multiple system messages (8+ wrappers, summary, memories, end wrappers, whispers). Rather than fighting the template, weftd renders the raw prompt itself and sends it to llama.cpp's native `/completion` endpoint. This gives full control over token placement and structural constraints per model family.

The rendering is pure Rust — no Jinja, no template engine. Each `PromptStructure` variant has its own rendering function that handles both control tokens and structural transformation (consolidating system messages, converting mid-conversation systems to OOC, enforcing alternation).

## Memory / Retrieval System

### Why Not Index Session Messages

Small local models produce imperfect prose. Indexing raw session messages would
propagate errors into the retrieval corpus. Instead, memories are user-curated
entries in the model TOML — the user corrects and refines them between sessions.
The corpus is static for the session lifetime: built once at startup, no mutations.

### Dual-Index Design

Two BM25 indices over the same memories — one indexing content text, one indexing
the keywords field joined as a string. Both are scored at query time and combined
with configurable weights. This gives a tunable spectrum from classic lorebook
(keyword_weight=1, content_weight=0) to pure content matching (the reverse).

The reason for two indices rather than concatenating keywords into content: the
weight spectrum. Concatenation would make keyword influence proportional to how
many times terms repeat, not independently tunable.

### Query Pipeline

YAKE extracts keywords from the last few conversation messages (configurable
depth). These keywords query both BM25 indices. Scores are combined:

    final = kw_weight × keyword_bm25 + content_weight × content_bm25
          + priority_weight × normalized_priority

Results are filtered by minimum score threshold and capped by max results. Each pool's results are injected as a named section at the position specified by prompt_order.

### Named Retrieval Pools

Memories are partitioned into named pools, each with its own RetrievalConfig and RetrievalEngine. At startup, main.rs iterates config.retrieval (a HashMap<String, RetrievalConfig>), filters memories by their pool field, and builds a NamedRetrieval { name, engine } per pool. Pools with no matching memories or enabled = false are skipped.
Each turn, generate_turn() iterates the pools and collects results into a HashMap<String, Vec<Message>> — the task output bus. Pool names are section names in prompt_order. This is the same mechanism future tasks (reflection, entity tracking) will use to inject their output.
The split solves a concrete problem: behavioral directives ("when user deflects a question, you hold silence") and historical facts ("Day 9 karaoke") have different retrieval characteristics — different result caps, score thresholds, and weight balances. Independent pools with independent configs prevent keyword collisions from crowding out one category with another.

### Dependencies

bm25 v2 for the search engine, yake-rust v1 for keyword extraction. Both are
pure Rust with no inference cost.

## Session Persistence

The config file is the session file. `ModelConfig` carries everything: hardware settings, wrappers, memories, and runtime state (`message`, `summary`, `whisper`). On shutdown, `session::snapshot()` clones the config and overwrites the runtime fields with current state, then `session::save()` writes it as TOML to `sessions/{timestamp}.toml`.

There is no `SessionFile` struct. Loading a saved session is `ModelConfig::load()` — the same function that loads a fresh config. `#[serde(default)]` on `message`, `summary`, and `whisper` means fresh configs without these fields load cleanly.

Memory entries are not stored in sessions — they live in the model TOML and the retrieval index is rebuilt from config at startup.

## KV Cache Persistence

On a CPU-only 24B model, prompt prefill costs 10–15 seconds per session resume. 
The stable prompt prefix (wrappers + summary + history) grows across turns but is identical between consecutive turns.
llama.cpp's slot save/restore API can persist the KV cache to disk so this prefix doesn't need to be recomputed.

### How It Works

llama-server exposes a slot API when started with `--slot-save-path <dir>`. weftd's `KvCacheManager` (`kvcache.rs`) wraps two HTTP calls:

- `POST /slots/0?action=save` with `{"filename": "main.bin"}` — writes the KV cache
- `POST /slots/0?action=restore` with `{"filename": "main.bin"}` — loads it back

The cache is always stored in `kv_cache/main.bin` relative to the working directory. 
A single cache file is maintained, overwritten on each save.

### Fingerprint Validation

The KV cache blob has no compatibility header. If the model, context size, flash-attn setting, or KV quantization type changes between save and restore, llama-server silently corrupts state or crashes. weftd writes a `kv_cache/main.meta.toml` alongside the cache file containing these four fields. On restore, the meta is loaded and compared against the current config. Any mismatch skips the restore with a logged warning.

### KV Cache Quantization

When `kv_cache` is enabled, weftd passes `--cache-type-k q8_0 --cache-type-v q8_0` to llama-server. This halves the cache file size (~1.2 GB vs ~2.5 GB at 16K context for a 24B model) with negligible quality loss, and also reduces live RAM usage during generation. The quantization type is hardcoded, not configurable — q8_0 is the right default for this model class.

### Mid-Session Checkpointing

Summary autogeneration is the one mid-session operation that invalidates the KV cache. It sends a different prompt (the summary instructions) to the same llama-server, evicting the fiction conversation's KV state from slot 0 and replacing it with the summary's working state. Without intervention, the next fiction turn would start from a cold slot and pay full prompt-prefill cost — minutes on CPU for a 24B model at 16K context.

`run_summary()` wraps the summary call in a save/restore: save slot 0 to `main.bin` before `summary::run()`, restore it after. Both calls reuse the existing `KvCacheManager::save()` / `restore()` — no separate checkpoint file. The mid-session save overwrites `main.bin` with the current good state; the post-summary restore reads it back. This means `main.bin` is updated multiple times per session, not just on shutdown — but it always reflects the most recent good cache state.

**Save failure aborts the summary.** If the checkpoint can't be written, we don't run summary — there'd be no rollback point. The summary aborts with an error event, and the UI still receives `summarized` so it doesn't hang on the spinner.

**Restore failure logs only.** Matches the startup restore philosophy: stale prefill is acceptable, corrupting state isn't. The user notices a slow next turn rather than receiving a frontend error.

**A `summarizing` flag protects shutdown.** While the summary is in flight, slot 0 transitions through three states: pre-summary good (before save), summary working state (during summary call), and pre-summary good again (after restore). If the user Ctrl+Cs during the summary call, slot 0 holds garbage — but `main.bin` already contains the pre-summary good state from the mid-session save. A naive shutdown save would clobber that with the in-RAM garbage. An `AtomicBool` set for the duration of `run_summary` causes the shutdown handler to skip the save, preserving the disk state. Cost: one turn of cache progress lost on next launch. Acceptable.

### Shutdown Ordering

The KV save must complete while llama-server is still running. llama-server is spawned in its own process group (`process_group(0)`) so that Ctrl+C (SIGINT) only reaches the weftd process — without this, the terminal sends SIGINT to the entire process group and llama-server exits before the save runs. In `main()`, `ServeHandle` is declared early and dropped when `main()` returns (Rust drops locals in reverse declaration order). The save call is placed after the session TOML save and before `main()` returns — llama-server is still alive at that point. This ordering is load-bearing: moving the save after `ServeHandle::drop` would send HTTP requests to a dead process.

The shutdown save is also gated on the summarizing flag (see Mid-Session Checkpointing above): if a summary autogeneration was in flight when Ctrl+C arrived, the save is skipped to preserve the pre-summary checkpoint already on disk.

axum::serve(...).await  →  session TOML save  →  KV cache save  →  main() returns  →  serve drops  →  llama-server killed

### Error Philosophy

Every KV cache operation is best-effort. Restore failures (missing file, fingerprint mismatch, HTTP error) return `Ok(false)` and log a warning — they never prevent startup. Save failures log a warning — they never prevent shutdown. The system works identically whether or not `kv_cache` is enabled. Stale prefill is always better than a corrupted cache or a crashed process.


## Frontend

### Pure View Model

The frontend has no authority over state. It renders what `sync` tells it. It sends user input and edits to the backend. That's it.

Event handling:
- `"sync"` — replaces the entire message list and summary state, re-renders both
- `"thinking"` — shows a placeholder (UI-only, doesn't touch messages)
- `"token"` — progressively updates the thinking placeholder with accumulated text
- `"error"` — shows error banner (click to dismiss)
- `"whispers"` — replaces whisper list display
- `"cues"` — stores cue list, renders pill buttons with active-state tied to whisper list
- `"summary"` — summary text and up_to updated (after autogeneration); frontend updates the summary panel
- `"summary_needed"` — prompt exceeded configured context fraction; shows banner or autogenerates if Automatic is checked
- `"summarizing"` / `"summarized"` — autogeneration in progress; hint text reflects the running state

### Editing

All messages (user, assistant, system) are editable via `contentEditable` on a child div. On blur, if the content changed, the frontend sends an `Edit` action. The backend applies the edit and responds with `sync`.

### Delete Buttons

Each message has a × button (visible on hover, always slightly visible on mobile). It sends `/delete {index}` as a regular submit. The backend processes it as a command and responds with `sync`.

## Future Architecture Notes

### Entity/State Tracking

Plugs into prompt assembly as a new injection layer between summary and recent history. Runs async after each assistant response (non-blocking). See `weftd — Entity/State Tracking Design.md` for the full design.

### WebSocket Reconnection

Currently, a browser refresh loses the connection and the frontend state. Since the backend holds all state, reconnection just needs to re-send `sync` and `whispers` on the new connection — the same thing `run()` already does on initial connect.

### Submit-ID Round-Trip

Every `submit` action from the frontend carries a `submit_id` (UUID). The backend echoes this id in the matching `sync` or `ack` event. The frontend disables the Send button when a submit is in flight and re-enables it only when an event with the matching `submit_id` arrives. A 5-second timeout surfaces a clear error if the round-trip never completes.

This exists because earlier versions cleared the input optimistically on click, which masked failures: a tap on mobile that produced no backend round-trip would leave the input cleared but no message sent, with no visible error. The user had no signal that anything had gone wrong. The id-based confirmation makes "send succeeded" a backend-confirmed event, not a UI assumption.

Cue buttons, action buttons (Delete, Continue), and the per-message delete × bypass `submit_id` because they're idempotent state mutations that the backend confirms via `sync` regardless. Only the free-text Send path uses the round-trip — that's the path where silent failure was actually a problem.

### Dev Mode

The `--dev` CLI flag swaps the embedded static-file routes for a `tower_http::services::ServeDir` fallback. With it, edits to `static/app.js` or `static/style.css` are visible on the next browser refresh — no Rust recompile. Production builds remain a single binary with `include_str!`.

The dev fallback is wired via `Router::fallback_service`, so the explicit `/ws` route still wins over the file server. There's no auto-reload; the page must be refreshed manually.

### Debug Overlay

`?debug=1` in the URL activates two on-screen diagnostics in the frontend:

- A green monospace line above the input showing `ws=<readyState> pending=<id-prefix> outbox=<n>`. Updated at every state transition.
- An event log inside the editor showing the last 10 WebSocket frames in/out, plus instrumented internal events (button taps, blocked submits, input reads).

This exists because Android Firefox over Tailscale has no remote debugger path that's reliable to set up. The overlay surfaces enough state to diagnose send-path issues without USB debugging. Production users who don't append `?debug=1` see nothing.
