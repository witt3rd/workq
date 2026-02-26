//! Structured events emitted by the engine on every state transition.
//!
//! Consumers subscribe to the event stream to build dashboards,
//! alerting, or audit logs. Events are the engine's voice;
//! work-scoped logs are the worker's voice.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::model::{State, WorkId};

/// A structured event emitted by the engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Monotonic sequence number. Consumers can detect gaps.
    pub seq: u64,
    /// When this event occurred.
    pub timestamp: DateTime<Utc>,
    /// What happened.
    pub kind: EventKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EventKind {
    WorkCreated {
        id: WorkId,
        work_type: String,
        dedup_key: Option<String>,
        priority: i32,
        source: String,
    },
    WorkMerged {
        id: WorkId,
        canonical_id: WorkId,
        reason: String,
    },
    WorkQueued {
        id: WorkId,
        priority: i32,
    },
    WorkClaimed {
        id: WorkId,
        worker_id: String,
    },
    WorkRunning {
        id: WorkId,
        worker_id: String,
    },
    WorkCompleted {
        id: WorkId,
        duration_ms: u64,
    },
    WorkFailed {
        id: WorkId,
        error: String,
        retryable: bool,
        attempt: u32,
    },
    WorkDead {
        id: WorkId,
        reason: String,
        attempts: u32,
    },
    WorkSpawned {
        parent_id: WorkId,
        child_ids: Vec<WorkId>,
    },
    StateTransition {
        id: WorkId,
        from: State,
        to: State,
    },
}
