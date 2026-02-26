# CLAUDE.md

## What This Is

workq is a standalone work-tracking engine in Rust. It ensures work gets done exactly once via structural deduplication, priority scheduling, and lifecycle management.

This is a library, not an application. The host (first consumer: [animus](~/src/witt3rd/animus)) provides worker implementations and policies.

## Commands

```bash
cargo test              # Run all tests (12 integration tests)
cargo clippy            # Lint (enforced by pre-commit hook)
cargo build             # Build library + CLI binary
```

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

## Design Spec

Full design lives in the animus repo: `~/src/witt3rd/animus/spec/bus_new.md`

## Conventions

- All state transitions go through `Engine` — never mutate storage directly from outside
- `Storage` enforces valid state transitions via `State::can_transition_to()`
- Events are recorded for every state change — dashboards subscribe to these
- Logs are scoped to work items — no global log stream, no console streaming
- Tests use `Engine::in_memory()` — no temp files, no cleanup
- Pre-commit hook runs tests + clippy; don't bypass it
