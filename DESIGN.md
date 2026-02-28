# animus-rs Design

*Substrate for relational beings — data plane, control plane, LLM abstraction, and observability, built on Postgres.*

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

Observability is part of the product, not a dev convenience. Every animus ships with integrated observability — you can see what your agent is doing out of the box.

The system emits all three OTel signal types (traces, metrics, logs) through a unified pipeline, using GenAI semantic conventions for LLM operations, backed by the Grafana stack for storage and visualization.

### Telemetry Pipeline

animus-rs emits all three signal types via OTLP to the OTel Collector, which routes them to the appropriate backends:

```
animus-rs (tracing + tracing-opentelemetry + opentelemetry-appender-tracing)
    → OTLP gRPC (:4317)
        → OTel Collector
            → Tempo (traces)
            → Prometheus (metrics, via remote write)
            → Loki (logs, via OTLP HTTP)
    → Grafana UI (:3000) — pre-configured datasources with cross-linking
```

**Traces:** Every work item execution gets a span (`work.execute`) with work type, ID, and state transitions as span events. LLM calls get GenAI spans (`gen_ai.chat`, `gen_ai.embeddings`) with model, provider, and token usage attributes.

**Metrics:** Counter and histogram instruments for work submission, state transitions, queue operations, memory operations, operation duration, and LLM token usage (`src/telemetry/metrics.rs`). All emitted via the OTel Meter API.

**Logs:** `tracing::info!` / `tracing::warn!` / etc. are bridged to OTel logs via `opentelemetry-appender-tracing` and exported to Loki. No separate logging system — `tracing` macros produce both OTel spans and OTel logs.

**No custom event tables.** Observability flows to the Grafana stack, not into the application database. Postgres stores domain state; OTel handles observability. Cleaner separation.

### Observability Stack

The full observability stack is part of the animus appliance:

| Service | Port | Purpose |
|---------|------|---------|
| **OTel Collector** | 4317/4318 | Unified OTLP ingestion, routes to backends |
| **Tempo** | 3200 (API) | Trace storage, receives from Collector via OTLP gRPC |
| **Loki** | 3100 | Log aggregation, receives from Collector via OTLP HTTP |
| **Prometheus** | 9090 | Metrics storage, receives from Collector via remote write |
| **Grafana** | 3000 | Unified UI with pre-provisioned, cross-linked datasources |

Grafana datasources are pre-wired at startup with full cross-linking: Tempo→Loki (traces to logs), Tempo→Prometheus (traces to metrics), Loki→Tempo (logs to traces via derived fields), plus node graph and service map support.

All observability data is persisted in Docker volumes. Same volumes across container restarts — no data loss.

Tempo runs with `metrics_generator` enabled (service-graphs, span-metrics, local-blocks processors), which powers Grafana's Traces Drilldown and service map views and writes derived metrics to Prometheus.

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

### The Animus Appliance

An animus is a self-contained, vertically-integrated appliance. One `docker compose up` starts a complete, working agent with integrated observability:

```
┌──────────────────────────────────────────────────┐
│                    animus                         │
│                                                  │
│  animus-rs  ←→  Postgres (pgmq + pgvector)       │
│      │                                           │
│      ▼ OTLP                                      │
│  OTel Collector → Tempo (traces)                 │
│                 → Prometheus (metrics)            │
│                 → Loki (logs)                     │
│                                                  │
│  Grafana (:3000) — your window into the agent    │
└──────────────────────────────────────────────────┘
```

| Container | Purpose |
|-----------|---------|
| **animus-rs** | The agent service |
| **postgres** | Postgres + pgmq + pgvector |
| **otel-collector** | OTLP ingestion, routes to backends |
| **tempo** | Trace storage |
| **loki** | Log aggregation |
| **prometheus** | Metrics storage |
| **grafana** | Unified observability UI |

This is the default. No external services required. No accounts to create. No API keys for observability. Clone the repo, `docker compose up`, open `localhost:3000`.

### Fleet Deployment (Shared Observer)

When running multiple animi (Latin plural, second declension neuter), you can amortize the observability stack across instances. A separate `docker-compose.observer.yml` provides a standalone observer:

```
┌──────────┐ ┌──────────┐ ┌──────────┐
│ animus-1 │ │ animus-2 │ │ animus-N │   each: animus-rs + Postgres
└────┬─────┘ └────┬─────┘ └────┬─────┘
     │            │            │
     └────────────┼────────────┘
                  │ OTLP (:4317)
         ┌────────▼────────┐
         │  shared observer │   OTel Collector + Tempo + Prometheus
         │  (separate host) │   + Loki + Grafana
         └─────────────────┘
```

Two compose files in the repo:

| File | Purpose | Command |
|------|---------|---------|
| `docker-compose.yml` | Full appliance (default) | `docker compose up` |
| `docker-compose.observer.yml` | Shared observer for fleet use | `docker compose -f docker-compose.observer.yml up` |

Each animus in the fleet runs with `--profile core` to skip its local observability stack, pointing OTLP at the shared observer:

```sh
OTEL_ENDPOINT=http://observer-host:4317 docker compose --profile core up
```

Most users will run 1:1 (one animus, one observer built in). The shared pattern is for operators running 15+ animi on one or more hosts.

### Configuration

animus-rs connects to the stack via environment variables:

- `DATABASE_URL` — Postgres connection string (default: `postgres://animus:animus_dev@postgres:5432/animus_dev`)
- `OTEL_ENDPOINT` — OTLP gRPC endpoint (default: `http://otel-collector:4317`, overridable for fleet use)

The Postgres image is built from `ghcr.io/pgmq/pg17-pgmq` (pgmq pre-installed) with `postgresql-17-pgvector` added via apt. Extensions are enabled at init time via `docker/init-extensions.sql`. One image, one set of extension versions, no environment drift.

Migrations run at startup (`Db::migrate()`) or manually via `cargo sqlx migrate run`.

See [docs/db.md](docs/db.md) for the full schema and API surface.

## Implementation Status

### Implemented (Milestone 1: Data Plane + Observability)
- **Config**: Typed env var loading, `secrecy::SecretString`, `dotenvy` for `.env`
- **DB pool + migrations**: SQLx `PgPool`, three migrations (extensions, work_items, memories)
- **pgmq operations**: create, send, read, archive, delete via SQL functions
- **Work items**: submit with structural dedup, transactional insert + dedup check + pgmq send
- **Semantic memory**: pgvector storage, vector similarity search (cosine), hybrid BM25+vector
- **LLM module**: rig-core Anthropic provider factory
- **Telemetry**: Three-signal OTel pipeline (traces, metrics, logs), GenAI semantic convention spans, work span helpers, metric instrument factories, `TelemetryGuard` with force-flush
- **Observability stack**: OTel Collector + Tempo + Loki + Prometheus + Grafana, cross-linked datasources, metrics generator with service graphs
- **Infrastructure**: Docker Compose appliance with Postgres (pgmq + pgvector) + full observability stack
- **Core types**: WorkItem, State, Provenance, Outcome, NewWorkItem builder
- **State machine**: enforced valid transitions via `State::can_transition_to()`
- **Test suite**: 18 tests (config, telemetry, DB, pgmq, work, memory, full lifecycle, observability smoke tests)

### Not Yet Implemented (Milestone 2: Appliance Packaging)
- Dockerfile for animus-rs (multi-stage build)
- animus-rs as a service in docker-compose
- Compose profiles (`core` for fleet use without local observability)
- `docker-compose.observer.yml` for shared fleet observer
- `animus serve` daemon mode

### Not Yet Implemented (Milestones 3+: Control Plane, Workers, CLI)
- Worker trait and worker pool
- Control plane scheduling and domain center orchestration
- Capacity management (global + per-type limits)
- Circuit breaking and backpressure
- Semantic dedup
- Dedup time window (recently-completed)
- Child work spawning and continuations
- IPC layer (Unix domain socket)
- CLI commands (status, list, show, logs)
- SQLx offline metadata for CI

## Open Design Questions

- **Worker interface**: trait-based or message-passing? In-process only or support child processes?
- **IPC protocol**: JSON lines vs protobuf over Unix domain socket
- **Semantic dedup**: embedding similarity threshold, when to invoke, cost control
- **Priority formula**: system-provided age boost, or fully host-controlled?
- **Configuration**: TOML for work type definitions (capacity, retry policy, priority)
- **Child work**: how to express continuations that run when all children complete
- **Embedding provider**: Anthropic doesn't support embeddings via rig-core; need a separate provider
