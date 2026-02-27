# animus-rs Design

*AI persistence engine — data plane, control plane, LLM abstraction, and observability, built on Postgres.*

## Origin

animus-rs started as `workq`, a standalone work-tracking engine. When we discovered pgmq (Postgres queue extension), it became clear that pgmq already provides the queue primitives workq was hand-rolling. The project pivoted: build the full Animus system as one well-structured Rust crate — data plane (work queues, semantic memory), control plane (scheduling, domain center orchestration), workers, and observability.

The predecessor system used filesystem-based storage (YAML task queues, markdown substrate, ChromaDB for vectors, JSONL logs). It worked but had real limitations: no structural dedup, no transactional guarantees, fragile file-based queues, a separate ChromaDB process. animus-rs replaces all of this with Postgres + extensions.

## The Core Reframe

animus-rs is a **work engine**, not a message bus. Work items exist, have identity, go through a lifecycle, and the system ensures they get done exactly once.

This is not request/reply. This is: **something needs doing**.

| Message bus | Work engine |
|-------------|-------------|
| Task is a message between processes | Work item is a thing that needs doing |
| Fixed processes poll their queues | Workers spin up on demand |
| Routing by queue name | Scheduling by work type and capacity |
| No dedup — same work dispatched N ways | Work identity — same intent recognized and merged |

## Design Principles

### 1. Work Has Identity

A work item has semantic identity. "Check in with Kelly" is the same work whether it came from a user request, an extracted initiative, or a heartbeat skill.

**Structural dedup** (built-in): exact match on `(work_type, dedup_key)` — e.g., `("engage", "person=kelly")` collapses duplicate requests targeting the same person.

**Semantic dedup** (future): LLM or embedding-based similarity for "is this initiative basically the same as that heartbeat task?" — more expensive, used selectively.

### 2. Work, Not Messages

Work items don't have `from` or `to` fields. They have:
- **What** needs doing — `work_type` + `params`
- **Why** it exists — `provenance` (source + trigger)
- **Priority** — how urgent
- **State** — lifecycle position

The caller says "this work needs doing" and the system figures out how.

### 3. Dynamic Workers, Not Fixed Processes

Workers are ephemeral — they exist to do one unit of work, then they're done. The system manages:
- **Global capacity** — total concurrent workers
- **Per-type capacity** — e.g., max 3 LLM workers
- **Backpressure** — queuing with priority ordering
- **Circuit breaking** — exponential backoff on failing work types
- **Poison pill detection** — dead-lettering items that fail repeatedly

### 4. Work-Once Guarantee

- A work item is **claimed** by exactly one worker (pgmq visibility timeout)
- **Duplicate** work is detected and merged (structural dedup, transactional)
- Every work item either **completes**, **fails** (with retry), or goes **dead** — nothing disappears silently

### 5. Minimal Public Surface

`Db` is the primary public API. All database operations — work queues, memory, pgmq — go through it. Internal types (`WorkItemRow`, `MemoryEntryRow`, `parse_state`) are private. New modules default to `pub(crate)` unless explicitly needed by consumers.

### 6. Postgres Is the Platform

animus-rs does not abstract away the database. Postgres with pgmq + pgvector is a deliberate choice, not a swappable backend. This means:
- Queue semantics via pgmq SQL functions — not a hand-rolled state machine
- Vector search via pgvector operators — not a separate vector DB process
- Transactional guarantees across work items and queue messages
- One operational dependency, not three

## Work Item Lifecycle

```
Created → Dedup Check → Queued → Claimed → Running → Completed
               ↓                              ↓
            Merged                     Failed / Abandoned
            (into existing)                   ↓
                                         Retry? → Queued
                                            ↓
                                          Dead
```

### States

| State | Meaning |
|-------|---------|
| **Created** | Submitted, pending dedup check |
| **Queued** | In pgmq, waiting for a worker |
| **Claimed** | Worker assigned via pgmq read (visibility timeout) |
| **Running** | Worker actively processing |
| **Completed** | Done successfully |
| **Failed** | Execution error, may be retried |
| **Dead** | Exhausted retries or poisoned — terminal |
| **Merged** | Structural dedup hit — linked to canonical item, terminal |

### Valid State Transitions

```
Created  → Queued       (passed dedup, sent to pgmq)
Created  → Merged       (structural dedup hit)
Queued   → Claimed      (worker assigned via pgmq.read)
Queued   → Dead         (cancelled or circuit-broken)
Claimed  → Running      (worker started)
Claimed  → Queued       (worker failed to start, re-queue)
Running  → Completed    (success)
Running  → Failed       (execution error)
Failed   → Queued       (retry)
Failed   → Dead         (exhausted retries)
```

Transitions are enforced by `State::can_transition_to()` in `model/work.rs`.

### Provenance

Every work item tracks where it came from:

```rust
Provenance {
    source: "heartbeat",        // What created it
    trigger: "skill/check-in",  // More specific origin
}
```

When work is merged, the canonical item retains its provenance and the merged item's origin is preserved via the `merged_into` relationship.

## Structural Dedup

When submitting work with a `dedup_key`, the system checks: is there already a work item with the same `(work_type, dedup_key)` in a non-terminal state? If so, **merge** — the new item is marked as merged and linked to the canonical item.

The entire submit flow runs within a single Postgres transaction:
1. Insert work_items row (state = `created`)
2. Check dedup against the partial index on `(work_type, dedup_key)`
3. If match: mark as `merged`, set `merged_into`, commit
4. If no match: `pgmq.send()` to queue, update state to `queued`, commit

This guarantees crash safety and correctness under concurrent access.

### Dedup Verdicts (Future)

| Verdict | Meaning |
|---------|---------|
| **Distinct** | Not the same work — both execute |
| **Merge** | Same work — merge into existing, one execution |
| **Supersede** | New work replaces old (e.g., updated instructions) |
| **Defer** | Same work but hold off — context may have changed |

Currently implements Distinct and Merge. Supersede and Defer are future extensions.

## Observability

Observability uses OpenTelemetry with the GenAI semantic conventions for LLM operations, backed by the Grafana stack for storage and visualization.

### Telemetry Pipeline

animus-rs emits traces via OTLP gRPC to the observability stack:

```
animus-rs (tracing + tracing-opentelemetry)
    → OTLP gRPC (:4317)
        → Grafana Tempo (traces)
        → Grafana Loki (logs, future)
        → Prometheus (metrics, future)
    → Grafana UI (:3000) — pre-configured datasources
```

**Traces:** Every work item execution gets a span (`work.execute`) with work type, ID, and state transitions as span events. LLM calls get GenAI spans (`gen_ai.chat`, `gen_ai.embeddings`) with model, provider, and token usage attributes.

**No custom event tables.** Observability flows to the Grafana stack, not into the application database. Postgres stores domain state; OTel handles observability. Cleaner separation.

The `tracing` + `tracing-opentelemetry` bridge means Rust code uses idiomatic `tracing::instrument` / `tracing::info!()` macros, and the OTel layer exports them as spans and logs.

### Observability Stack

The full Grafana stack runs alongside Postgres in Docker Compose:

| Service | Port | Purpose |
|---------|------|---------|
| **Tempo** | 4317/4318 | Trace storage, receives OTLP gRPC/HTTP |
| **Loki** | 3100 | Log aggregation |
| **Prometheus** | 9090 | Metrics storage |
| **Grafana** | 3000 | Unified UI with pre-provisioned datasources |

All observability data is persisted in Docker volumes (`tempo-data`, `loki-data`, `prometheus-data`, `grafana-data`). Same volumes across container restarts — no data loss.

Grafana auto-provisions Tempo, Loki, and Prometheus as datasources on startup (`docker/grafana/datasources.yml`). No manual configuration needed — `docker compose up -d` gives you a fully wired observability stack.

## Storage

Postgres with pgmq (queue extension) and pgvector (embedding search). The database is the single source of truth — work items, queue messages, and memories all live in Postgres.

Two-layer data access:
- **Direct SQLx** (`db/` module): pgmq operations, work_items dedup/provenance, custom queries
- **rig-postgres** (`memory/` module): pgvector VectorStoreIndex for embedding storage and search

Both share the same `sqlx::PgPool`. Migrations managed by SQLx (`./migrations/`).

See [docs/db.md](docs/db.md) for the full schema, API surface, and deployment details.

## Worker Interface (Future)

Workers are provided by the host. The system defines the contract:

```rust
trait Worker {
    async fn execute(&self, item: &WorkItem) -> WorkOutcome;
    fn accepts(&self, work_type: &str) -> bool;
    fn resources(&self, work_type: &str) -> ResourceClaim;
}

enum WorkOutcome {
    Success { result: Value },
    Failure { error: String, retryable: bool },
    Spawn { children: Vec<NewWorkItem>, continuation: Option<Continuation> },
}
```

The system doesn't care what the worker does. It cares about:
- Did it succeed or fail?
- How long did it take?
- Did it spawn child work?

## Deployment

### Infrastructure Stack

One `docker compose up -d` starts everything animus-rs needs:

| Container | Image | Purpose |
|-----------|-------|---------|
| **postgres** | Custom (`docker/Dockerfile`) | Postgres 17 + pgmq + pgvector |
| **tempo** | `grafana/tempo` | Trace storage (OTLP receiver) |
| **loki** | `grafana/loki` | Log aggregation |
| **prometheus** | `prom/prometheus` | Metrics storage |
| **grafana** | `grafana/grafana` | Unified observability UI |

The Postgres image is built from `ghcr.io/pgmq/pg17-pgmq` (pgmq pre-installed) with `postgresql-17-pgvector` added via apt. Extensions are enabled at init time via `docker/init-extensions.sql`. One image, one set of extension versions, no environment drift.

The same Docker Compose stack is used across all environments:

- **Local dev:** `docker compose up -d` — full stack with Grafana UI at `:3000`
- **CI:** GitHub Actions runs the same containers as service containers
- **Production:** Same images with production config (volumes, networking, secrets)

### animus-rs Service

animus-rs connects to the stack via two env vars:

- `DATABASE_URL` — Postgres connection string
- `OTEL_ENDPOINT` — Tempo OTLP endpoint (`:4317`)

Deployment per environment:

- **Local dev:** `cargo run` or tests against Docker Compose
- **CI:** `cargo test -- --include-ignored` against the service containers
- **Production (Arch Linux):** systemd user service (`~/.config/systemd/user/animus.service`), secrets via `EnvironmentFile` (chmod 600)

Migrations run at startup (`Db::migrate()`) or manually via `cargo sqlx migrate run`.

See [docs/db.md](docs/db.md) for the full schema and API surface.

## Implementation Status

### Implemented (Milestone 1: Data Plane)
- **Config**: Typed env var loading, `secrecy::SecretString`, `dotenvy` for `.env`
- **DB pool + migrations**: SQLx `PgPool`, three migrations (extensions, work_items, memories)
- **pgmq operations**: create, send, read, archive, delete via SQL functions
- **Work items**: submit with structural dedup, transactional insert + dedup check + pgmq send
- **Semantic memory**: pgvector storage, vector similarity search (cosine), hybrid BM25+vector
- **LLM module**: rig-core Anthropic provider factory
- **OpenTelemetry**: OTel init, GenAI semantic convention spans, work span helpers
- **Observability stack**: Grafana + Tempo + Loki + Prometheus, pre-wired datasources
- **Infrastructure**: Docker Compose with Postgres (pgmq + pgvector) + full Grafana stack
- **Core types**: WorkItem, State, Provenance, Outcome, NewWorkItem builder
- **State machine**: enforced valid transitions via `State::can_transition_to()`
- **Test suite**: 14 tests (config, telemetry, DB, pgmq, work, memory, full lifecycle)

### Not Yet Implemented (Milestones 2+: Control Plane, Workers, CLI)
- Worker trait and worker pool
- Control plane scheduling and domain center orchestration
- Capacity management (global + per-type limits)
- Circuit breaking and backpressure
- Semantic dedup
- Dedup time window (recently-completed)
- Child work spawning and continuations
- IPC layer (Unix domain socket)
- CLI commands (status, list, show, logs)
- `animus serve` daemon mode
- SQLx offline metadata for CI

## Open Design Questions

- **Worker interface**: trait-based or message-passing? In-process only or support child processes?
- **IPC protocol**: JSON lines vs protobuf over Unix domain socket
- **Semantic dedup**: embedding similarity threshold, when to invoke, cost control
- **Priority formula**: system-provided age boost, or fully host-controlled?
- **Configuration**: TOML for work type definitions (capacity, retry policy, priority)
- **Child work**: how to express continuations that run when all children complete
- **Embedding provider**: Anthropic doesn't support embeddings via rig-core; need a separate provider
