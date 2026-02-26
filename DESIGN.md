# Work Engine Design

*Adapted from the original bus_new spec, which seeded this project.*

## The Core Reframe

workq is a **work engine**, not a message bus. Work items exist, have identity, go through a lifecycle, and the engine ensures they get done exactly once.

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

**Structural dedup** (engine-provided): exact match on `(work_type, dedup_key)` — e.g., `("engage", "person=kelly")` collapses duplicate requests targeting the same person.

**Semantic dedup** (host-provided): LLM or embedding-based similarity for "is this initiative basically the same as that heartbeat task?" — more expensive, used selectively. The engine provides a hook; the host implements the logic.

### 2. Work, Not Messages

Work items don't have `from` or `to` fields. They have:
- **What** needs doing — `work_type` + `params`
- **Why** it exists — `provenance` (source + trigger)
- **Priority** — how urgent
- **State** — lifecycle position

Routing is the engine's job. The caller says "this work needs doing" and the engine figures out how.

### 3. Dynamic Workers, Not Fixed Processes

Workers are ephemeral — they exist to do one unit of work, then they're done. The engine manages:
- **Global capacity** — total concurrent workers
- **Per-type capacity** — e.g., max 3 LLM workers
- **Backpressure** — queuing with priority ordering
- **Circuit breaking** — exponential backoff on failing work types
- **Poison pill detection** — dead-lettering items that fail repeatedly

### 4. Work-Once Guarantee

- A work item is **claimed** by exactly one worker
- **Duplicate** work is detected and merged (structural or semantic)
- Every work item either **completes**, **fails** (with retry), or goes **dead** — nothing disappears silently

### 5. Standalone and Reusable

workq is a library, not an application. It knows nothing about LLMs, agents, or domain logic. The host provides:
- **Worker implementations** — what actually does the work
- **Dedup policies** — how to recognize semantic duplicates
- **Scheduling policies** — priority formulas, capacity limits

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
| **Created** | Submitted, not yet dedup-checked |
| **Queued** | Ready for execution, waiting for a worker |
| **Claimed** | Worker assigned, execution starting |
| **Running** | Worker actively processing |
| **Completed** | Done successfully |
| **Failed** | Unexpected failure, may be retried |
| **Dead** | Exhausted retries or poisoned — terminal |
| **Merged** | Duplicate of existing work — linked to canonical item, terminal |

### Valid State Transitions

```
Created  → Queued       (passed dedup, ready for execution)
Created  → Merged       (structural dedup hit)
Queued   → Claimed      (worker assigned)
Queued   → Dead         (cancelled or circuit-broken)
Claimed  → Running      (worker started)
Claimed  → Queued       (worker failed to start, re-queue)
Running  → Completed    (success)
Running  → Failed       (execution error)
Failed   → Queued       (retry)
Failed   → Dead         (exhausted retries)
```

### Provenance

Every work item tracks where it came from:

```rust
Provenance {
    source: "heartbeat",        // What created it
    trigger: "skill/check-in",  // More specific origin
}
```

When work is merged, both provenances are preserved via `merged_provenance`. You can always answer "why does this work exist?" and "who asked for it?"

## Structural Dedup

When creating work with a `dedup_key`, the engine checks: is there already a work item with the same `(work_type, dedup_key)` that is queued, claimed, or running? If so, **merge** — the new item links to the existing one, both provenances are recorded, only one execution happens.

The entire submit flow (insert + dedup check + merge-or-queue + event recording) runs within a single SQLite transaction. This guarantees crash safety (no orphaned `Created` items) and correctness under concurrent access (two submits with the same dedup key cannot both slip through). The transactional boundary (`TxContext`) is designed so additional dedup strategies can be plugged in within the same atomic operation.

The dedup window covers all non-terminal states. A configurable time-based window for recently-completed work is a planned extension ("don't re-check a project within 1 hour of the last check").

### Dedup Verdicts (Future)

| Verdict | Meaning |
|---------|---------|
| **Distinct** | Not the same work — both execute |
| **Merge** | Same work — merge into existing, one execution |
| **Supersede** | New work replaces old (e.g., updated instructions) |
| **Defer** | Same work but hold off — context may have changed |

Currently the engine implements Distinct and Merge. Supersede and Defer are extension points for the host.

## Observability

Observability is a first-class design concern. The engine is **designed to be watched**.

### Event Stream

Every state transition emits a structured event with a monotonic sequence number:

```
WorkCreated { id, work_type, dedup_key, priority, source }
WorkMerged { id, canonical_id, reason }
WorkQueued { id, priority }
WorkClaimed { id, worker_id }
WorkRunning { id, worker_id }
WorkCompleted { id, duration_ms }
WorkFailed { id, error, retryable, attempt }
WorkDead { id, reason, attempts }
WorkSpawned { parent_id, child_ids }
```

Events are the engine's voice — state transitions and scheduling decisions. Logs are the worker's voice — what's happening inside execution.

### Work-Scoped Logging

Every work item has a log. No global log stream, no console streaming. Logs are scoped to work items and stored with them.

```rust
engine.log(work_id, LogLevel::Info, "Starting engagement with Kelly");
engine.log(work_id, LogLevel::Error, "API timeout after 30s");
```

Failure diagnosis = pull the item's logs. No grepping through shared log files.

### Snapshot State (Future)

Queryable system state via CLI and API:

```bash
workq status                            # Overview
workq list --state=running              # What's executing
workq list --state=queued --type=engage # What's waiting
workq show <id>                         # Full item history
workq logs <id>                         # Item logs
workq logs <id> --follow                # Tail running item
```

## Worker Interface (Future)

Workers are host-provided. The engine defines the contract:

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

The engine doesn't care what the worker does. It cares about:
- Did it succeed or fail?
- How long did it take?
- Did it spawn child work?

## Storage

SQLite with WAL mode. Single file, embeddable, queryable, supports concurrent readers.

The database is the single source of truth. Events are emitted as side effects of state transitions. Queries read the database directly. No divergence possible.

## Deployment Model (Future)

One engine process, dynamic workers. The engine is a daemon that owns the SQLite database, runs the scheduler, and spawns workers as needed.

Workers run in-process (Rust-native) or as child processes (Python, etc.) spawned by the engine. The engine caps concurrency globally and per-type.

External work sources (Telegram, CLI, other apps) submit work via IPC (Unix domain socket). They're separate processes — work sources, not workers.

## Implementation Status

### Implemented
- Core types: WorkItem, State, Provenance, Outcome, LogEntry, NewWorkItem builder
- State machine with enforced valid transitions
- Structural dedup on `(work_type, dedup_key)`
- Transactional submit (atomic insert + dedup check + state transition + events)
- Merge with provenance preservation and state transition validation
- Priority-ordered claiming
- Retry with configurable max attempts
- Poison pill detection (fail → dead after exhausted retries)
- Work-scoped logging
- Structured event stream with monotonic sequencing
- SQLite storage with WAL mode and partial indexes
- Transaction support (`TxContext` + `with_transaction`) for multi-step atomic operations
- In-memory mode for tests
- Integration test suite (15 tests)

### Not Yet Implemented
- Worker trait and worker pool
- Capacity management (global + per-type limits)
- Circuit breaking
- Semantic dedup hook (transactional infrastructure in place via `TxContext`)
- Dedup time window (recently-completed)
- Supersede / Defer dedup verdicts
- Child work spawning and continuations
- IPC layer (Unix domain socket)
- CLI commands (status, list, show, logs, watch)
- `workq serve` daemon mode
- Metrics aggregation
- Checkpointing for long-running work

## Open Design Questions

- **IPC protocol**: JSON lines vs protobuf over Unix domain socket
- **Semantic dedup hook**: `TxContext` provides the transactional execution boundary — remaining question is the host-facing API shape (trait-based callback vs channel-based async)
- **Priority formula**: engine-provided age boost, or fully host-controlled?
- **Configuration**: TOML for work type definitions (capacity, retry policy, priority)
- **Child work**: how to express continuations that run when all children complete
