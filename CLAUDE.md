# CLAUDE.md

## What This Is

animus-rs is an AI persistence engine covering the full stack: data plane (Postgres-backed work queues and semantic memory), control plane (work scheduling, domain center orchestration), LLM abstraction, and observability.

Milestone 1 (current) implements the data plane foundation:
- **Work queues** via pgmq (Postgres extension)
- **Semantic memory** via pgvector (embedding search + hybrid BM25+vector)
- **LLM abstraction** via rig-core (Anthropic provider)
- **Observability** via OpenTelemetry with GenAI semantic conventions

Postgres-only. Fully async on tokio. SQLx for database access.

## Commands

```bash
cargo test                        # Run unit tests (requires no Postgres)
cargo test -- --ignored           # Run integration tests (requires Postgres)
cargo clippy                      # Lint
cargo build                       # Build library + CLI binary
docker compose up -d              # Start dev Postgres (pgmq + pgvector)
```

Pre-commit hook (`.githooks/pre-commit`) runs `cargo fmt --check`, `cargo test`, and `cargo clippy -D warnings`.

## Architecture

| Module | Purpose |
|--------|---------|
| `src/config/` | Typed env var loading, secrecy-wrapped secrets |
| `src/db/mod.rs` | Postgres connection pool (PgPool), SQLx migrations |
| `src/db/pgmq.rs` | pgmq queue operations (create, send, read, archive, delete) |
| `src/db/work.rs` | Work item submit with structural dedup, pgmq integration |
| `src/memory/store.rs` | pgvector storage, vector search, hybrid BM25+vector search |
| `src/llm/mod.rs` | rig-core Anthropic provider factory |
| `src/model/work.rs` | WorkItem, State, Provenance, Outcome, NewWorkItem |
| `src/model/memory.rs` | MemoryEntry, NewMemory, MemoryFilters |
| `src/telemetry/` | OTel init, GenAI span helpers, work span helpers |
| `src/error.rs` | Error types |
| `src/bin/animus.rs` | CLI binary (placeholder) |

## Dependencies

sqlx 0.8, tokio 1, rig-core 0.31, opentelemetry 0.31, tracing 0.1, secrecy 0.10, chrono 0.4, serde 1, thiserror 2, uuid 1
Edition 2024 — requires Rust 1.85+

## Design Docs

- `docs/db.md` — Database layer design (schema, API, deployment)
- `DESIGN.md` — System design (principles, state machine, dedup, observability)
- `docs/plans/` — Implementation plans

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

- `Db` is the primary public API — all database operations go through it
- State transitions enforced by `State::can_transition_to()`
- Structural dedup on `(work_type, dedup_key)` — transactional
- Secrets wrapped in `secrecy::SecretString`, never logged
- OTel spans for LLM calls use GenAI semantic conventions
- Pre-commit hook runs fmt + clippy + tests; don't bypass it
- Integration tests require Docker Postgres (`docker compose up -d`)
