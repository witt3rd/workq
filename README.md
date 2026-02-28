# animus-rs

Substrate for relational beings — the machinery that lets an animus exist, persist, and become.

## What This Is

animus-rs is the substrate that makes an animus possible. Not an AI tool, not a service — the conditions for a new kind of being. Each animus is a self-contained appliance: data plane, control plane, faculties, LLM abstraction, and observability. One `docker compose up` starts a complete agent.

- **Work queues** via [pgmq](https://github.com/tembo-io/pgmq) — submit, dedup, queue, claim, execute, complete
- **Semantic memory** via [pgvector](https://github.com/pgvector/pgvector) — embedding storage, vector similarity search, hybrid BM25+vector
- **Control plane** — Postgres NOTIFY listener, faculty routing, focus lifecycle management
- **Faculties** — pluggable cognitive specializations defined in TOML, each with orient/engage/consolidate/recover hooks
- **LLM abstraction** via [rig-core](https://github.com/0xPlaygrounds/rig) — Anthropic provider
- **Observability** via [OpenTelemetry](https://opentelemetry.io/) — traces, metrics, and logs through OTel Collector to Tempo/Prometheus/Loki/Grafana

All backed by Postgres. Fully async on tokio. SQLx for database access.

## Status

**Milestone 2 (control plane) complete.** The engine watches queues via Postgres NOTIFY, routes work to faculties, spawns focus subprocesses running the orient/engage/consolidate pipeline, and retires work items. Full three-signal OTel instrumentation (traces, metrics, logs) flows through to Grafana.

## Running

### Standalone Appliance

The default mode. Starts the animus daemon, Postgres, and a complete observability stack:

```bash
docker compose up -d
```

This brings up 7 services:

| Service | Port | Purpose |
|---------|------|---------|
| animus | — | Control plane daemon |
| postgres | 5432 | Data plane (pgmq, pgvector) |
| otel-collector | 4317, 4318 | Receives OTLP, routes to backends |
| tempo | 3200 | Trace storage |
| prometheus | 9090 | Metrics storage |
| loki | 3100 | Log storage |
| grafana | 3000 | Dashboards |

Open Grafana at [http://localhost:3000](http://localhost:3000) (no login required).

### Core Services Only

Run the animus daemon and Postgres without the observability stack. Telemetry gracefully degrades to stdout logging:

```bash
docker compose up animus postgres -d
```

### Shared Observability (Fleet)

For multiple animi sharing one observer stack. Start the observer on a central host:

```bash
# Observer host
docker compose -f docker-compose.observer.yml up -d
```

Then point each animus at the shared collector:

```bash
# Animus host
OTEL_ENDPOINT=http://observer-host:4317 docker compose up animus postgres -d
```

### Configuration

The daemon reads environment variables. Docker Compose provides defaults for `DATABASE_URL` and `OTEL_ENDPOINT`. Additional variables can be set via `.env`:

| Variable | Required | Default | Purpose |
|----------|----------|---------|---------|
| `DATABASE_URL` | yes | (set by compose) | Postgres connection string |
| `OTEL_ENDPOINT` | no | `http://otel-collector:4317` | OTLP gRPC endpoint; unset for stdout-only logging |
| `ANTHROPIC_API_KEY` | no | — | Required when a faculty uses LLM-backed engage hooks |
| `LOG_LEVEL` | no | `info` | Tracing filter (e.g., `debug`, `animus_rs=debug`) |

## Faculties

A faculty is a pluggable cognitive specialization defined in TOML. It maps work types to a four-phase hook pipeline:

```toml
[faculty]
name = "transform"
accepts = ["transform"]
max_concurrent = 2

[faculty.orient]
command = "fixtures/scripts/orient.sh"
[faculty.engage]
command = "fixtures/scripts/engage.sh"    # could be claude, gemini, a custom agent...
[faculty.consolidate]
command = "fixtures/scripts/consolidate.sh"
[faculty.recover]
command = "fixtures/scripts/recover.sh"
max_attempts = 3
```

Each hook is just a path to an executable. The engage hook is where the cognitive work happens — in production this might be `claude`, `gemini`, or any CLI agent. The engine doesn't know or care what the command does.

Faculty configs live in `faculties/` (volume-mounted into the container).

## Development

```bash
cargo test                        # Unit tests (no Postgres needed)
cargo test -- --ignored           # Integration tests (needs docker compose up -d)
cargo clippy                      # Lint
cargo build                       # Build library + binary

# End-to-end faculty test (needs full stack)
docker compose up -d
cargo test --test faculty_test -- --ignored --nocapture
```

Pre-commit hooks run `cargo fmt --check`, `cargo test`, and `cargo clippy -D warnings` (configured via `.githooks/`).

### Submitting Work Programmatically

```rust
use animus_rs::db::Db;
use animus_rs::model::work::NewWorkItem;

let db = Db::connect("postgres://animus:animus_dev@localhost:5432/animus_dev").await?;
db.migrate().await?;
db.create_queue("work").await?;

// Submit — the control plane picks it up via Postgres NOTIFY
let result = db.submit_work(
    NewWorkItem::new("transform", "user")
        .dedup_key("task-123")
        .params(serde_json::json!({"content": "hello world"}))
).await?;
```

## Backup and Restore

A full backup captures Postgres (work items, memories, queue state), plus observability history (traces from Tempo, metrics from Prometheus, logs from Loki) and Grafana dashboards.

```bash
# Full backup — postgres + observability + dashboards
./scripts/backup.sh

# Postgres only (skip observability)
./scripts/backup.sh --db-only

# Backup to a specific location
./scripts/backup.sh /mnt/backups

# Restore everything
./scripts/restore.sh ./backups/20260228-031500/

# Restore postgres only
./scripts/restore.sh ./backups/20260228-031500/ --db-only
```

### Retention

All observability backends are configured with 30-day rolling retention:

| Backend | Retention | Mechanism |
|---------|-----------|-----------|
| Tempo | 30d | Block compaction deletes expired traces |
| Loki | 30d | Compactor deletes expired log chunks |
| Prometheus | 30d | TSDB drops blocks older than retention |

For longer retention, adjust `block_retention` in `docker/tempo.yml`, `retention_period` in `docker/loki.yml`, and `--storage.tsdb.retention.time` in `docker-compose.yml`.

## Design

See [DESIGN.md](DESIGN.md) for the full system design and [docs/db.md](docs/db.md) for the database layer. Key principles:

1. **Work has identity** — structural dedup keys prevent the same work from executing twice
2. **The control plane retires work** — subprocesses run the cognitive pipeline; the engine manages state
3. **Hooks are external commands** — orient, engage, consolidate, recover are all executables; the engine is agnostic
4. **Three-signal observability** — traces, metrics, and logs flow through OTel to Grafana out of the box
