//! Core data model.
//!
//! A work item is something that needs doing. It has identity (type + dedup key),
//! provenance (where it came from), priority, and lifecycle state.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Work Item
// ---------------------------------------------------------------------------

/// A unit of work tracked by the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkItem {
    /// Unique identifier.
    pub id: WorkId,

    /// What kind of work this is (e.g., "engage", "project-check").
    /// Determines which worker handles it and capacity accounting.
    pub work_type: String,

    /// Structural dedup key. Work items with the same (work_type, dedup_key)
    /// are candidates for dedup. None means no structural dedup.
    pub dedup_key: Option<String>,

    /// Where this work came from.
    pub provenance: Provenance,

    /// Arbitrary parameters for the worker. The engine doesn't interpret these.
    pub params: serde_json::Value,

    /// Priority. Higher = more urgent. Engine provides base priority per
    /// work type; provenance and age can boost it.
    pub priority: i32,

    /// Current lifecycle state.
    pub state: State,

    /// If merged, the canonical work item this was merged into.
    pub merged_into: Option<WorkId>,

    /// Parent work item (if spawned by another work item's worker).
    pub parent_id: Option<WorkId>,

    /// Number of execution attempts so far.
    pub attempts: u32,

    /// Maximum retry attempts before going dead. None = use engine default.
    pub max_attempts: Option<u32>,

    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}

/// Newtype for work item IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkId(pub Uuid);

impl WorkId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl std::fmt::Display for WorkId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Short display: first 8 chars of UUID
        write!(f, "{}", &self.0.to_string()[..8])
    }
}

impl Default for WorkId {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// Lifecycle state of a work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Submitted, pending dedup check.
    Created,
    /// Ready for execution, waiting for a worker.
    Queued,
    /// Worker assigned, execution starting.
    Claimed,
    /// Worker actively processing.
    Running,
    /// Done successfully.
    Completed,
    /// Execution failed, may be retried.
    Failed,
    /// Exhausted retries or poisoned. Terminal.
    Dead,
    /// Recognized as duplicate, linked to canonical item. Terminal.
    Merged,
}

impl State {
    /// Can transition from self to `to`?
    pub fn can_transition_to(self, to: State) -> bool {
        use State::*;
        matches!(
            (self, to),
            (Created, Queued)
                | (Created, Merged)
                | (Queued, Claimed)
                | (Queued, Dead)        // cancelled or circuit-broken
                | (Claimed, Running)
                | (Claimed, Queued)     // worker failed to start, re-queue
                | (Running, Completed)
                | (Running, Failed)
                | (Failed, Queued)      // retry
                | (Failed, Dead) // exhausted retries
        )
    }

    /// Is this a terminal state?
    pub fn is_terminal(self) -> bool {
        matches!(self, State::Completed | State::Dead | State::Merged)
    }
}

impl std::fmt::Display for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            State::Created => "created",
            State::Queued => "queued",
            State::Claimed => "claimed",
            State::Running => "running",
            State::Completed => "completed",
            State::Failed => "failed",
            State::Dead => "dead",
            State::Merged => "merged",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// Provenance
// ---------------------------------------------------------------------------

/// Where a work item came from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    /// High-level source (e.g., "user", "heartbeat", "initiative", "worker").
    pub source: String,

    /// More specific trigger (e.g., "skill/check-in", "user/kelly").
    pub trigger: Option<String>,
}

// ---------------------------------------------------------------------------
// Outcome
// ---------------------------------------------------------------------------

/// Result of work execution, stored with the work item on completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub success: bool,
    /// Arbitrary result data. Opaque to the engine.
    pub data: Option<serde_json::Value>,
    /// Error message if failed.
    pub error: Option<String>,
    /// Execution duration.
    pub duration_ms: u64,
}

// ---------------------------------------------------------------------------
// Log Entry
// ---------------------------------------------------------------------------

/// A log entry scoped to a work item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub work_id: WorkId,
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Debug,
    Info,
    Warn,
    Error,
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        };
        write!(f, "{s}")
    }
}

// ---------------------------------------------------------------------------
// Builder
// ---------------------------------------------------------------------------

/// Builder for creating new work items. The engine's public API for submitting work.
pub struct NewWorkItem {
    pub(crate) work_type: String,
    pub(crate) dedup_key: Option<String>,
    pub(crate) provenance: Provenance,
    pub(crate) params: serde_json::Value,
    pub(crate) priority: i32,
    pub(crate) parent_id: Option<WorkId>,
    pub(crate) max_attempts: Option<u32>,
}

impl NewWorkItem {
    pub fn new(work_type: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            work_type: work_type.into(),
            dedup_key: None,
            provenance: Provenance {
                source: source.into(),
                trigger: None,
            },
            params: serde_json::Value::Null,
            priority: 0,
            parent_id: None,
            max_attempts: None,
        }
    }

    pub fn dedup_key(mut self, key: impl Into<String>) -> Self {
        self.dedup_key = Some(key.into());
        self
    }

    pub fn trigger(mut self, trigger: impl Into<String>) -> Self {
        self.provenance.trigger = Some(trigger.into());
        self
    }

    pub fn params(mut self, params: serde_json::Value) -> Self {
        self.params = params;
        self
    }

    pub fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    pub fn parent(mut self, parent_id: WorkId) -> Self {
        self.parent_id = Some(parent_id);
        self
    }

    pub fn max_attempts(mut self, n: u32) -> Self {
        self.max_attempts = Some(n);
        self
    }
}
