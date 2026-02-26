# CLAUDE.md

## What This Is

workq is a standalone work-tracking engine in Rust. It ensures work gets done exactly once via structural deduplication, priority scheduling, and lifecycle management.

This is a library, not an application. The host (first consumer: [animus](~/src/witt3rd/animus)) provides worker implementations and policies.

## Commands

```bash
cargo test              # Run all tests (12 integration tests)
cargo clippy            # Lint
cargo build             # Build library + CLI binary
```

Pre-commit hook (`.githooks/pre-commit`) runs `cargo fmt --check`, `cargo test`, and `cargo clippy -D warnings`.
CI (GitHub Actions) runs the same checks on push/PR to master with `RUSTFLAGS="-D warnings"`.

## Architecture

| File | Purpose |
|------|---------|
| `src/model.rs` | Core types: WorkItem, State, Provenance, Outcome, LogEntry, NewWorkItem builder |
| `src/engine.rs` | Public API: submit, claim, start, complete, fail, log, events |
| `src/storage.rs` | SQLite storage layer: schema, queries, state transitions |
| `src/event.rs` | Structured event types emitted on every state transition |
| `src/error.rs` | Error types |
| `src/bin/workq.rs` | CLI binary (placeholder) |
| `tests/engine_test.rs` | Integration tests covering lifecycle, dedup, retry, logs, events |

## Dependencies

chrono 0.4, rusqlite 0.35 (bundled), serde 1, serde_json 1, thiserror 2, uuid 1
Edition 2024 — requires Rust 1.85+

## Design Spec

- Original spec: `~/src/witt3rd/animus/spec/bus_new.md`
- `DESIGN.md` — adapted design doc (states, dedup, worker interface, storage)
- `ISSUES.md` — code review findings: 3 critical, 7 important, 4 minor (transactions, encapsulation, fragile SQL)

## State Machine

```
Created → Queued | Merged
Queued  → Claimed | Dead
Claimed → Running | Queued
Running → Completed | Failed
Failed  → Queued | Dead
Terminal: Completed, Dead, Merged
```

## Conventions

- All state transitions go through `Engine` — never mutate storage directly from outside
- `Storage` enforces valid state transitions via `State::can_transition_to()`
- Events are recorded for every state change — dashboards subscribe to these
- Logs are scoped to work items — no global log stream, no console streaming
- Tests use `Engine::in_memory()` — no temp files, no cleanup
- Pre-commit hook runs fmt + clippy + tests; don't bypass it
- `Storage` and its methods are `pub` but should be `pub(crate)` per conventions — see `ISSUES.md`
