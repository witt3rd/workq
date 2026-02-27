//! Work item operations: submit with dedup, state tracking, provenance.

use crate::error::{Error, Result};
use crate::model::work::*;
use uuid::Uuid;

/// Result of submitting work.
#[derive(Debug)]
pub enum SubmitResult {
    /// New work item was created and queued.
    Created(WorkItem),
    /// Duplicate detected, merged into existing item.
    Merged {
        new_id: WorkId,
        canonical_id: WorkId,
    },
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
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12)",
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
        .bind(new.parent_id.map(|p| p.0))
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
                 LIMIT 1",
            )
            .bind(&new.work_type)
            .bind(dedup_key)
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;

            if let Some((canonical_id,)) = existing {
                // Merge: mark new item as merged
                sqlx::query(
                    "UPDATE work_items SET state = 'merged', merged_into = $1, resolved_at = now(), updated_at = now() WHERE id = $2",
                )
                .bind(canonical_id)
                .bind(id)
                .execute(&mut *tx)
                .await?;

                tx.commit().await?;
                return Ok(SubmitResult::Merged {
                    new_id: WorkId(id),
                    canonical_id: WorkId(canonical_id),
                });
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
            "UPDATE work_items SET state = 'queued', pgmq_msg_id = $1, updated_at = now() WHERE id = $2",
        )
        .bind(msg_id.0)
        .bind(id)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        let item = self.get_work_item(WorkId(id)).await?;
        Ok(SubmitResult::Created(item))
    }

    /// Get a work item by ID.
    pub async fn get_work_item(&self, id: WorkId) -> Result<WorkItem> {
        let row: Option<WorkItemRow> = sqlx::query_as(
            "SELECT id, work_type, dedup_key, source, trigger_info, params, priority, state, merged_into, parent_id, attempts, max_attempts, created_at, updated_at, resolved_at
             FROM work_items WHERE id = $1",
        )
        .bind(id.0)
        .fetch_optional(&self.pool)
        .await?;

        row.map(WorkItem::from)
            .ok_or_else(|| Error::NotFound(format!("work item {id}")))
    }
}

/// Internal row type for sqlx::FromRow.
#[derive(sqlx::FromRow)]
struct WorkItemRow {
    id: Uuid,
    work_type: String,
    dedup_key: Option<String>,
    source: String,
    trigger_info: Option<String>,
    params: serde_json::Value,
    priority: i32,
    state: String,
    merged_into: Option<Uuid>,
    parent_id: Option<Uuid>,
    attempts: i32,
    max_attempts: Option<i32>,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    resolved_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<WorkItemRow> for WorkItem {
    fn from(row: WorkItemRow) -> Self {
        Self {
            id: WorkId(row.id),
            work_type: row.work_type,
            dedup_key: row.dedup_key,
            provenance: Provenance {
                source: row.source,
                trigger: row.trigger_info,
            },
            params: row.params,
            priority: row.priority,
            state: parse_state(&row.state),
            merged_into: row.merged_into.map(WorkId),
            parent_id: row.parent_id.map(WorkId),
            attempts: row.attempts as u32,
            max_attempts: row.max_attempts.map(|n| n as u32),
            created_at: row.created_at,
            updated_at: row.updated_at,
            resolved_at: row.resolved_at,
        }
    }
}

fn parse_state(s: &str) -> State {
    match s {
        "created" => State::Created,
        "queued" => State::Queued,
        "claimed" => State::Claimed,
        "running" => State::Running,
        "completed" => State::Completed,
        "failed" => State::Failed,
        "dead" => State::Dead,
        "merged" => State::Merged,
        _ => State::Created, // fallback
    }
}
