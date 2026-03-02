# animus-rs Design

*Substrate for relational beings — data plane, control plane, LLM abstraction, and observability, built on Postgres.*

## Origin

animus-rs started as `workq`, a standalone work-tracking engine. When we discovered pgmq (Postgres queue extension), it became clear that pgmq already provides the queue primitives workq was hand-rolling. The project pivoted: build the full Animus system as one well-structured Rust crate — data plane (work queues, semantic memory), control plane (queue watching, resource gating, focus spawning), faculties (pluggable cognitive specializations), and observability.

The predecessor system used filesystem-based storage (YAML task queues, markdown substrate, ChromaDB for vectors, JSONL logs). It worked but had real limitations: no structural dedup, no transactional guarantees, fragile file-based queues, a separate ChromaDB process. animus-rs replaces all of this with Postgres + extensions.

## Design Principles

### 1. Work Has Identity

A work item has semantic identity. "Check in with Kelly" is the same work whether it came from a user request, an extracted initiative, or a heartbeat skill. **Structural dedup** on `(faculty, dedup_key)` collapses duplicates transactionally. Semantic dedup (embedding-based) is a future extension.

### 2. Work, Not Messages

Work items have: which **faculty** handles them, which **skill** drives the methodology, **params** for the specific task, **provenance** (where it came from), **priority**, and lifecycle **state**. The caller says who should do this, how, and with what context.

### 3. Faculties and Foci, Not Fixed Processes

A **faculty** is a cognitive specialization — Social, Initiative, Heartbeat, Radiate, Engineer, or domain-specific. Faculties are **configuration, not code** — adding a faculty means adding a TOML file, not writing Rust. The faculty provides infrastructure: model, tools, concurrency mode, isolation strategy.

A **focus** is a single activation of a faculty on a specific work item. Ephemeral, atomic, self-contained. Four phases: Orient → Engage → Consolidate → Recover.

**The work item specifies the faculty directly** — no routing table, no `accepts` list. The submitter says `faculty: "engineer"` and the control plane dispatches to that faculty.

**Skills provide methodology.** The work item carries a `skill` field that tells the engage loop *how* to work. The same faculty can execute different skills: `tdd-implementation` for building, `systematic-debugging` for fixing, `code-review` for reviewing. Skills are cross-faculty — any faculty that writes code can use the TDD skill.

**Concurrency is separated into capability and allocation.** The faculty declares *whether* it supports parallel foci and *how* they're isolated (e.g., git worktrees). The control plane decides *how many* to run based on global resource limits.

### 4. Work-Once Guarantee

A work item is claimed by exactly one focus (pgmq visibility timeout). Duplicates are detected and merged. Every work item either completes, fails (with retry), or goes dead — nothing disappears silently.

### 5. Postgres Is the Platform

Postgres with pgmq + pgvector is a deliberate choice, not a swappable backend. Queue semantics, vector search, transactional guarantees, the work ledger, and orient/consolidate context all live in one database. One operational dependency. All phase communication goes through the database — not the filesystem.

### 6. Observability Is Product

Every animus ships with integrated three-signal OTel observability (traces, metrics, logs) through the Grafana stack. You can see what your agent is doing out of the box. Postgres stores domain state; OTel handles observability. No custom event tables.

---

## Architecture Overview

```
┌───────────────────────────────────────────────────────────────────────┐
│  ANIMUS APPLIANCE                                                     │
│                                                                       │
│  ┌─ Control Plane ──────────────────────────────────────────────────┐ │
│  │  Queue watching (pg_notify), faculty dispatch, capacity mgmt      │ │
│  │  Reads faculty name from work item, dispatches to matching faculty │ │
│  └──────────────────────────────────────────────────────────────────┘ │
│           │                                                           │
│  ┌─ Focus (one activation on one work item) ────────────────────────┐ │
│  │  Orient → Engage → Consolidate (→ Recover on failure)            │ │
│  │                                                                   │ │
│  │  Orient:  External hook — writes context to DB (awareness digest) │ │
│  │  Engage:  Built-in agentic loop (LLM + skill + tools + ledger)   │ │
│  │  Consolidate: External hook — reads ledger from DB                │ │
│  └──────────────────────────────────────────────────────────────────┘ │
│           │                                                           │
│  ┌─ Data Plane (Postgres) ──────────────────────────────────────────┐ │
│  │  work_items (faculty, skill, lifecycle, dedup, parent-child)      │ │
│  │  work_ledger (durable working memory per work item)               │ │
│  │  pgmq queues (claim, visibility timeout, dead letter)             │ │
│  │  memories (pgvector embeddings, hybrid BM25+vector search)        │ │
│  └──────────────────────────────────────────────────────────────────┘ │
│           │                                                           │
│  ┌─ Observability ──────────────────────────────────────────────────┐ │
│  │  OTel Collector → Tempo (traces) + Prometheus (metrics) + Loki   │ │
│  │  Grafana (:3000) — pre-wired, cross-linked datasources           │ │
│  └──────────────────────────────────────────────────────────────────┘ │
└───────────────────────────────────────────────────────────────────────┘
```

---

## Subsystem Designs

Each subsystem has a detailed design document. DESIGN.md is the high-level overview; the subsystem docs are authoritative for implementation details.

### Data Plane — [docs/db.md](docs/db.md)

Postgres schema, SQLx migrations, two-layer data access (direct SQLx for queues/work items, rig-postgres for vector search). Work item lifecycle, structural dedup on `(faculty, dedup_key)`, pgmq operations, memory storage and hybrid search.

### Work Ledger — [docs/ledger.md](docs/ledger.md)

Postgres-backed durable working memory for the agentic loop. Append-only typed entries (plan, finding, decision, step, error, note) that the agent maintains during its engage loop via `ledger_append` / `ledger_read` tools. The engine uses the ledger for context compaction. The consolidate hook reads it for post-processing. Cross-faculty findings feed the awareness digest.

### Engage Phase — [docs/engage.md](docs/engage.md)

The agentic loop architecture. The engage loop is a generic iteration engine — LLM call, tool execution, repeat. All behavioral specificity comes from the **skill** activated for the work item. Five infrastructure concerns: bounded sub-contexts, parallel tool execution, child work items, the awareness digest, and the code execution sandbox.

### Skills — [docs/skills.md](docs/skills.md)

Skills are methodology — they tell the engage loop *how* to work. Progressive discovery, runtime activation, and autopoietic evolution. The work item's `skill` field determines which skill is activated. Skills are flat (not namespaced by faculty) because methodology is orthogonal to infrastructure.

### LLM Abstraction — [docs/llm.md](docs/llm.md)

Thin, provider-specific HTTP clients. `LlmClient` trait with two methods: `complete` and `complete_stream`. The engage loop calls it directly — one call per iteration.

### CLI — [docs/cli.md](docs/cli.md)

Operator interface. `animus serve` (daemon), `animus work submit/list/show` (work management), `animus ledger show/append` (future).

### Operations — [docs/ops.md](docs/ops.md)

Observability stack, backups, alerting, multi-instance deployment, configuration reference.

---

## Work Item

A work item carries everything the system needs to execute it:

| Field | Purpose |
|---|---|
| `faculty` | Which faculty handles this (replaces `work_type` routing) |
| `skill` | Which skill drives the methodology (e.g., `tdd-implementation`) |
| `dedup_key` | Structural dedup within the faculty |
| `params` | Task-specific context (spec path, description, etc.) |
| `provenance` | Where it came from (source + trigger) |
| `priority` | Urgency (higher = more urgent) |
| `state` | Lifecycle position |
| `parent_id` | If spawned by another work item |

Dedup is on `(faculty, dedup_key)`. The submitter specifies the faculty directly — no routing table.

---

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

Transitions enforced by `State::can_transition_to()`.

---

## Focus Lifecycle

```
Orient → Engage → Consolidate
                      ↓ (on failure at any phase)
                   Recover → Requeue or Dead
```

| Phase | Driver | Purpose |
|---|---|---|
| **Orient** | External hook | Gather context, write to DB, inject awareness digest |
| **Engage** | Built-in engine loop | Activate skill, iterate with LLM + tools + ledger |
| **Consolidate** | External hook | Read ledger from DB, store memories, create skills |
| **Recover** | External hook | Assess failure, decide retry or dead-letter |

All phase communication goes through Postgres — not the filesystem. The focus directory is scratch space only.

---

## Faculty Configuration

Faculties are TOML — configuration, not code:

```toml
[faculty]
name = "social"
concurrent = false

[faculty.orient]
command = "scripts/social-orient"

[faculty.engage]
model = "claude-sonnet-4-5-20250514"
tools = ["memory-search", "calendar", "send-message"]
max_turns = 50

[faculty.awareness]
enabled = true

[faculty.consolidate]
command = "scripts/social-consolidate"

[faculty.recover]
command = "scripts/recover-default"
max_attempts = 3
backoff = "exponential"
```

```toml
[faculty]
name = "engineer"
concurrent = true
isolation = "worktree"

[faculty.engage]
model = "claude-sonnet-4-5-20250514"
tools = ["read_file", "write_file", "edit_file", "bash", "grep", "glob"]
max_turns = 100
code_execution = true

[faculty.awareness]
enabled = true

[faculty.consolidate]
command = "scripts/engineer-consolidate"

[faculty.recover]
command = "scripts/recover-default"
max_attempts = 2
backoff = "exponential"
```

No `accepts` field — the work item specifies `faculty: "engineer"` directly. No `system_prompt_file` — the skill provides the methodology.

---

## Implementation Status

### Implemented
- Config, DB pool, SQLx migrations
- pgmq operations, work items with structural dedup
- Semantic memory (pgvector, hybrid BM25+vector search)
- Three-signal OTel pipeline with GenAI semantic conventions
- Full observability stack (OTel Collector + Tempo + Prometheus + Loki + Grafana)
- Docker Compose appliance with durable host-mounted volumes
- Core types, state machine, test suite
- Control plane, faculty system, focus lifecycle
- CLI: `animus serve`, `animus work submit/list/show`
- Engineer faculty (stub hooks, placeholder engage)
- Grafana dashboard: Animus Work Queue (Postgres + Prometheus)
- Unroutable work detection with metric + alert

### Designed (Not Yet Implemented)
- Engage loop with bounded sub-contexts and parallel tools → [docs/engage.md](docs/engage.md)
- Work ledger (Postgres-backed durable working memory) → [docs/ledger.md](docs/ledger.md)
- Skills system (discovery, activation, autopoiesis) → [docs/skills.md](docs/skills.md)
- Thin LLM client (replacing rig-core for completions) → [docs/llm.md](docs/llm.md)
- DB-based orient/consolidate (replacing filesystem protocol)
- `faculty` + `skill` fields on work items (replacing `work_type` routing)

### Open Design Questions
- **Semantic dedup**: embedding similarity threshold, when to invoke, cost control
- **Priority formula**: system-provided age boost, or fully host-controlled?
- **Embedding provider**: separate from LLM provider; need to evaluate options

See the open questions sections in each subsystem doc for domain-specific questions.

---

## Research

- [docs/research/microclaw/agent.md](docs/research/microclaw/agent.md) — Deep analysis of MicroClaw's agent loop. Informed the engage loop and ledger designs.
