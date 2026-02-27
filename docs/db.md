# animus-rs: Database Layer Design

## Context

Animus v1 is a Python system with filesystem-based storage: YAML task queues, markdown substrate, ChromaDB for vector search, and JSONL logs. It works, but has real limitations -- no structural dedup, no transactional guarantees, fragile file-based queues, and a separate ChromaDB process to manage.

Animus v2 is a full Rust rewrite. This document defines the **database layer** -- the first milestone. The plan is to rename the `workq` repo to `animus-rs` and replace the SQLite storage layer with Postgres, proving three core capabilities:

1. **Work queues via pgmq** -- replacing the filesystem YAML bus
2. **Semantic memory via pgvector** -- replacing ChromaDB
3. **Observability via OpenTelemetry** -- traces, metrics, and logs using OTel standard conventions (including GenAI semantic conventions for LLM operations)

This is the foundation. No runtime supervisor or DC orchestration yet -- just the data layer with a clean async Rust API.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Architecture | One crate, internal modules | No value in N crates that cannot exist independently; extract later if needed |
| Database | Postgres-only | animus always runs alongside Postgres; no dual-backend abstraction |
| Queue mechanism | pgmq extension | Replaces hand-rolled state machine; provides send/read/archive, visibility timeouts, delayed messages |
| Semantic search | pgvector extension | Replaces ChromaDB; same Postgres instance, hybrid BM25+vector search |
| Async | Fully async on tokio | Postgres clients are async-first; Engine API becomes async fn |
| Rust Postgres client | SQLx | Compile-time SQL checking, built-in migrations, async, offline mode for CI |
| LLM abstraction | rig-core | Unified provider interface (CompletionModel, EmbeddingModel), `#[derive(Embed)]`, agent tooling |
| Semantic memory | rig-postgres | pgvector VectorStoreIndex from the Rig ecosystem; handles embedding storage and search |
| Low-level DB access | Direct SQLx | pgmq operations, work_items dedup/provenance, custom queries, migrations |
| Observability | OpenTelemetry | OTel traces/metrics/logs with GenAI semantic conventions; replaces custom events table |
| Secrets | secrecy + dotenvy + env vars | Secrets loaded from env at runtime, wrapped in secrecy types, never logged. `.env` for local dev. Prod uses systemd EnvironmentFile |

## Module Structure

```
src/
  lib.rs                — crate root, re-exports public API
  db/
    mod.rs              — connection pool (shared by SQLx + rig-postgres), migrations, health check
    migrations/         — SQLx migration files (.sql)
    pgmq.rs             — pgmq queue operations via direct SQLx (send, read, archive, delete)
    work.rs             — work_items table: dedup, provenance, state tracking via direct SQLx
  memory/
    mod.rs              — semantic memory module root
    store.rs            — embedding storage, search, hybrid search (wraps rig-postgres VectorStoreIndex)
  llm/
    mod.rs              — LLM provider setup via rig-core (CompletionModel, EmbeddingModel)
  model/
    mod.rs              — re-exports
    work.rs             — WorkItem, State, Provenance, Outcome (carried from workq)
    memory.rs           — MemoryEntry types (compatible with Rig's Embed derive)
  config/
    mod.rs              — typed config loading: env vars to strongly-typed structs, fail-fast at startup
    secrets.rs          — secrecy-wrapped sensitive values (DB URL, API keys); dotenvy for local dev
  telemetry/
    mod.rs              — OTel initialization (tracer, meter, logger providers)
    genai.rs            — GenAI semantic convention spans and metrics for LLM calls
    work.rs             — work execution spans and metrics
  error.rs              — error types (sqlx::Error replaces rusqlite::Error)
  bin/
    animus.rs           — CLI binary (placeholder)
```

## Two-Layer Data Access

The crate uses two distinct data access strategies that share the same Postgres connection pool:

**Layer 1: Direct SQLx** (`db/` module) -- for infrastructure concerns where we need full SQL control. This covers pgmq queue operations (`SELECT pgmq.send(...)`, `SELECT * FROM pgmq.read(...)`), work_items dedup and provenance tracking, custom queries, and migrations. pgmq's API is SQL-native, so calling it via SQLx is natural and avoids depending on a potentially immature Rust client crate.

**Layer 2: rig-postgres** (`memory/` module) -- for AI concerns where Rig's abstractions add real value. This covers pgvector VectorStoreIndex for embedding storage and semantic search, leveraging Rig's `#[derive(Embed)]` and `EmbeddingsBuilder`. For queries that need more control (hybrid search with metadata filters), we drop down to direct SQLx.

Both layers operate against the same Postgres instance and can participate in the same transactions when needed.

## Schema

Three SQLx migrations define the schema.

### Migration 001: Extensions

```sql
CREATE EXTENSION IF NOT EXISTS pgmq;
CREATE EXTENSION IF NOT EXISTS vector;
```

### Migration 002: Work Items

pgmq creates its own queue tables via `pgmq.create('work')`. We add a companion metadata table for dedup, provenance, and lifecycle tracking:

```sql
CREATE TABLE work_items (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    pgmq_msg_id     BIGINT,
    queue_name      TEXT NOT NULL,
    work_type       TEXT NOT NULL,
    dedup_key       TEXT,
    source          TEXT NOT NULL,
    trigger_info    TEXT,
    params          JSONB NOT NULL DEFAULT '{}',
    priority        INTEGER NOT NULL DEFAULT 0,
    state           TEXT NOT NULL DEFAULT 'queued',
    merged_into     UUID REFERENCES work_items(id),
    parent_id       UUID REFERENCES work_items(id),
    attempts        INTEGER NOT NULL DEFAULT 0,
    max_attempts    INTEGER,
    outcome_data    JSONB,
    outcome_error   TEXT,
    outcome_ms      BIGINT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at     TIMESTAMPTZ
);

CREATE INDEX idx_work_dedup ON work_items(work_type, dedup_key)
    WHERE dedup_key IS NOT NULL AND state NOT IN ('completed', 'dead', 'merged');
CREATE INDEX idx_work_state ON work_items(state);
CREATE INDEX idx_work_parent ON work_items(parent_id) WHERE parent_id IS NOT NULL;
```

The `pgmq_msg_id` column links each work item to its pgmq message. The partial index on `(work_type, dedup_key)` covers only active items, keeping dedup lookups fast without indexing terminal states.

### Migration 003: Memories

```sql
CREATE TABLE memories (
    id              BIGSERIAL PRIMARY KEY,
    content         TEXT NOT NULL,
    memory_type     TEXT NOT NULL,
    source          TEXT,
    metadata        JSONB NOT NULL DEFAULT '{}',
    embedding       vector(1536),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_memories_embedding ON memories
    USING hnsw (embedding vector_cosine_ops);
CREATE INDEX idx_memories_type ON memories(memory_type);

ALTER TABLE memories ADD COLUMN search_text tsvector
    GENERATED ALWAYS AS (
        to_tsvector('english', coalesce(content, ''))
    ) STORED;
CREATE INDEX idx_memories_fts ON memories USING gin(search_text);
```

The HNSW index on the embedding column enables fast approximate nearest-neighbor search. The generated `tsvector` column and GIN index support BM25-style full-text search. Together they enable hybrid queries that combine semantic similarity with keyword matching.

The `memory_type` column distinguishes episodic, semantic, relational, and other memory categories. The embedding dimension (1536) matches common embedding models but may need to be configurable.

### No Events or Logs Tables

Observability moves entirely to OpenTelemetry. Instead of custom Postgres tables for events and work-scoped logs, we emit OTel spans, metrics, and logs. These flow to standard backends (Jaeger, Grafana, OTLP collector). Postgres stores domain state; OTel handles observability. This is a cleaner separation of concerns.

## API Surface

### Config

```rust
pub struct Config {
    pub database_url: Secret<String>,
    pub anthropic_api_key: Secret<String>,
    pub otel_endpoint: Option<String>,
    pub log_level: String,
}

impl Config {
    /// Load from environment. Panics with clear message if required vars missing.
    pub fn from_env() -> Result<Self>
}
```

Local dev uses `dotenvy::dotenv().ok()` to load a `.env` file (gitignored with real secrets; `.env.example` committed with placeholders). Prod uses systemd `EnvironmentFile`. CI uses GitHub secrets and service containers.

### Db

```rust
// Connection
pub async fn connect(url: &str) -> Result<Db>
pub async fn migrate(&self) -> Result<()>

// Work queues (pgmq + dedup/provenance)
pub async fn submit_work(&self, new: NewWorkItem) -> Result<SubmitResult>
pub async fn read_work(&self, queue: &str, vt_seconds: i32) -> Result<Option<WorkItem>>
pub async fn archive_work(&self, queue: &str, msg_id: i64) -> Result<()>
pub async fn delete_work(&self, queue: &str, msg_id: i64) -> Result<()>

// Semantic memory (via rig-postgres VectorStoreIndex)
pub async fn store_memories(&self, docs: Vec<impl Embed>, model: &impl EmbeddingModel) -> Result<()>
pub async fn search_memory(&self, query: &str, limit: usize, model: &impl EmbeddingModel) -> Result<Vec<MemoryEntry>>

// Hybrid search (direct SQLx for full control)
pub async fn hybrid_search(&self, text: &str, embedding: &[f32], limit: i32, filters: &MemoryFilters) -> Result<Vec<MemoryEntry>>
```

`submit_work` handles the full submit flow: insert the work_items row, check for structural dedup, merge or send to pgmq, and return whether the item was queued or merged. `read_work` reads from pgmq with a visibility timeout and joins the work_items metadata.

### Telemetry

```rust
// Initialization
pub fn init_telemetry(config: TelemetryConfig) -> Result<TelemetryGuard>

// GenAI semantic convention helpers
pub fn start_chat_span(model: &str, provider: &str) -> Span
pub fn start_embedding_span(model: &str, provider: &str) -> Span
pub fn record_token_usage(span: &Span, input: u64, output: u64, cache_read: u64, cache_create: u64)
pub fn record_operation_duration(operation: &str, provider: &str, duration: Duration)

// Work execution spans
pub fn start_work_span(work_type: &str, work_id: &Uuid) -> Span
pub fn record_work_lifecycle(work_id: &Uuid, from_state: &str, to_state: &str)
```

OTel GenAI semantic conventions used:
- **Spans:** `gen_ai.operation.name`, `gen_ai.request.model`, `gen_ai.response.model`, `gen_ai.provider.name`
- **Token attributes:** `gen_ai.usage.input_tokens`, `gen_ai.usage.output_tokens`, `gen_ai.usage.cache_creation.input_tokens`, `gen_ai.usage.cache_read.input_tokens`
- **Metrics:** `gen_ai.client.token.usage` (histogram), `gen_ai.client.operation.duration` (histogram)

The telemetry module uses the `tracing` + `tracing-opentelemetry` bridge so Rust code uses idiomatic `tracing::instrument` / `tracing::info!()` macros, and the OTel layer exports them as spans and logs. GenAI-specific attributes are set explicitly on spans.

## Deployment Model

animus-rs runs on Arch Linux as a systemd user service alongside Postgres:

```
Postgres (system service, pgmq + pgvector extensions)
  +-- animus-rs (user service, connects via DATABASE_URL)
        |-- reads config from env vars (EnvironmentFile)
        |-- exports OTel to local collector or Jaeger
        +-- Postgres backups via systemd timer (pg_dump + retention)
```

- **Postgres** runs as a system-level `postgresql.service` with pgmq and pgvector extensions installed via pacman/AUR
- **animus-rs** runs as a user-level systemd service (`~/.config/systemd/user/animus.service`)
- **Secrets** are stored in `~/.config/animus/secrets.env` (chmod 600) and loaded via systemd `EnvironmentFile` -- no extra secret management infra needed
- **Backups** run via a systemd timer executing `pg_dump` daily with retention, independent of the animus service
- **Migrations** run at startup (`Db::migrate()` on connect) or manually via `cargo sqlx migrate run`

## Dependencies

```toml
[package]
name = "animus-rs"
version = "0.1.0"
edition = "2024"

[dependencies]
# Config + Secrets
dotenvy = "0.15"
secrecy = { version = "0.10", features = ["serde"] }

# Core
chrono = { version = "0.4", features = ["serde"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "uuid", "chrono", "json"] }
thiserror = "2"
tokio = { version = "1", features = ["full"] }
uuid = { version = "1", features = ["v4", "serde"] }

# LLM + Embeddings
rig-core = "0.11"
rig-postgres = "0.3"

# OpenTelemetry
opentelemetry = "0.28"
opentelemetry_sdk = { version = "0.28", features = ["rt-tokio"] }
opentelemetry-otlp = { version = "0.28", features = ["tonic"] }
opentelemetry-semantic-conventions = "0.28"
tracing = "0.1"
tracing-opentelemetry = "0.29"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
```

## Open Items

These are not blocking for milestone 1 but need resolution during implementation:

- Exact pgmq Rust integration (mature client crate vs raw SQL via SQLx)
- Embedding dimension configurability (1536 is the default but models vary)
- Docker compose setup for dev/test Postgres with pgmq + pgvector
- CI pipeline update (Postgres service container with extensions)
- Rig version pinning (verify rig-core and rig-postgres latest versions)
- Repo rename mechanics (GitHub rename, update all references)
- OTel collector/backend for dev (Jaeger container, Grafana stack)
- GenAI semantic conventions are "Development" status and may evolve
