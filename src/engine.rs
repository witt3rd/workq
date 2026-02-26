//! Core engine. The public API for submitting and managing work.
//!
//! The engine owns the storage and event stream. All state transitions
//! go through here. External consumers interact via this module.

use chrono::Utc;

use crate::error::Result;
use crate::event::EventKind;
use crate::model::*;
use crate::storage::Storage;

/// The work engine. Owns all state and enforces all invariants.
pub struct Engine {
    storage: Storage,
    /// Default max attempts before a work item goes dead.
    pub default_max_attempts: u32,
}

/// What happened when work was submitted.
#[derive(Debug)]
pub enum SubmitResult {
    /// New work item created and queued.
    Created(WorkItem),
    /// Merged into an existing work item (dedup hit).
    Merged {
        new_id: WorkId,
        canonical_id: WorkId,
    },
}

impl Engine {
    /// Create an engine with in-memory storage (for testing).
    pub fn in_memory() -> Result<Self> {
        Ok(Self {
            storage: Storage::in_memory()?,
            default_max_attempts: 3,
        })
    }

    /// Create an engine backed by a file.
    pub fn open(path: &str) -> Result<Self> {
        Ok(Self {
            storage: Storage::open(path)?,
            default_max_attempts: 3,
        })
    }

    /// Submit new work. Performs structural dedup check, then queues.
    ///
    /// The entire flow (insert + dedup check + merge-or-queue + events) runs
    /// within a single SQLite transaction for crash safety and correctness
    /// under concurrent access.
    pub fn submit(&mut self, new: NewWorkItem) -> Result<SubmitResult> {
        let now = Utc::now();
        let id = WorkId::new();

        let item = WorkItem {
            id,
            work_type: new.work_type.clone(),
            dedup_key: new.dedup_key.clone(),
            provenance: new.provenance.clone(),
            params: new.params.clone(),
            priority: new.priority,
            state: State::Created,
            merged_into: None,
            parent_id: new.parent_id,
            attempts: 0,
            max_attempts: new.max_attempts,
            created_at: now,
            updated_at: now,
            completed_at: None,
        };

        self.storage.with_transaction(|ctx| {
            ctx.insert_work_item(&item)?;

            ctx.record_event(EventKind::WorkCreated {
                id,
                work_type: new.work_type.clone(),
                dedup_key: new.dedup_key.clone(),
                priority: new.priority,
                source: new.provenance.source.clone(),
            })?;

            // Structural dedup check — layer 1 (exact key match).
            // Future layers (embedding similarity, LLM evaluation) can be
            // added here within the same transaction boundary.
            if let Some(ref dedup_key) = new.dedup_key {
                let candidates = ctx.find_active_by_dedup(&new.work_type, dedup_key)?;

                // Find existing (not the one we just inserted)
                let existing = candidates.iter().find(|c| c.id != id);

                if let Some(canonical) = existing {
                    let canonical_id = canonical.id;

                    ctx.merge_work_item(id, canonical_id)?;

                    ctx.record_event(EventKind::WorkMerged {
                        id,
                        canonical_id,
                        reason: format!("structural dedup: {}={}", new.work_type, dedup_key),
                    })?;

                    return Ok(SubmitResult::Merged {
                        new_id: id,
                        canonical_id,
                    });
                }
            }

            // No dedup match — queue it
            ctx.update_state(id, State::Queued)?;

            ctx.record_event(EventKind::WorkQueued {
                id,
                priority: new.priority,
            })?;

            let item = ctx.get_work_item(id)?;
            Ok(SubmitResult::Created(item))
        })
    }

    /// Get a work item by ID.
    pub fn get(&self, id: WorkId) -> Result<WorkItem> {
        self.storage.get_work_item(id)
    }

    /// List work items by state.
    pub fn list_by_state(&self, state: State) -> Result<Vec<WorkItem>> {
        self.storage.list_by_state(state)
    }

    /// Claim the next queued work item for a worker. Returns None if queue is empty.
    pub fn claim(&mut self, worker_id: &str) -> Result<Option<WorkItem>> {
        let queued = self.storage.list_by_state(State::Queued)?;
        let Some(item) = queued.into_iter().next() else {
            return Ok(None);
        };

        self.storage.update_state(item.id, State::Claimed)?;

        self.storage.record_event(EventKind::WorkClaimed {
            id: item.id,
            worker_id: worker_id.to_string(),
        })?;

        self.storage.get_work_item(item.id).map(Some)
    }

    /// Mark a claimed work item as running.
    pub fn start(&mut self, id: WorkId, worker_id: &str) -> Result<()> {
        self.storage.update_state(id, State::Running)?;
        self.storage.increment_attempts(id)?;

        self.storage.record_event(EventKind::WorkRunning {
            id,
            worker_id: worker_id.to_string(),
        })?;

        Ok(())
    }

    /// Mark a running work item as completed.
    pub fn complete(&mut self, id: WorkId, outcome: Outcome) -> Result<()> {
        self.storage.set_outcome(id, &outcome)?;
        self.storage.update_state(id, State::Completed)?;

        self.storage.record_event(EventKind::WorkCompleted {
            id,
            duration_ms: outcome.duration_ms,
        })?;

        Ok(())
    }

    /// Mark a running work item as failed. May be retried or go dead.
    ///
    /// `start()` already incremented `attempts`, so we use the current
    /// value directly — no extra +1 here.
    pub fn fail(&mut self, id: WorkId, error: &str, retryable: bool) -> Result<()> {
        let item = self.storage.get_work_item(id)?;
        let max = item.max_attempts.unwrap_or(self.default_max_attempts);
        let attempts = item.attempts; // Already incremented by start()

        self.storage.update_state(id, State::Failed)?;

        self.storage.record_event(EventKind::WorkFailed {
            id,
            error: error.to_string(),
            retryable,
            attempt: attempts,
        })?;

        if !retryable || attempts >= max {
            // Go dead
            self.storage.update_state(id, State::Dead)?;
            self.storage.record_event(EventKind::WorkDead {
                id,
                reason: if !retryable {
                    format!("non-retryable failure: {error}")
                } else {
                    format!("exhausted {attempts}/{max} attempts: {error}")
                },
                attempts,
            })?;
        } else {
            // Re-queue for retry
            self.storage.update_state(id, State::Queued)?;
            self.storage.record_event(EventKind::WorkQueued {
                id,
                priority: item.priority,
            })?;
        }

        Ok(())
    }

    /// Append a log entry for a work item.
    pub fn log(
        &mut self,
        work_id: WorkId,
        level: LogLevel,
        message: impl Into<String>,
    ) -> Result<()> {
        self.storage.append_log(&LogEntry {
            work_id,
            timestamp: Utc::now(),
            level,
            message: message.into(),
        })
    }

    /// Get logs for a work item.
    pub fn get_logs(&self, work_id: WorkId) -> Result<Vec<LogEntry>> {
        self.storage.get_logs(work_id)
    }

    /// Get events since a sequence number.
    pub fn get_events_since(&self, since_seq: u64) -> Result<Vec<crate::event::Event>> {
        self.storage.get_events_since(since_seq)
    }
}
