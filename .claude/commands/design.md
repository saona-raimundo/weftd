---
description: Produce a design (no code edits) for a weftd change
argument-hint: <feature or change>
---
Design, do not implement: $ARGUMENTS

1. Read the relevant code first; cite `file:line` for every claim about current behavior.
2. State which architecture invariants in CLAUDE.md the change touches.
3. Proposal: where logic lives (pure `execute.rs` vs `connection.rs`), new `Effect`
   usage (flat `Multi` only), turn-pipeline slot if any, config fields
   (`#[serde(default)]`), and confirmation that the WebSocket protocol is unchanged.
4. KV-cache impact: does it alter the cached prefix (expensive) or only the tail?
5. Failure behavior: what stays as last-good state; what `Effect::Error` reports.
6. Break into ordered, independently testable steps, each a small diff.
7. Tests to add. Open questions last.
Do not edit any files.
