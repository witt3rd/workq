# animus-rs Design

*Substrate for relational beings — data plane, control plane, LLM abstraction, and observability, built on Postgres.*

## Origin

animus-rs started as `workq`, a standalone work-tracking engine. When we discovered pgmq (Postgres queue extension), it became clear that pgmq already provides the queue primitives workq was hand-rolling. The project pivoted: build the full Animus system as one well-structured Rust crate — data plane (work queues, semantic memory), control plane (queue watching, resource gating, focus spawning), faculties (pluggable cognitive specializations), and observability.

The predecessor system used filesystem-based storage (YAML task queues, markdown substrate, ChromaDB for vectors, JSONL logs). It worked but had real limitations: no structural dedup, no transactional guarantees, fragile file-based queues, a separate ChromaDB process. animus-rs replaces all of this with Postgres + extensions.

## Design Principles

### 1. Work Has Identity

A work item has semantic identity. "Check in with Kelly" is the same work whether it came from a user request, an extracted initiative, or a heartbeat skill. **Structural dedup** on `(work_type, dedup_key)` collapses duplicates transactionally. Semantic dedup (embedding-based) is a future extension.

### 2. Work, Not Messages

Work items don't have `from` or `to` fields. They have what needs doing (`work_type` + `params`), why it exists (`provenance`), priority, and lifecycle state. The caller says "this work needs doing" and the system figures out how.

### 3. Faculties and Foci, Not Fixed Processes

A **faculty** is a cognitive specialization — Social, Initiative, Heartbeat, Radiate, Computer Use, or domain-specific. Faculties are **configuration, not code** — adding a faculty means adding a TOML file, not writing Rust.

A **focus** is a single activation of a faculty on a specific work item. Ephemeral, atomic, self-contained. Four phases: Orient → Engage → Consolidate → Recover. Orient and consolidate are external hooks (any executable). Engage is a built-in agentic loop configured by the faculty. Recover handles failures.

### 4. Work-Once Guarantee

A work item is claimed by exactly one focus (pgmq visibility timeout). Duplicates are detected and merged. Every work item either completes, fails (with retry), or goes dead — nothing disappears silently.

### 5. Postgres Is the Platform

Postgres with pgmq + pgvector is a deliberate choice, not a swappable backend. Queue semantics, vector search, transactional guarantees, and the work ledger all live in one database. One operational dependency.

### 6. Observability Is Product

Every animus ships with integrated three-signal OTel observability (traces, metrics, logs) through the Grafana stack. You can see what your agent is doing out of the box. Postgres stores domain state; OTel handles observability. No custom event tables.

---

## Architecture Overview

```
┌───────────────────────────────────────────────────────────────────────┐
│  ANIMUS APPLIANCE                                                     │
│                                                                       │
│  ┌─ Control Plane ──────────────────────────────────────────────────┐ │
│  │  Queue watching (pg_notify), faculty routing, capacity mgmt      │ │
│  │  Spawns foci when work is ready and faculty has capacity          │ │
│  └──────────────────────────────────────────────────────────────────┘ │
│           │                                                           │
│  ┌─ Focus (one activation on one work item) ────────────────────────┐ │
│  │  Orient → Engage → Consolidate (→ Recover on failure)            │ │
│  │                                                                   │ │
│  │  Orient:  External hook + awareness digest injection              │ │
│  │  Engage:  Built-in agentic loop (LLM + tools + ledger + sandbox) │ │
│  │  Consolidate: External hook (reads ledger, creates memories/skills)│ │
│  └──────────────────────────────────────────────────────────────────┘ │
│           │                                                           │
│  ┌─ Data Plane (Postgres) ──────────────────────────────────────────┐ │
│  │  work_items (lifecycle, dedup, parent-child)                      │ │
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

Postgres schema, SQLx migrations, two-layer data access (direct SQLx for queues/work items, rig-postgres for vector search). Work item lifecycle, structural dedup, pgmq operations, memory storage and hybrid search.

### Work Ledger — [docs/ledger.md](docs/ledger.md)

Postgres-backed durable working memory for the agentic loop. Append-only typed entries (plan, finding, decision, step, error, note) that the agent maintains during its engage loop via `ledger_append` / `ledger_read` tools. The engine uses the ledger for context compaction. The consolidate hook reads it for post-processing. Cross-faculty findings feed the awareness digest.

### Engage Phase — [docs/engage.md](docs/engage.md)

The agentic loop architecture. Five interconnected concerns:

1. **Bounded sub-contexts** — agent-declared context scoping via `ledger_append(step)` entries. Closed blocks are replaced with their ledger stubs. The open block is preserved verbatim.
2. **Parallel tool execution** — multiple `tool_use` blocks execute concurrently via `tokio::JoinSet`.
3. **Child work items** — async delegation via the work queue. `spawn_child_work` creates a child with its own focus, ledger, and context window. `await_child_work` blocks via `pg_notify`.
4. **The awareness digest** — engine-level cross-faculty coherence. Assembled at orient from `work_items` + `work_ledger`. Shows running siblings, recent completions, and cross-faculty findings. Default-on because coherence is the baseline.
5. **Code execution sandbox** — Docker-based Python sandbox for programmatic tool composition. The agent writes code that calls tools as functions; the code's return value (not raw tool output) enters context. Handles output management, multi-step composition, conditional logic, and agent-controlled timeouts.

The engage phase is a built-in engine loop, not an external process. Orient, consolidate, and recover remain external hooks. An escape hatch (`mode = "external"`) allows faculties to override the built-in loop.

### Skills — [docs/skills.md](docs/skills.md)

Progressive discovery, runtime activation, and autopoietic evolution. Three levels:

1. **Runtime skills** — YAML frontmatter + markdown body, discovered and activated during the engage loop. Auto-activation during orient based on work type triggers.
2. **Autopoietic skills** — created by the agent from its own experience. The consolidate hook detects recurring ledger patterns and encodes them as skills for future foci.
3. **System skills** — composable code modifications (nanoclaw-style) that extend the system with new tools, faculties, or hooks. Future.

Engine tools: `discover_skills`, `activate_skill`, `create_skill`. Skill scripts callable from the code execution sandbox.

### LLM Abstraction — [docs/llm.md](docs/llm.md)

Thin, provider-specific HTTP clients replacing rig-core for LLM calls. `LlmClient` trait with two methods: `complete` and `complete_stream`. Anthropic Messages API and OpenAI Chat Completions API implementations, ~400-500 lines total. Raw `reqwest` + SSE parsing, no framework.

The engage loop calls `LlmClient` directly — one call per iteration. The client makes HTTP requests and returns data. All orchestration (context management, tool execution, ledger, hooks) lives in the engage loop, not in the LLM abstraction.

rig-postgres retained for pgvector/embedding search.

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

| State | Meaning |
|---|---|
| **Created** | Submitted, pending dedup check |
| **Queued** | In pgmq, waiting for a focus |
| **Claimed** | Focus assigned via pgmq read (visibility timeout) |
| **Running** | Focus actively processing |
| **Completed** | Done successfully |
| **Failed** | Execution error, may be retried |
| **Dead** | Exhausted retries or poisoned — terminal |
| **Merged** | Structural dedup hit — linked to canonical item, terminal |

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
| **Orient** | External hook | Gather context, inject awareness digest, auto-activate skills |
| **Engage** | Built-in engine loop | Agentic tool-use iteration with ledger, sandbox, child work |
| **Consolidate** | External hook | Integrate results: store memories, create skills, emit follow-up work |
| **Recover** | External hook | Assess failure, decide retry or dead-letter |

---

## Faculty Configuration

Faculties are TOML — configuration, not code:

```toml
[faculty]
name = "social"
accepts = ["engage", "respond", "check-in"]
max_concurrent = 3

[faculty.orient]
command = "scripts/social-orient"
auto_activate_skills = true

[faculty.engage]
model = "claude-sonnet-4-5-20250514"
system_prompt_file = "prompts/social.md"
tools = ["memory-search", "calendar", "send-message"]
max_turns = 50
parallel_tool_execution = true
code_execution = true
ledger_nudge_interval = 5
truncate_closed_blocks = true

[faculty.awareness]
enabled = true
lookback_hours = 24

[faculty.consolidate]
command = "scripts/social-consolidate"
skill_creation = true

[faculty.recover]
command = "scripts/recover-default"
max_attempts = 3
backoff = "exponential"
```

See [docs/engage.md](docs/engage.md) for the complete configuration reference.

---

## Deployment

### The Animus Appliance

One `docker compose up` starts a complete agent with integrated observability:

| Container | Purpose |
|---|---|
| **animus-rs** | The agent service |
| **postgres** | Postgres + pgmq + pgvector |
| **otel-collector** | OTLP ingestion |
| **tempo** | Trace storage |
| **loki** | Log aggregation |
| **prometheus** | Metrics storage |
| **grafana** | Unified observability UI |

Fleet deployment uses a shared observer stack (`docker-compose.observer.yml`). See [docs/db.md](docs/db.md) for database configuration and schema details.

---

## Implementation Status

### Implemented
- Config, DB pool, SQLx migrations
- pgmq operations, work items with structural dedup
- Semantic memory (pgvector, hybrid BM25+vector search)
- Three-signal OTel pipeline with GenAI semantic conventions
- Full observability stack (OTel Collector + Tempo + Prometheus + Loki + Grafana)
- Docker Compose appliance with Postgres + observability
- Core types, state machine, test suite
- Control plane, faculty system, focus lifecycle

### Designed (Not Yet Implemented)
- Engage loop with bounded sub-contexts and parallel tools → [docs/engage.md](docs/engage.md)
- Work ledger (Postgres-backed durable working memory) → [docs/ledger.md](docs/ledger.md)
- Code execution sandbox → [docs/engage.md](docs/engage.md) § 5
- Child work items (async delegation) → [docs/engage.md](docs/engage.md) § 3
- Awareness digest (cross-faculty coherence) → [docs/engage.md](docs/engage.md) § 4
- Skills system (discovery, activation, autopoiesis) → [docs/skills.md](docs/skills.md)
- Thin LLM client (replacing rig-core for completions) → [docs/llm.md](docs/llm.md)

### Open Design Questions
- **Semantic dedup**: embedding similarity threshold, when to invoke, cost control
- **Priority formula**: system-provided age boost, or fully host-controlled?
- **Embedding provider**: separate from LLM provider; need to evaluate options
- **Awareness digest freshness**: refresh during long foci, or orient-time only?
- **Skill sharing across animi**: fleet skill propagation and identity implications

See the open questions sections in each subsystem doc for domain-specific questions.

---

## Research

- [docs/research/microclaw/agent.md](docs/research/microclaw/agent.md) — Deep analysis of MicroClaw's agent loop (tool system, hooks, permissions, context management, LLM integration). Informed the engage loop and ledger designs.
