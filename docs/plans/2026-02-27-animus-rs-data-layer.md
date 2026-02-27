# animus-rs Data Layer Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Transform the workq SQLite crate into animus-rs, a Postgres-backed data layer with pgmq work queues, pgvector semantic memory, OpenTelemetry observability, and rig-core LLM abstraction.

**Architecture:** One crate with internal modules. Two-layer data access: direct SQLx for work queues/dedup/provenance, rig-postgres for semantic memory. All async on tokio. OTel replaces custom events/logs tables. Config loaded from env vars with secrecy wrapping.

**Tech Stack:** Rust 1.85+ (edition 2024), SQLx 0.8, tokio, rig-core, rig-postgres, pgmq (SQL via SQLx), pgvector, OpenTelemetry 0.28, tracing, secrecy, dotenvy. Postgres 16+ with pgmq + pgvector extensions. Docker Compose for dev/test.

---

## Phase 1: Foundation

### Task 1: Write design doc to docs/db.md

**Files:**
- Modify: `docs/db.md`

**Step 1: Write the design doc**

Capture the full architectural design (from our brainstorming session) in `docs/db.md`. This is the reference document for the entire data layer. Include: context, decisions table, module structure, schema, API surface, deployment model, open items.

**Step 2: Commit**

```bash
git add docs/db.md
git commit -m "docs: animus-rs database layer design"
```

---

### Task 2: Rename crate from workq to animus-rs

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`
- Modify: `src/bin/workq.rs` → rename to `src/bin/animus.rs`
- Modify: `CLAUDE.md`
- Modify: `DESIGN.md`

**Step 1: Update Cargo.toml**

Change package name, description, and binary target:

```toml
[package]
name = "animus-rs"
version = "0.1.0"
edition = "2024"
description = "Animus v2 engine — Postgres-backed data layer for AI persistence"
license = "MIT"

[[bin]]
name = "animus"
path = "src/bin/animus.rs"
```

Keep existing dependencies for now (they'll be replaced in Task 3).

**Step 2: Rename binary source**

```bash
mv src/bin/workq.rs src/bin/animus.rs
```

Update any references in the binary file.

**Step 3: Update lib.rs doc comment**

Replace the workq doc comment with animus-rs description.

**Step 4: Update CLAUDE.md and DESIGN.md**

Replace workq references with animus-rs. Update command examples.

**Step 5: Verify build**

```bash
cargo build
```

Expected: compiles (still using old deps, that's fine).

**Step 6: Commit**

```bash
git add -A
git commit -m "rename: workq → animus-rs"
```

---

### Task 3: Replace dependencies

**Files:**
- Modify: `Cargo.toml`

**Step 1: Replace Cargo.toml dependencies**

Remove `rusqlite`. Add all new dependencies:

```toml
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

# LLM + Embeddings (Rig)
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

**Step 2: Verify dependency resolution**

```bash
cargo check 2>&1 | head -50
```

Expected: dependency resolution succeeds (compilation will fail because old code references rusqlite — that's expected and fine).

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "deps: replace rusqlite with sqlx, rig, otel, secrecy"
```

Note: If rig-core or rig-postgres versions don't resolve, check crates.io for latest versions and adjust. The exact versions in the plan are estimates — use whatever is current.

---

### Task 4: Create module structure with stubs

**Files:**
- Rewrite: `src/lib.rs`
- Create: `src/db/mod.rs`
- Create: `src/db/pgmq.rs`
- Create: `src/db/work.rs`
- Create: `src/memory/mod.rs`
- Create: `src/memory/store.rs`
- Create: `src/llm/mod.rs`
- Create: `src/model/mod.rs`
- Create: `src/model/work.rs`
- Create: `src/model/memory.rs`
- Create: `src/config/mod.rs`
- Create: `src/config/secrets.rs`
- Create: `src/telemetry/mod.rs`
- Create: `src/telemetry/genai.rs`
- Create: `src/telemetry/work.rs`
- Rewrite: `src/error.rs`
- Remove: `src/storage.rs`
- Remove: `src/engine.rs`
- Remove: `src/event.rs`
- Remove: `tests/engine_test.rs` (will be replaced with new tests)

**Step 1: Remove old code**

```bash
rm src/storage.rs src/engine.rs src/event.rs tests/engine_test.rs
```

**Step 2: Create new directory structure**

```bash
mkdir -p src/db src/memory src/llm src/model src/config src/telemetry
```

**Step 3: Write stub modules**

Each module gets a minimal stub with a doc comment explaining its purpose. Example for `src/db/mod.rs`:

```rust
//! Database connection pool, migrations, and health check.
//!
//! Shared Postgres connection pool used by both direct SQLx queries
//! and rig-postgres VectorStoreIndex.

pub mod pgmq;
pub mod work;
```

Write similar stubs for all modules. `src/lib.rs` re-exports the public API:

```rust
//! # animus-rs
//!
//! Postgres-backed data layer for the Animus v2 AI persistence engine.
//!
//! Provides work queues (pgmq), semantic memory (pgvector via rig-postgres),
//! LLM abstraction (rig-core), and OpenTelemetry observability.

pub mod config;
pub mod db;
pub mod error;
pub mod llm;
pub mod memory;
pub mod model;
pub mod telemetry;
```

**Step 4: Write new error.rs**

```rust
//! Error types for animus-rs.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid state transition: {from} -> {to}")]
    InvalidTransition { from: String, to: String },

    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("configuration error: {0}")]
    Config(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, Error>;
```

**Step 5: Write model/work.rs**

Adapt from existing `src/model.rs`. Keep: WorkItem, State, Provenance, Outcome, NewWorkItem, WorkId. Remove: LogEntry, LogLevel. Keep `State::can_transition_to()`. Adapt to use `sqlx::FromRow` where appropriate.

**Step 6: Write model/memory.rs**

```rust
//! Memory entry types, compatible with rig's Embed derive.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: i64,
    pub content: String,
    pub memory_type: String,
    pub source: Option<String>,
    pub metadata: serde_json::Value,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct NewMemory {
    pub content: String,
    pub memory_type: String,
    pub source: Option<String>,
    pub metadata: serde_json::Value,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryFilters {
    pub memory_type: Option<String>,
    pub source: Option<String>,
    pub since: Option<DateTime<Utc>>,
}
```

**Step 7: Verify it compiles**

```bash
cargo check
```

Expected: compiles with warnings about unused imports/dead code (stubs). No errors.

**Step 8: Commit**

```bash
git add -A
git commit -m "scaffold: new module structure for animus-rs"
```

---

### Task 5: Docker Compose for dev/test Postgres

**Files:**
- Create: `docker-compose.yml`
- Create: `.env.example`
- Modify: `.gitignore`

**Step 1: Write docker-compose.yml**

```yaml
services:
  postgres:
    image: pgvector/pgvector:pg17
    environment:
      POSTGRES_USER: animus
      POSTGRES_PASSWORD: animus_dev
      POSTGRES_DB: animus_dev
    ports:
      - "5432:5432"
    volumes:
      - pgdata:/var/lib/postgresql/data
      - ./docker/init-extensions.sql:/docker-entrypoint-initdb.d/01-extensions.sql
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U animus"]
      interval: 5s
      timeout: 5s
      retries: 5

volumes:
  pgdata:
```

**Step 2: Write docker/init-extensions.sql**

```sql
-- Install pgmq extension (must be available in the image)
CREATE EXTENSION IF NOT EXISTS pgmq;
CREATE EXTENSION IF NOT EXISTS vector;
```

Note: the `pgvector/pgvector` image includes pgvector. For pgmq, we may need a custom Dockerfile that adds it. Research needed — if pgmq isn't in the base image, create a `docker/Dockerfile`:

```dockerfile
FROM pgvector/pgvector:pg17
RUN apt-get update && apt-get install -y postgresql-17-pgmq && rm -rf /var/lib/apt/lists/*
```

And reference it in docker-compose: `build: ./docker`

**Step 3: Write .env.example**

```
DATABASE_URL=postgres://animus:animus_dev@localhost:5432/animus_dev
ANTHROPIC_API_KEY=sk-ant-your-key-here
OTEL_ENDPOINT=http://localhost:4317
LOG_LEVEL=info
```

**Step 4: Update .gitignore**

Add:
```
.env
!.env.example
```

**Step 5: Test that Postgres starts**

```bash
docker compose up -d
docker compose exec postgres psql -U animus -d animus_dev -c "SELECT 1;"
docker compose exec postgres psql -U animus -d animus_dev -c "CREATE EXTENSION IF NOT EXISTS vector; SELECT extversion FROM pg_extension WHERE extname = 'vector';"
docker compose exec postgres psql -U animus -d animus_dev -c "CREATE EXTENSION IF NOT EXISTS pgmq; SELECT extversion FROM pg_extension WHERE extname = 'pgmq';"
```

Expected: all three queries succeed. If pgmq fails, build custom Docker image per note above.

**Step 6: Commit**

```bash
git add docker-compose.yml docker/ .env.example .gitignore
git commit -m "infra: docker compose for dev/test Postgres with pgmq + pgvector"
```

---

### Task 6: Config module

**Files:**
- Modify: `src/config/mod.rs`
- Modify: `src/config/secrets.rs`

**Step 1: Write the test**

Create `tests/config_test.rs`:

```rust
use animus_rs::config::Config;

#[test]
fn config_from_env_loads_required_fields() {
    // Set required env vars for test
    unsafe {
        std::env::set_var("DATABASE_URL", "postgres://test:test@localhost/test");
        std::env::set_var("ANTHROPIC_API_KEY", "sk-test-key");
    }

    let config = Config::from_env().unwrap();
    // secrecy: can't read the value directly, but we can verify it loaded
    assert!(!config.log_level.is_empty());

    // Clean up
    unsafe {
        std::env::remove_var("DATABASE_URL");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }
}

#[test]
fn config_from_env_fails_without_required() {
    unsafe {
        std::env::remove_var("DATABASE_URL");
        std::env::remove_var("ANTHROPIC_API_KEY");
    }

    let result = Config::from_env();
    assert!(result.is_err());
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test --test config_test
```

Expected: FAIL (Config not implemented yet).

**Step 3: Implement config/mod.rs**

```rust
//! Typed configuration from environment variables.
//!
//! Loads once at startup, fails fast if required vars are missing.
//! Sensitive values wrapped in secrecy::Secret to prevent log leaks.

pub mod secrets;

use crate::error::{Error, Result};
use secrecy::Secret;

#[derive(Debug)]
pub struct Config {
    pub database_url: Secret<String>,
    pub anthropic_api_key: Secret<String>,
    pub otel_endpoint: Option<String>,
    pub log_level: String,
}

impl Config {
    /// Load configuration from environment variables.
    ///
    /// In local dev, call `dotenvy::dotenv().ok()` before this.
    /// In production, systemd EnvironmentFile provides the vars.
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: Secret::new(
                required_var("DATABASE_URL")?,
            ),
            anthropic_api_key: Secret::new(
                required_var("ANTHROPIC_API_KEY")?,
            ),
            otel_endpoint: std::env::var("OTEL_ENDPOINT").ok(),
            log_level: std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string()),
        })
    }
}

fn required_var(name: &str) -> Result<String> {
    std::env::var(name).map_err(|_| {
        Error::Config(format!("required environment variable {name} is not set"))
    })
}
```

**Step 4: Implement config/secrets.rs**

```rust
//! Secret handling utilities.
//!
//! Re-exports secrecy types and provides helpers for working with
//! secrets in the animus-rs context.

pub use secrecy::{ExposeSecret, Secret};
```

**Step 5: Run tests**

```bash
cargo test --test config_test
```

Expected: PASS.

**Step 6: Commit**

```bash
git add src/config/ tests/config_test.rs
git commit -m "feat: config module with typed env loading and secrecy"
```

---

## Phase 2: Data Layer (pgmq + work items)

### Task 7: Database connection pool and migrations

**Files:**
- Modify: `src/db/mod.rs`
- Create: `migrations/001_extensions.sql`
- Create: `migrations/002_work_items.sql`
- Create: `migrations/003_memories.sql`

**Step 1: Write the test**

Create `tests/db_test.rs`:

```rust
use animus_rs::db::Db;

#[tokio::test]
async fn connects_and_migrates() {
    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://animus:animus_dev@localhost:5432/animus_dev".to_string());

    let db = Db::connect(&db_url).await.unwrap();
    db.migrate().await.unwrap();
    assert!(db.health_check().await.is_ok());
}
```

**Step 2: Run test to verify it fails**

```bash
docker compose up -d
cargo test --test db_test -- --nocapture
```

Expected: FAIL (Db struct not implemented).

**Step 3: Write src/db/mod.rs**

```rust
//! Database connection pool, migrations, and health check.

pub mod pgmq;
pub mod work;

use crate::error::Result;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

/// Database handle. Owns the connection pool shared across all modules.
pub struct Db {
    pool: PgPool,
}

impl Db {
    /// Connect to Postgres and create a connection pool.
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(10)
            .connect(url)
            .await?;
        Ok(Self { pool })
    }

    /// Run all pending migrations.
    pub async fn migrate(&self) -> Result<()> {
        sqlx::migrate!("./migrations")
            .run(&self.pool)
            .await
            .map_err(|e| crate::error::Error::Other(format!("migration failed: {e}")))?;
        Ok(())
    }

    /// Simple health check — run a SELECT 1.
    pub async fn health_check(&self) -> Result<()> {
        sqlx::query("SELECT 1")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Get a reference to the connection pool (for submodules).
    pub(crate) fn pool(&self) -> &PgPool {
        &self.pool
    }
}
```

**Step 4: Write migration files**

`migrations/001_extensions.sql`:
```sql
CREATE EXTENSION IF NOT EXISTS pgmq;
CREATE EXTENSION IF NOT EXISTS vector;
```

`migrations/002_work_items.sql`:
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

`migrations/003_memories.sql`:
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

**Step 5: Run test**

```bash
cargo test --test db_test -- --nocapture
```

Expected: PASS (connects, migrates, health check succeeds).

**Step 6: Commit**

```bash
git add src/db/mod.rs migrations/ tests/db_test.rs
git commit -m "feat: db connection pool, migrations (extensions, work_items, memories)"
```

---

### Task 8: pgmq queue operations

**Files:**
- Modify: `src/db/pgmq.rs`

**Step 1: Write the test**

Add to `tests/db_test.rs`:

```rust
#[tokio::test]
async fn pgmq_send_and_read() {
    let db = test_db().await;

    // Create a queue
    db.create_queue("test_work").await.unwrap();

    // Send a message
    let msg_id = db.send_to_queue("test_work", &serde_json::json!({"task": "hello"}), 0).await.unwrap();
    assert!(msg_id > 0);

    // Read it back (30s visibility timeout)
    let msg = db.read_from_queue("test_work", 30).await.unwrap();
    assert!(msg.is_some());
    let msg = msg.unwrap();
    assert_eq!(msg.msg_id, msg_id);

    // Archive it
    db.archive_message("test_work", msg_id).await.unwrap();

    // Queue should be empty now
    let msg = db.read_from_queue("test_work", 30).await.unwrap();
    assert!(msg.is_none());
}

/// Helper: connect + migrate for tests
async fn test_db() -> animus_rs::db::Db {
    let url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://animus:animus_dev@localhost:5432/animus_dev".to_string());
    let db = animus_rs::db::Db::connect(&url).await.unwrap();
    db.migrate().await.unwrap();
    db
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test --test db_test pgmq_send_and_read -- --nocapture
```

Expected: FAIL (methods not implemented).

**Step 3: Implement src/db/pgmq.rs**

pgmq operations via direct SQL (calling pgmq's SQL functions through SQLx):

```rust
//! pgmq queue operations via direct SQLx.
//!
//! Calls pgmq's SQL functions: pgmq.create, pgmq.send, pgmq.read,
//! pgmq.archive, pgmq.delete.

use crate::error::Result;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PgmqMessage {
    pub msg_id: i64,
    pub read_ct: i32,
    pub enqueued_at: chrono::DateTime<chrono::Utc>,
    pub vt: chrono::DateTime<chrono::Utc>,
    pub message: serde_json::Value,
}

impl super::Db {
    /// Create a pgmq queue (idempotent).
    pub async fn create_queue(&self, queue_name: &str) -> Result<()> {
        sqlx::query("SELECT pgmq.create($1)")
            .bind(queue_name)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Send a message to a pgmq queue. Returns the message ID.
    /// delay_seconds: 0 for immediate, >0 for delayed delivery.
    pub async fn send_to_queue(
        &self,
        queue_name: &str,
        payload: &serde_json::Value,
        delay_seconds: i32,
    ) -> Result<i64> {
        let row: (i64,) = sqlx::query_as(
            "SELECT pgmq.send($1, $2, $3)"
        )
            .bind(queue_name)
            .bind(payload)
            .bind(delay_seconds)
            .fetch_one(&self.pool)
            .await?;
        Ok(row.0)
    }

    /// Read the next message from a queue (visibility timeout in seconds).
    /// Returns None if queue is empty.
    pub async fn read_from_queue(
        &self,
        queue_name: &str,
        vt_seconds: i32,
    ) -> Result<Option<PgmqMessage>> {
        let row = sqlx::query_as::<_, (i64, i32, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>, serde_json::Value)>(
            "SELECT msg_id, read_ct, enqueued_at, vt, message FROM pgmq.read($1, $2, 1)"
        )
            .bind(queue_name)
            .bind(vt_seconds)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|(msg_id, read_ct, enqueued_at, vt, message)| PgmqMessage {
            msg_id,
            read_ct,
            enqueued_at,
            vt,
            message,
        }))
    }

    /// Archive a message (moves to archive table, preserves for audit).
    pub async fn archive_message(&self, queue_name: &str, msg_id: i64) -> Result<()> {
        sqlx::query("SELECT pgmq.archive($1, $2)")
            .bind(queue_name)
            .bind(msg_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Delete a message permanently.
    pub async fn delete_message(&self, queue_name: &str, msg_id: i64) -> Result<()> {
        sqlx::query("SELECT pgmq.delete($1, $2)")
            .bind(queue_name)
            .bind(msg_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
```

**Step 4: Run test**

```bash
cargo test --test db_test pgmq_send_and_read -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/db/pgmq.rs tests/db_test.rs
git commit -m "feat: pgmq queue operations (create, send, read, archive, delete)"
```

---

### Task 9: Work item operations (submit with dedup, state tracking)

**Files:**
- Modify: `src/db/work.rs`

**Step 1: Write the tests**

Add to `tests/db_test.rs`:

```rust
use animus_rs::model::work::{NewWorkItem, Provenance};

#[tokio::test]
async fn submit_work_creates_and_queues() {
    let db = test_db().await;
    db.create_queue("work").await.unwrap();

    let new = NewWorkItem::new("engage", "heartbeat")
        .dedup_key("person=kelly")
        .params(serde_json::json!({"person": "kelly"}));

    let result = db.submit_work(new).await.unwrap();
    assert!(matches!(result, animus_rs::db::work::SubmitResult::Created(_)));
}

#[tokio::test]
async fn submit_duplicate_work_merges() {
    let db = test_db().await;
    db.create_queue("work").await.unwrap();

    let new1 = NewWorkItem::new("engage", "heartbeat")
        .dedup_key("person=kelly");
    let result1 = db.submit_work(new1).await.unwrap();
    assert!(matches!(result1, animus_rs::db::work::SubmitResult::Created(_)));

    let new2 = NewWorkItem::new("engage", "user")
        .dedup_key("person=kelly");
    let result2 = db.submit_work(new2).await.unwrap();
    assert!(matches!(result2, animus_rs::db::work::SubmitResult::Merged { .. }));
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test --test db_test submit_work -- --nocapture
```

Expected: FAIL.

**Step 3: Implement src/db/work.rs**

Work item operations: submit (with dedup check), get, update state. Uses SQLx transactions for atomicity.

```rust
//! Work item operations: submit with dedup, state tracking, provenance.

use crate::error::{Error, Result};
use crate::model::work::*;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug)]
pub enum SubmitResult {
    Created(WorkItem),
    Merged { new_id: Uuid, canonical_id: Uuid },
}

impl super::Db {
    /// Submit new work. Checks structural dedup, sends to pgmq queue.
    pub async fn submit_work(&self, new: NewWorkItem) -> Result<SubmitResult> {
        let mut tx = self.pool.begin().await?;
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        // Insert work_items row
        sqlx::query(
            "INSERT INTO work_items (id, queue_name, work_type, dedup_key, source, trigger_info, params, priority, state, parent_id, max_attempts, created_at, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12)"
        )
            .bind(id)
            .bind("work") // default queue
            .bind(&new.work_type)
            .bind(&new.dedup_key)
            .bind(&new.provenance.source)
            .bind(&new.provenance.trigger)
            .bind(&new.params)
            .bind(new.priority)
            .bind("created")
            .bind(new.parent_id)
            .bind(new.max_attempts.map(|n| n as i32))
            .bind(now)
            .execute(&mut *tx)
            .await?;

        // Structural dedup check
        if let Some(ref dedup_key) = new.dedup_key {
            let existing: Option<(Uuid,)> = sqlx::query_as(
                "SELECT id FROM work_items
                 WHERE work_type = $1 AND dedup_key = $2
                 AND state NOT IN ('completed', 'dead', 'merged')
                 AND id != $3
                 ORDER BY created_at ASC
                 LIMIT 1"
            )
                .bind(&new.work_type)
                .bind(dedup_key)
                .bind(id)
                .fetch_optional(&mut *tx)
                .await?;

            if let Some((canonical_id,)) = existing {
                // Merge: mark new item as merged
                sqlx::query(
                    "UPDATE work_items SET state = 'merged', merged_into = $1, resolved_at = now(), updated_at = now() WHERE id = $2"
                )
                    .bind(canonical_id)
                    .bind(id)
                    .execute(&mut *tx)
                    .await?;

                tx.commit().await?;
                return Ok(SubmitResult::Merged { new_id: id, canonical_id });
            }
        }

        // No dedup match — queue it via pgmq
        let msg_id: (i64,) = sqlx::query_as("SELECT pgmq.send($1, $2, $3)")
            .bind("work")
            .bind(&new.params)
            .bind(0i32)
            .fetch_one(&mut *tx)
            .await?;

        // Update work item with pgmq msg ID and state
        sqlx::query(
            "UPDATE work_items SET state = 'queued', pgmq_msg_id = $1, updated_at = now() WHERE id = $2"
        )
            .bind(msg_id.0)
            .bind(id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        let item = self.get_work_item(id).await?;
        Ok(SubmitResult::Created(item))
    }

    /// Get a work item by ID.
    pub async fn get_work_item(&self, id: Uuid) -> Result<WorkItem> {
        let row = sqlx::query_as::<_, WorkItemRow>(
            "SELECT * FROM work_items WHERE id = $1"
        )
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;

        Ok(row.into())
    }
}
```

Note: `WorkItemRow` is a helper struct with `sqlx::FromRow` derive that maps DB columns to Rust types, then converts to the public `WorkItem` type. Implement this in `model/work.rs`.

**Step 4: Run tests**

```bash
cargo test --test db_test submit_work -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/db/work.rs src/model/work.rs tests/db_test.rs
git commit -m "feat: work item submit with structural dedup and pgmq integration"
```

---

## Phase 3: Semantic Memory

### Task 10: Memory storage and vector search

**Files:**
- Modify: `src/memory/mod.rs`
- Modify: `src/memory/store.rs`

**Step 1: Write the tests**

Create `tests/memory_test.rs`:

```rust
#[tokio::test]
async fn store_and_search_memory() {
    let db = test_db().await;

    // Store a memory with a fake embedding (1536 dims)
    let embedding = vec![0.1_f32; 1536];
    let new = NewMemory {
        content: "Kelly prefers morning meetings".to_string(),
        memory_type: "relational".to_string(),
        source: Some("engage".to_string()),
        metadata: serde_json::json!({"person": "kelly"}),
        embedding: embedding.clone(),
    };

    let id = db.store_memory(new).await.unwrap();
    assert!(id > 0);

    // Search with a similar embedding
    let results = db.search_memory_by_vector(&embedding, 10, &Default::default()).await.unwrap();
    assert!(!results.is_empty());
    assert_eq!(results[0].content, "Kelly prefers morning meetings");
}

#[tokio::test]
async fn hybrid_search_text_and_vector() {
    let db = test_db().await;

    let embedding = vec![0.1_f32; 1536];
    db.store_memory(NewMemory {
        content: "Kelly prefers morning meetings and coffee".to_string(),
        memory_type: "relational".to_string(),
        source: None,
        metadata: serde_json::json!({}),
        embedding: embedding.clone(),
    }).await.unwrap();

    let results = db.hybrid_search("morning coffee", &embedding, 10, &Default::default()).await.unwrap();
    assert!(!results.is_empty());
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test --test memory_test -- --nocapture
```

**Step 3: Implement memory storage and search**

Direct SQLx for now (rig-postgres integration can be layered on later as the rig API stabilizes). The key operations:

- `store_memory`: INSERT with embedding vector
- `search_memory_by_vector`: vector similarity search via `<->` operator
- `hybrid_search`: combined BM25 + vector scoring

**Step 4: Run tests**

```bash
cargo test --test memory_test -- --nocapture
```

Expected: PASS.

**Step 5: Commit**

```bash
git add src/memory/ tests/memory_test.rs
git commit -m "feat: semantic memory with pgvector search and hybrid BM25+vector"
```

---

## Phase 4: Telemetry

### Task 11: OpenTelemetry initialization and GenAI spans

**Files:**
- Modify: `src/telemetry/mod.rs`
- Modify: `src/telemetry/genai.rs`
- Modify: `src/telemetry/work.rs`

**Step 1: Write the test**

Create `tests/telemetry_test.rs`:

```rust
#[test]
fn telemetry_initializes_without_endpoint() {
    // Without an OTLP endpoint, should fall back to stdout/noop
    let config = TelemetryConfig {
        endpoint: None,
        service_name: "animus-test".to_string(),
    };
    let _guard = animus_rs::telemetry::init_telemetry(config).unwrap();
}
```

**Step 2: Implement telemetry/mod.rs**

OTel initialization with tracing-opentelemetry bridge. Configurable: if OTLP endpoint is set, export there; otherwise stdout or noop for tests.

**Step 3: Implement telemetry/genai.rs**

Helper functions that create spans with OTel GenAI semantic convention attributes:
- `start_chat_span` — sets `gen_ai.operation.name = "chat"`, model, provider
- `start_embedding_span` — sets `gen_ai.operation.name = "embeddings"`
- `record_token_usage` — sets input/output/cache token attributes on span

**Step 4: Implement telemetry/work.rs**

Work execution span helpers:
- `start_work_span` — span per work item execution
- Lifecycle events as span events (state transitions)

**Step 5: Run tests**

```bash
cargo test --test telemetry_test -- --nocapture
```

Expected: PASS.

**Step 6: Commit**

```bash
git add src/telemetry/ tests/telemetry_test.rs
git commit -m "feat: OpenTelemetry setup with GenAI semantic conventions"
```

---

## Phase 5: LLM Abstraction and Integration

### Task 12: LLM module (rig-core setup)

**Files:**
- Modify: `src/llm/mod.rs`

**Step 1: Implement llm/mod.rs**

Thin wrapper around rig-core provider initialization:

```rust
//! LLM provider setup via rig-core.
//!
//! Provides helper functions to create CompletionModel and EmbeddingModel
//! instances from configuration.
```

This is primarily a configuration/factory module. The actual LLM calls happen through rig-core's traits, which other modules use directly.

**Step 2: Commit**

```bash
git add src/llm/
git commit -m "feat: llm module for rig-core provider setup"
```

---

### Task 13: Integration tests and docs

**Files:**
- Create: `tests/integration_test.rs`
- Modify: `CLAUDE.md`
- Modify: `DESIGN.md`

**Step 1: Write end-to-end integration test**

```rust
//! Full integration test: submit work → read from queue → search memory

#[tokio::test]
async fn full_lifecycle() {
    let db = test_db().await;
    db.create_queue("work").await.unwrap();

    // Submit work
    let new = NewWorkItem::new("engage", "heartbeat")
        .dedup_key("person=kelly")
        .params(serde_json::json!({"person": "kelly"}));
    let result = db.submit_work(new).await.unwrap();
    // ... assert created

    // Read from queue
    let msg = db.read_from_queue("work", 30).await.unwrap();
    // ... assert message matches

    // Store a memory
    let embedding = vec![0.1_f32; 1536];
    db.store_memory(NewMemory { /* ... */ }).await.unwrap();

    // Search memory
    let results = db.search_memory_by_vector(&embedding, 10, &Default::default()).await.unwrap();
    // ... assert results
}
```

**Step 2: Update CLAUDE.md**

Update with new commands, architecture table, dependencies, and conventions for animus-rs.

**Step 3: Update DESIGN.md**

Reflect the pivot from workq to animus-rs. Reference `docs/db.md` for the database layer design.

**Step 4: Run all tests**

```bash
cargo test
cargo clippy
```

Expected: all pass, no warnings.

**Step 5: Commit**

```bash
git add -A
git commit -m "feat: integration tests and updated docs for animus-rs"
```

---

### Task 14: SQLx offline metadata for CI

**Step 1: Generate offline query metadata**

```bash
cargo sqlx prepare
```

This creates `.sqlx/` directory with cached query metadata so CI can build without a live Postgres.

**Step 2: Commit**

```bash
git add .sqlx/
git commit -m "ci: sqlx offline query metadata"
```

---

Plan complete and saved to `docs/plans/2026-02-27-animus-rs-data-layer.md`. Two execution options:

**1. Subagent-Driven (this session)** - I dispatch fresh subagent per task, review between tasks, fast iteration

**2. Parallel Session (separate)** - Open new session with executing-plans, batch execution with checkpoints

Which approach?
