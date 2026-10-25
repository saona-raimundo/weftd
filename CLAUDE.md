# weftd

Local-first scaffolding runtime for local LLMs: a Rust/axum daemon that drives
`llama-server` (llama.cpp) as a child process, plus a vanilla-JS UI over a single
WebSocket. Current application: collaborative fiction. Direction: a task-DAG
runtime whose tasks feed one generation sink.
Formerly named `personai`; older docs/identifiers may still use it. Same project —
don't rename in passing; flag leftovers instead.

<!-- Maintainer note (stripped from Claude's context): module and symbol names below
come from design docs. If any drift from src/, fix this file, don't work around it. -->

## Commands
- Build: `cargo build` · release: `cargo build --release`
- Test: `cargo test` · single: `cargo test <name>`
- Lint: `cargo clippy --all-targets -- -D warnings`
- Format: `cargo fmt` · check only: `cargo fmt --check`
- Run: `cargo run -- <config.toml>`; add `--dev` to serve `static/` from disk
  (edit HTML/JS/CSS without rebuilding). Confirm args with `cargo run -- --help`.
- Runtime needs `llama-server` on PATH and a GGUF model named in the config.
  Tests must never require either.
- Done means: `cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test` pass.
- First session in a fresh checkout: verify the Layout section against `src/` and report mismatches.

## Layout
- `src/main.rs` — startup (config, llama-server, KV restore, retrieval pools), axum routes, shutdown save.
- `src/cli.rs` — clap args: `[PATH]`, `--timeout`, `--serve-log`, `--no-open`, `--dev`.
- `src/connection.rs` — async WebSocket handler: parse client action → call execute → apply Effect.
  Also owns all async IO: `generate_turn()`, streaming, summary runs, KV save/restore around them.
- `src/command.rs` — parses `/`-prefixed text into `Command` (`ParseResult::Fiction | Command | Unknown`).
- `src/execute.rs` — pure, synchronous command logic (`dispatch`) returning `Effect`.
- `src/chat.rs` — `Message`, `Role`, `Conversation`, `SummaryState`, client `Action` enum (the WS protocol).
- `src/whisper.rs` — `WhisperStack`: turn-limited system injections.
- `src/summary.rs` — context-ratio threshold check and summary generation.
- `src/prompt.rs` — `prompt::build()` fills named sections; order comes from `prompt_order` in the TOML.
- `src/render.rs` — renders sections per `PromptStructure` (`chatml`, `mistral_v7_tekken`).
- `src/retrieval.rs` — named pools; two BM25 indices per pool (content + keywords); YAKE extracts queries.
- `src/client.rs` — llama-server HTTP client: native `/completions` (alias of `/completion`), SSE via `reqwest-eventsource`.
- `src/serve.rs` — spawns `llama-server` as a child in its own process group (`ServeHandle`).
- `src/kvcache.rs` — slot 0 save/restore via the slot API + `kv_cache/main.meta.toml` fingerprint.
- `src/config.rs` — `ModelConfig` (config/session TOML schema). `src/session.rs` — snapshot + save.
- `src/tailscale.rs` — prints the Tailscale URL if available. `src/utils.rs` — timestamps.
- Unit tests live in `#[cfg(test)]` modules next to the code in most files.
- `static/` — vanilla JS frontend. No framework, no bundler, no build step.
- `models/` — example TOML configs. A config file IS a session file.

## Architecture invariants (do not break)
1. Backend is the single source of truth. Frontend is a pure view: on every full
   `sync` event it replaces its state wholesale. No client-owned state.
2. Every backend mutation ends with `send_sync()`. A mutation path without it is a bug.
3. Commands are plain text with a `/` prefix inside the existing submit action.
   Adding a command must NOT change the WebSocket protocol.
4. Command logic goes in `execute.rs` as a pure fn returning `Effect`
   (`Sync`, `Generate`, `WhispersChanged`, `Error`, `Multi`). `Multi` is flat:
   never nest `Multi` in `Multi`. IO/async only in `connection.rs`.
5. Turn pipeline in `generate_turn()`: snapshot history → pre-generation tasks
   (retrieval) → `generate()` → sync → post-generation tasks (summary threshold).
   New per-turn work goes into the pre- or post-generation slot; don't reorder.
6. Config file == session file (`ModelConfig`). New fields get `#[serde(default)]`
   so existing files keep loading. Sessions are saved on shutdown to a new
   `sessions/<timestamp>.toml`; the loaded file is never overwritten.
7. Errors: best-effort; stale is better than wrong; never corrupt state.
   On failure keep the last good state and return `Effect::Error`; never half-apply.

## Gotchas
- Shutdown ordering is load-bearing: save session + KV cache BEFORE `ServeHandle`
  drops (the save needs llama-server alive). llama-server runs in its own process
  group so the terminal's Ctrl-C doesn't reach it before the save. Don't reorder.
- The `summarizing` AtomicBool guards the shutdown save; an in-flight summary must
  not race it. Any new background writer needs the same treatment.
- KV cache persists via the llama-server slot API plus a fingerprint file
  (`kvcache.rs` `KvMeta`). The fingerprint guards cache *compatibility* (model,
  ctx_size, flash_attn, cache types): add any new server param that changes the
  KV layout. Prompt-prefix changes don't need it — the saved slot carries its
  tokens, so llama-server reuses only the common prefix; the cost is re-prefill time.
- KV prefix cost: changes early in the rendered prompt (start wrappers, system,
  early history) invalidate the cached prefix → full re-prefill (slow on CPU).
  The prompt tail (end wrappers, trailing system content) is uncached and cheap.
- `mistral_v7_tekken`: zero whitespace around control tokens. Trailing system
  content renders as `[OOC: ...]` inside the final `[INST]`. Every template change
  needs a render test asserting exact output.
- We use llama-server's native `/completion` with our own templating — not the
  OpenAI-compatible chat endpoint.
- Retrieval indices are built once at startup. Mutating a pool at runtime means
  rebuilding BOTH of that pool's indices.
- "weftd" is also the name of unrelated projects; don't trust web results for it.

## Working conventions
- Design before implementing. When asked for a design, deliver design only — no edits.
- Targeted diffs. Don't rewrite whole files or reformat code you didn't change.
- One incremental change at a time; build + tests green between steps.
- Verify claims against source (cite `file:line`) or upstream docs before asserting.
- Follow the existing architecture. Generalize only after two concrete cases.
- No new dependencies without asking.
- Terse responses: what changed, how it was verified, open questions.

## Direction: task DAG
Tasks (algorithmic or generative) become DAG nodes feeding a single generation
sink. Adopt incrementally: hardcode concrete tasks in the pre/post-generation
slots first; extract the abstraction once the shape is proven.

## Planned work (context, not instructions)
Moving offline-only features into the UI, roughly cheapest first:
- Live end-wrapper editing (cheap: uncached tail).
- Live sampler editing; live cue editing.
- `/remember`: add an entry to a retrieval pool and rebuild that pool's BM25 indices.
- `/save`: on-demand checkpoint of session (and KV slot).
- Retrieval pool config; per-turn retrieval visibility in the UI.
- Bulk history replacement.
- Start-wrapper editing invalidates the whole KV prefix → keep offline, or warn
  and confirm before applying.
Also planned: entity/state-tracking task, stop button (cancel in-flight
generation), WebSocket reconnection (client re-requests a full sync),
session auto-save (must respect the `summarizing` guard).
