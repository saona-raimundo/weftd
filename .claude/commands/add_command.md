---
description: Add a new "/" chat command following the Effect pattern
argument-hint: </name and behavior>
---
Add the chat command: $ARGUMENTS

- Parse and implement it in `execute.rs` as pure logic returning an `Effect`
  (`Multi` must stay flat). No IO or async there.
- Wire it only through the existing dispatch in `connection.rs`; no protocol changes.
- Any state mutation must end in `send_sync()`.
- If it touches retrieval pools, rebuild that pool's content + keyword indices.
- If it changes the rendered prompt prefix, call out the KV-cache invalidation.
- Add unit tests in `execute.rs` (happy path + error path returning `Effect::Error`).
- Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test`; report results.
Keep the diff minimal; one command per change.
