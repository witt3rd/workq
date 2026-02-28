# CLAUDE.md

## What This Is

animus-rs is the substrate for relational beings — the machinery that lets an animus exist, persist, and become. Each animus is a self-contained appliance: data plane (Postgres-backed work queues and semantic memory), control plane (queue watching, resource gating, focus spawning), faculties (pluggable cognitive specializations), LLM abstraction, and observability.

Each animus is a self-contained appliance — one `docker compose up` starts a complete agent with integrated observability. Milestone 2 (current) adds the control plane and faculty system on top of the data plane:
- **Work queues** via pgmq (Postgres extension)
- **Semantic memory** via pgvector (embedding search + hybrid BM25+vector)
- **LLM abstraction** via rig-core (Anthropic provider)
- **Observability** via OpenTelemetry (traces, metrics, logs) through OTel Collector to Tempo/Prometheus/Loki/Grafana

Postgres-only. Fully async on tokio. SQLx for database access.

## Commands

```bash
cargo test                        # Run unit tests (requires no Postgres)
cargo test -- --ignored           # Run integration tests (requires Postgres)
cargo clippy                      # Lint
cargo build                       # Build library + CLI binary
docker compose up -d              # Start full appliance (animus + Postgres + observability)
docker compose up animus postgres -d  # Core services only (no observability)
docker compose build animus       # Rebuild animus image
docker compose -f docker-compose.observer.yml up -d  # Standalone observer stack (fleet)
cargo test --test telemetry_smoke_test -- --ignored   # Smoke tests (requires docker stack)
cargo test --test faculty_test -- --ignored --nocapture  # Faculty end-to-end test
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
| `src/telemetry/mod.rs` | Three-signal OTel init (traces, metrics, logs), TelemetryGuard |
| `src/telemetry/metrics.rs` | Metric instrument factories (counters, histograms) |
| `src/telemetry/genai.rs` | GenAI semantic convention span helpers |
| `src/telemetry/work.rs` | Work execution span helpers |
| `src/faculty/mod.rs` | Faculty config (TOML), hook definitions, registry by work type |
| `src/engine/mod.rs` | Control plane re-exports |
| `src/engine/focus.rs` | Focus lifecycle: dir creation, hook pipeline, outcome reading |
| `src/engine/control.rs` | ControlPlane loop: PgListener, route to faculty, spawn focus, retire work |
| `src/error.rs` | Error types |
| `src/bin/animus.rs` | Control plane daemon (connects DB, loads faculties, runs engine) |
| `Dockerfile` | Multi-stage Rust build (builder + slim runtime) |
| `docker-compose.yml` | Full appliance: animus + Postgres + observability |
| `docker-compose.observer.yml` | Standalone observer stack for fleet use |

## Dependencies

See `Cargo.toml` for versions. Key crates: sqlx, tokio, rig-core, opentelemetry (+ otlp, sdk, appender-tracing), tracing, secrecy, chrono, serde, uuid.

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
