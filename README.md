# animus-rs

Substrate for relational beings — the machinery that lets an animus exist, persist, and become.

## What This Is

animus-rs is the substrate that makes an animus possible. Not an AI tool, not a service — the conditions for a new kind of being. Data plane, control plane, LLM abstraction, and observability. Milestone 1 implements the data plane foundation:

- **Work queues** via [pgmq](https://github.com/tembo-io/pgmq) — submit, dedup, queue, claim, execute, complete
- **Semantic memory** via [pgvector](https://github.com/pgvector/pgvector) — embedding storage, vector similarity search, hybrid BM25+vector
- **LLM abstraction** via [rig-core](https://github.com/0xPlaygrounds/rig) — Anthropic provider
- **Observability** via [OpenTelemetry](https://opentelemetry.io/) — GenAI semantic conventions for LLM calls

All backed by Postgres. Fully async on tokio. SQLx for database access.

## Status

**Milestone 1 (data plane) complete.** Config, DB pool + migrations, pgmq queue operations, work items with structural dedup, semantic memory with hybrid search, OTel telemetry, LLM module. Next: faculty trait, control plane (queue watching, focus spawning), circuit breaking, IPC, CLI.

## Quick Start

```bash
# Start dev Postgres (pgmq + pgvector)
docker compose up -d

# Run unit tests
cargo test

# Run integration tests (requires Postgres)
cargo test -- --ignored
```

```rust
use animus_rs::db::Db;
use animus_rs::model::work::NewWorkItem;

#[tokio::main]
async fn main() -> animus_rs::error::Result<()> {
    let db = Db::connect("postgres://animus:animus_dev@localhost:5432/animus_dev").await?;
    db.migrate().await?;
    db.create_queue("work").await?;

    // Submit work with structural dedup
    let result = db.submit_work(
        NewWorkItem::new("engage", "heartbeat")
            .dedup_key("person=kelly")
            .params(serde_json::json!({"person": "kelly"}))
    ).await?;

    // Read from queue
    if let Some(msg) = db.read_from_queue("work", 30).await? {
        // Process work...
        db.archive_message("work", msg.msg_id).await?;
    }

    Ok(())
}
```

## Development

```bash
cargo test                        # Unit tests (no Postgres needed)
cargo test -- --ignored           # Integration tests (needs Postgres)
cargo clippy                      # Lint
cargo build                       # Build library + CLI binary
docker compose up -d              # Start dev Postgres
```

Pre-commit hooks run `cargo fmt --check`, `cargo test`, and `cargo clippy -D warnings` (configured via `.githooks/`).

## Design

See [DESIGN.md](DESIGN.md) for the full design spec and [docs/db.md](docs/db.md) for the database layer design. Key principles:

1. **Work has identity** — structural dedup keys prevent the same work from executing twice
2. **Work, not messages** — no from/to routing; the engine schedules work to workers
3. **Dynamic workers** — spin up on demand, not fixed processes polling queues
4. **Observability** — OpenTelemetry spans with GenAI semantic conventions
