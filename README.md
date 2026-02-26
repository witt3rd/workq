# workq

A work-tracking engine. Not a message bus — a system that ensures work gets done exactly once.

## What This Is

workq tracks work items through their lifecycle: submit → dedup → queue → claim → execute → complete. It provides structural deduplication, priority scheduling, retry with poison pill detection, and full observability via events and work-scoped logs.

The engine is a library. The host application provides worker implementations and policies. workq knows nothing about what the work *is* — it knows about work items, lifecycle, scheduling, and guarantees.

## Status

**Early development.** Core engine works: submit, dedup, claim, start, complete, fail, retry, logs, events. Not yet built: worker trait, scheduling, circuit breaking, IPC, CLI.

## Quick Start

```rust
use workq::engine::Engine;
use workq::model::*;

let mut engine = Engine::in_memory()?;

// Submit work
let result = engine.submit(
    NewWorkItem::new("project-check", "heartbeat")
        .dedup_key("project=garden")
        .priority(5)
        .params(serde_json::json!({"project": "garden"}))
)?;

// Claim and execute
if let Some(item) = engine.claim("worker-1")? {
    engine.start(item.id, "worker-1")?;
    engine.log(item.id, LogLevel::Info, "checking project health")?;
    engine.complete(item.id, Outcome {
        success: true,
        data: Some(serde_json::json!({"status": "healthy"})),
        error: None,
        duration_ms: 250,
    })?;
}
```

## Development

```bash
cargo test              # Run all tests
cargo clippy            # Lint
```

Git hooks run tests and clippy on every commit (configured via `.githooks/`).

## Design

See [DESIGN.md](DESIGN.md) for the full design spec. Key principles:

1. **Work has identity** — structural dedup keys prevent the same work from executing twice
2. **Work, not messages** — no from/to routing; the engine schedules work to workers
3. **Dynamic workers** — spin up on demand, not fixed processes polling queues
4. **Observability** — events (engine voice) + logs (worker voice), both queryable
