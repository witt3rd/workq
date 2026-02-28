//! pgmq queue operations via direct SQLx.
//!
//! Calls pgmq's SQL functions: pgmq.create, pgmq.send, pgmq.read,
//! pgmq.archive, pgmq.delete.

use crate::error::Result;
use crate::telemetry::metrics;
use opentelemetry::KeyValue;

/// A message read from a pgmq queue.
#[derive(Debug, Clone)]
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
        metrics::queue_operations().add(
            1,
            &[
                KeyValue::new("queue", queue_name.to_string()),
                KeyValue::new("operation", "create"),
            ],
        );
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
        let row: (i64,) = sqlx::query_as("SELECT pgmq.send($1, $2, $3)")
            .bind(queue_name)
            .bind(payload)
            .bind(delay_seconds)
            .fetch_one(&self.pool)
            .await?;
        metrics::queue_operations().add(
            1,
            &[
                KeyValue::new("queue", queue_name.to_string()),
                KeyValue::new("operation", "send"),
            ],
        );
        Ok(row.0)
    }

    /// Read the next message from a queue (visibility timeout in seconds).
    /// Returns None if queue is empty.
    pub async fn read_from_queue(
        &self,
        queue_name: &str,
        vt_seconds: i32,
    ) -> Result<Option<PgmqMessage>> {
        let row = sqlx::query_as::<
            _,
            (
                i64,
                i32,
                chrono::DateTime<chrono::Utc>,
                chrono::DateTime<chrono::Utc>,
                serde_json::Value,
            ),
        >(
            "SELECT msg_id, read_ct, enqueued_at, vt, message FROM pgmq.read($1, $2, 1)"
        )
        .bind(queue_name)
        .bind(vt_seconds)
        .fetch_optional(&self.pool)
        .await?;

        let msg = row.map(|(msg_id, read_ct, enqueued_at, vt, message)| PgmqMessage {
            msg_id,
            read_ct,
            enqueued_at,
            vt,
            message,
        });

        metrics::queue_operations().add(
            1,
            &[
                KeyValue::new("queue", queue_name.to_string()),
                KeyValue::new(
                    "operation",
                    if msg.is_some() { "read" } else { "read_empty" },
                ),
            ],
        );

        Ok(msg)
    }

    /// Archive a message (moves to archive table, preserves for audit).
    pub async fn archive_message(&self, queue_name: &str, msg_id: i64) -> Result<()> {
        sqlx::query("SELECT pgmq.archive($1, $2)")
            .bind(queue_name)
            .bind(msg_id)
            .execute(&self.pool)
            .await?;
        metrics::queue_operations().add(
            1,
            &[
                KeyValue::new("queue", queue_name.to_string()),
                KeyValue::new("operation", "archive"),
            ],
        );
        Ok(())
    }

    /// Delete a message permanently.
    pub async fn delete_message(&self, queue_name: &str, msg_id: i64) -> Result<()> {
        sqlx::query("SELECT pgmq.delete($1, $2)")
            .bind(queue_name)
            .bind(msg_id)
            .execute(&self.pool)
            .await?;
        metrics::queue_operations().add(
            1,
            &[
                KeyValue::new("queue", queue_name.to_string()),
                KeyValue::new("operation", "delete"),
            ],
        );
        Ok(())
    }
}
