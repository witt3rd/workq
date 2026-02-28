//! Work item operations: submit with dedup, state tracking, provenance.

use crate::error::{Error, Result};
use crate::model::work::*;
use crate::telemetry::metrics;
use opentelemetry::KeyValue;
use uuid::Uuid;

/// Result of submitting work.
#[derive(Debug)]
pub enum SubmitResult {
    /// New work item was created and queued.
    Created(Box<WorkItem>),
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
                metrics::work_submitted().add(
                    1,
                    &[
                        KeyValue::new("work_type", new.work_type.clone()),
                        KeyValue::new("result", "duplicate"),
                    ],
                );
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

        // NOTIFY is transactional — only fires on commit
        sqlx::query("SELECT pg_notify('work_ready', $1)")
            .bind(&new.work_type)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        metrics::work_submitted().add(
            1,
            &[
                KeyValue::new("work_type", new.work_type),
                KeyValue::new("result", "ok"),
            ],
        );

        let item = self.get_work_item(WorkId(id)).await?;
        Ok(SubmitResult::Created(Box::new(item)))
    }

    /// Get a work item by ID.
    pub async fn get_work_item(&self, id: WorkId) -> Result<WorkItem> {
        let row: Option<WorkItemRow> = sqlx::query_as(
            "SELECT id, work_type, dedup_key, source, trigger_info, params, priority, state, merged_into, parent_id, attempts, max_attempts, created_at, updated_at, resolved_at, outcome_data, outcome_error, outcome_ms
             FROM work_items WHERE id = $1",
        )
        .bind(id.0)
        .fetch_optional(&self.pool)
        .await?;

        row.ok_or_else(|| Error::NotFound(format!("work item {id}")))?
            .try_into_work_item()
    }

    /// Transition a work item's state with optimistic concurrency.
    pub async fn transition_state(&self, id: WorkId, from: State, to: State) -> Result<WorkItem> {
        validate_transition(from, to)?;

        let now = chrono::Utc::now();
        let resolved_at = if to.is_terminal() { Some(now) } else { None };

        // Increment attempts when entering Running
        let attempts_increment = if to == State::Running { 1 } else { 0 };

        let rows_affected = sqlx::query(
            "UPDATE work_items SET state = $1, updated_at = $2, resolved_at = COALESCE($3, resolved_at), attempts = attempts + $4
             WHERE id = $5 AND state = $6",
        )
        .bind(to.to_string())
        .bind(now)
        .bind(resolved_at)
        .bind(attempts_increment)
        .bind(id.0)
        .bind(from.to_string())
        .execute(&self.pool)
        .await?
        .rows_affected();

        if rows_affected == 0 {
            return Err(Error::InvalidTransition {
                from: from.to_string(),
                to: to.to_string(),
            });
        }

        metrics::work_state_transitions().add(
            1,
            &[
                KeyValue::new("from", from.to_string()),
                KeyValue::new("to", to.to_string()),
            ],
        );

        self.get_work_item(id).await
    }

    /// Complete a work item: Running → Completed with outcome data.
    pub async fn complete_work(&self, id: WorkId, outcome: Outcome) -> Result<WorkItem> {
        validate_transition(State::Running, State::Completed)?;

        let now = chrono::Utc::now();
        let rows_affected = sqlx::query(
            "UPDATE work_items SET state = 'completed', updated_at = $1, resolved_at = $1, outcome_data = $2, outcome_error = $3, outcome_ms = $4
             WHERE id = $5 AND state = 'running'",
        )
        .bind(now)
        .bind(&outcome.data)
        .bind(&outcome.error)
        .bind(outcome.duration_ms as i64)
        .bind(id.0)
        .execute(&self.pool)
        .await?
        .rows_affected();

        if rows_affected == 0 {
            return Err(Error::InvalidTransition {
                from: "running".to_string(),
                to: "completed".to_string(),
            });
        }

        metrics::work_state_transitions().add(
            1,
            &[
                KeyValue::new("from", "running"),
                KeyValue::new("to", "completed"),
            ],
        );
        metrics::operation_duration_ms().record(
            outcome.duration_ms as f64,
            &[KeyValue::new("operation", "work.execute")],
        );

        self.get_work_item(id).await
    }

    /// Fail a work item: Running → Failed with error info.
    pub async fn fail_work(&self, id: WorkId, error: &str, duration_ms: u64) -> Result<WorkItem> {
        validate_transition(State::Running, State::Failed)?;

        let now = chrono::Utc::now();
        let rows_affected = sqlx::query(
            "UPDATE work_items SET state = 'failed', updated_at = $1, outcome_error = $2, outcome_ms = $3
             WHERE id = $4 AND state = 'running'",
        )
        .bind(now)
        .bind(error)
        .bind(duration_ms as i64)
        .bind(id.0)
        .execute(&self.pool)
        .await?
        .rows_affected();

        if rows_affected == 0 {
            return Err(Error::InvalidTransition {
                from: "running".to_string(),
                to: "failed".to_string(),
            });
        }

        metrics::work_state_transitions().add(
            1,
            &[
                KeyValue::new("from", "running"),
                KeyValue::new("to", "failed"),
            ],
        );
        metrics::operation_duration_ms().record(
            duration_ms as f64,
            &[KeyValue::new("operation", "work.execute")],
        );

        self.get_work_item(id).await
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
    outcome_data: Option<serde_json::Value>,
    outcome_error: Option<String>,
    outcome_ms: Option<i64>,
}

impl WorkItemRow {
    fn try_into_work_item(self) -> Result<WorkItem> {
        let outcome = if self.outcome_data.is_some() || self.outcome_error.is_some() {
            Some(Outcome {
                success: self.outcome_error.is_none(),
                data: self.outcome_data,
                error: self.outcome_error,
                duration_ms: self.outcome_ms.unwrap_or(0) as u64,
            })
        } else {
            None
        };

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
            outcome,
        })
    }
}
