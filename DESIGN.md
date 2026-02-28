# animus-rs Design

*Substrate for relational beings — data plane, control plane, LLM abstraction, and observability, built on Postgres.*

## Origin

animus-rs started as `workq`, a standalone work-tracking engine. When we discovered pgmq (Postgres queue extension), it became clear that pgmq already provides the queue primitives workq was hand-rolling. The project pivoted: build the full Animus system as one well-structured Rust crate — data plane (work queues, semantic memory), control plane (queue watching, resource gating, focus spawning), faculties (pluggable cognitive specializations), and observability.

The predecessor system used filesystem-based storage (YAML task queues, markdown substrate, ChromaDB for vectors, JSONL logs). It worked but had real limitations: no structural dedup, no transactional guarantees, fragile file-based queues, a separate ChromaDB process. animus-rs replaces all of this with Postgres + extensions.

## The Core Reframe

animus-rs is a **work engine**, not a message bus. Work items exist, have identity, go through a lifecycle, and the system ensures they get done exactly once.

This is not request/reply. This is: **something needs doing**.

| Message bus | Work engine |
|-------------|-------------|
| Task is a message between processes | Work item is a thing that needs doing |
| Fixed processes poll their queues | Foci spin up on demand |
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

### 3. Faculties and Foci, Not Fixed Processes

A **faculty** is a cognitive specialization — Social, Initiative, Heartbeat, Radiate, Computer Use, or domain-specific. Each faculty defines its own lifecycle hooks and agentic loop configuration.

A **focus** is a single activation of a faculty on a specific work item. The control plane spawns a focus (subprocess) when work is ready and resources are available. Each focus runs through four phases:

1. **Orient** — pre-hook: prepare context, gather what's needed
2. **Engage** — agentic loop: reason, plan, use tools, iterate until self-termination
3. **Consolidate** — post-hook: integrate results, clean up
4. **Recover** — exception-hook: on failure, assess and stabilize (requeue or dead-letter)

Foci are ephemeral — they exist to do one unit of work, then they're done. The control plane manages:
- **Global capacity** — total concurrent foci
- **Per-faculty capacity** — e.g., max 3 Social foci
- **Backpressure** — queuing with priority ordering
- **Circuit breaking** — exponential backoff on failing faculties
- **Poison pill detection** — dead-lettering items that fail repeatedly

### 4. Work-Once Guarantee

- A work item is **claimed** by exactly one focus (pgmq visibility timeout)
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
| **Queued** | In pgmq, waiting for a focus to pick it up |
| **Claimed** | Focus assigned via pgmq read (visibility timeout) |
| **Running** | Focus actively processing |
| **Completed** | Done successfully |
| **Failed** | Execution error, may be retried |
| **Dead** | Exhausted retries or poisoned — terminal |
| **Merged** | Structural dedup hit — linked to canonical item, terminal |

### Valid State Transitions

```
Created  → Queued       (passed dedup, sent to pgmq)
Created  → Merged       (structural dedup hit)
Queued   → Claimed      (focus assigned via pgmq.read)
Queued   → Dead         (cancelled or circuit-broken)
Claimed  → Running      (focus started)
Claimed  → Queued       (focus failed to start, re-queue)
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

## Faculty / Focus Model (Future)

A **faculty** is a cognitive specialization — Social, Initiative, Heartbeat, Radiate, Computer Use, or domain-specific. A **focus** is a single activation of a faculty on a specific work item.

Faculties are **configuration, not code**. The animus-rs engine is the runtime; faculties are data that tell the engine what to do. Adding a faculty means adding a config file, not writing Rust.

### Faculty Configuration

Each faculty is defined in TOML. The engine loads all faculty configs at startup and builds a registry.

```toml
[faculty]
name = "social"
accepts = ["engage", "respond", "check-in"]
max_concurrent = 3

[faculty.orient]
command = "scripts/social-orient"

[faculty.engage]
model = "claude-sonnet-4-5-20250514"
system_prompt_file = "prompts/social.md"
tools = ["memory-search", "calendar", "send-message"]
max_turns = 50

[faculty.consolidate]
command = "scripts/social-consolidate"

[faculty.recover]
command = "scripts/recover-default"
max_attempts = 3
backoff = "exponential"
```

### Hook Commands

Hooks are **external processes** — any executable the OS can run. The engine doesn't care about the implementation language:

- Shell scripts (`bash`, `zsh`)
- Python (`python`, `uv run`, `uvx`)
- Node/TypeScript (`node`, `npx`, `tsx`)
- Compiled binaries (Rust, C, Go, whatever)

The engine needs a path to an executable. That's it. The command is spawned as a subprocess via `tokio::process::Command`.

**What the engine provides to hooks:** The engine writes a focus context file (JSON) to a known location before invoking each hook. This contains the work item, faculty config, and any state from prior phases. The hook reads what it needs, does its work, and writes its output to a known location. Decoupled — no IPC protocol, no stdin/stdout framing, just files.

```
/tmp/animus/foci/{focus-id}/
  context.json      # engine writes: work item, faculty config, phase
  orient-out.json   # orient hook writes its output here
  engage-out.json   # engage phase writes outcome here
  consolidate-out.json  # consolidate hook writes here
```

**Open question:** What arguments, if any, do hooks need beyond the context file path? Options:
- **Minimal:** just the path to the focus directory. Hook reads `context.json` for everything.
- **Convenience args:** path + work type + work item ID as positional args, for hooks that want to dispatch without parsing JSON.
- **Environment variables:** `ANIMUS_FOCUS_DIR`, `ANIMUS_WORK_TYPE`, `ANIMUS_WORK_ID` — available to all hooks, no argument parsing needed.

These aren't mutually exclusive. The engine can provide all three and hooks use whichever is convenient.

### Focus Lifecycle

When a work item is ready and the faculty has capacity, the control plane spawns a **focus** (subprocess). The focus runs through four phases:

```
Orient → Engage → Consolidate
                      ↓ (on failure at any phase)
                   Recover → Requeue or Dead
```

1. **Orient** — prepare context for the agentic loop. The orient hook gathers relevant memories, prior conversation state, external context — whatever the faculty needs. Output goes to the focus directory for the engage phase to pick up.

2. **Engage** — the agentic loop. The engine drives an LLM conversation: system prompt (from faculty config), tools (from faculty config), context (from orient output). The loop runs until the agent self-terminates or hits `max_turns`.

3. **Consolidate** — integrate results back into the substrate. The consolidate hook reads the engage output and does post-processing: store new memories, update relationship state, emit follow-up work items, whatever the faculty needs.

4. **Recover** — on failure at any phase. The recover hook assesses what went wrong and decides: requeue (try again) or dead-letter (unrecoverable). Recovery can also do cleanup — release resources, log diagnostics, notify.

The control plane doesn't steer foci — once spawned, a focus is self-directed. The engine cares about:
- Did it succeed or fail?
- How long did it take?
- Did it spawn child work?

### Rust Data Model

Faculty is a struct deserialized from config, not a trait:

```rust
struct Faculty {
    name: String,
    accepts: Vec<String>,
    max_concurrent: usize,
    orient: HookConfig,
    engage: EngageConfig,
    consolidate: HookConfig,
    recover: RecoverConfig,
}

struct HookConfig {
    command: PathBuf,  // path to executable
}

struct EngageConfig {
    model: String,
    system_prompt_file: PathBuf,
    tools: Vec<String>,
    max_turns: usize,
}

struct RecoverConfig {
    command: PathBuf,
    max_attempts: u32,
    backoff: BackoffStrategy,
}

struct FacultyRegistry {
    faculties: HashMap<String, Faculty>,
}
```

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

### Not Yet Implemented (Milestones 3+: Control Plane, Faculties, CLI)
- Faculty config loading (TOML → FacultyRegistry)
- Focus lifecycle engine (Orient → Engage → Consolidate → Recover)
- Hook subprocess spawning (tokio::process::Command)
- Focus context directory and JSON protocol
- Agentic loop driver (model, system prompt, tools, max turns)
- Control plane: queue watching (async notify), resource gating, focus spawning
- Capacity management (global + per-faculty limits)
- Circuit breaking and backpressure
- Semantic dedup
- Dedup time window (recently-completed)
- Child work spawning and continuations
- CLI commands (status, list, show, logs)
- SQLx offline metadata for CI

## Open Design Questions

- **Hook argument passing**: minimal (just focus dir path), convenience positional args, env vars, or all three?
- **Engage phase driver**: how does the engine drive the agentic loop? Subprocess wrapping an LLM CLI? In-process rig-core? Both as options?
- **Focus context protocol**: what exactly goes in `context.json` and what do output files look like?
- **Faculty config location**: single directory of TOML files? Embedded in a master config? Per-faculty directories with prompt files alongside?
- **Semantic dedup**: embedding similarity threshold, when to invoke, cost control
- **Priority formula**: system-provided age boost, or fully host-controlled?
- **Child work**: how to express continuations that run when all children complete
- **Embedding provider**: Anthropic doesn't support embeddings via rig-core; need a separate provider
