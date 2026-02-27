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

/// Validate a state transition, returning an error if disallowed.
fn validate_transition(from: State, to: State) -> Result<()> {
    if from.can_transition_to(to) {
        Ok(())
    } else {
        Err(Error::InvalidTransition {
            from: from.to_string(),
            to: to.to_string(),
        })
    }
}

impl super::Db {
    /// Submit new work. Checks structural dedup, sends to pgmq queue.
    pub async fn submit_work(&self, new: NewWorkItem) -> Result<SubmitResult> {
        let mut tx = self.pool.begin().await?;
        let id = Uuid::new_v4();
        let now = chrono::Utc::now();

        if let Some(ref dedup_key) = new.dedup_key {
            // Attempt insert with ON CONFLICT for dedup-enabled items.
            // The unique partial index on (work_type, dedup_key) prevents
            // concurrent inserts with the same key for active items.
            let inserted: Option<(Uuid,)> = sqlx::query_as(
                "INSERT INTO work_items (id, queue_name, work_type, dedup_key, source, trigger_info, params, priority, state, parent_id, max_attempts, created_at, updated_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $12)
                 ON CONFLICT (work_type, dedup_key) WHERE dedup_key IS NOT NULL AND state NOT IN ('completed', 'dead', 'merged')
                 DO NOTHING
                 RETURNING id",
            )
            .bind(id)
            .bind("work")
            .bind(&new.work_type)
            .bind(dedup_key)
            .bind(&new.provenance.source)
            .bind(&new.provenance.trigger)
            .bind(&new.params)
            .bind(new.priority)
            .bind("created")
            .bind(new.parent_id.map(|p| p.0))
            .bind(new.max_attempts.map(|n| n as i32))
            .bind(now)
            .fetch_optional(&mut *tx)
            .await?;

            if inserted.is_none() {
                // Conflict: a duplicate exists. Find the canonical item.
                let canonical: (Uuid,) = sqlx::query_as(
                    "SELECT id FROM work_items
                     WHERE work_type = $1 AND dedup_key = $2
                     AND state NOT IN ('completed', 'dead', 'merged')
                     LIMIT 1",
                )
                .bind(&new.work_type)
                .bind(dedup_key)
                .fetch_one(&mut *tx)
                .await?;

                // Insert the new item as merged (dedup_key = NULL to avoid
                // conflicting with the unique index).
                validate_transition(State::Created, State::Merged)?;
                sqlx::query(
                    "INSERT INTO work_items (id, queue_name, work_type, dedup_key, source, trigger_info, params, priority, state, merged_into, parent_id, max_attempts, created_at, updated_at, resolved_at)
                     VALUES ($1, $2, $3, NULL, $4, $5, $6, $7, 'merged', $8, $9, $10, $11, $11, $11)",
                )
                .bind(id)
                .bind("work")
                .bind(&new.work_type)
                .bind(&new.provenance.source)
                .bind(&new.provenance.trigger)
                .bind(&new.params)
                .bind(new.priority)
                .bind(canonical.0)
                .bind(new.parent_id.map(|p| p.0))
                .bind(new.max_attempts.map(|n| n as i32))
                .bind(now)
                .execute(&mut *tx)
                .await?;

                tx.commit().await?;
                return Ok(SubmitResult::Merged {
                    new_id: WorkId(id),
                    canonical_id: WorkId(canonical.0),
                });
            }
        } else {
            // No dedup key — straight insert, no conflict possible
            sqlx::query(
                "INSERT INTO work_items (id, queue_name, work_type, dedup_key, source, trigger_info, params, priority, state, parent_id, max_attempts, created_at, updated_at)
                 VALUES ($1, $2, $3, NULL, $4, $5, $6, $7, $8, $9, $10, $11, $11)",
            )
            .bind(id)
            .bind("work")
            .bind(&new.work_type)
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
        }

        // Inserted successfully — queue via pgmq
        validate_transition(State::Created, State::Queued)?;

        let payload = serde_json::json!({
            "work_item_id": id,
            "params": new.params
        });
        let msg_id: (i64,) = sqlx::query_as("SELECT pgmq.send($1, $2, $3)")
            .bind("work")
            .bind(&payload)
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

        row.ok_or_else(|| Error::NotFound(format!("work item {id}")))?
            .try_into_work_item()
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

impl WorkItemRow {
    fn try_into_work_item(self) -> Result<WorkItem> {
        Ok(WorkItem {
            id: WorkId(self.id),
            work_type: self.work_type,
            dedup_key: self.dedup_key,
            provenance: Provenance {
                source: self.source,
                trigger: self.trigger_info,
            },
            params: self.params,
            priority: self.priority,
            state: self.state.parse()?,
            merged_into: self.merged_into.map(WorkId),
            parent_id: self.parent_id.map(WorkId),
            attempts: self.attempts as u32,
            max_attempts: self.max_attempts.map(|n| n as u32),
            created_at: self.created_at,
            updated_at: self.updated_at,
            resolved_at: self.resolved_at,
        })
    }
}
