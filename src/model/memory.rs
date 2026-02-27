//! Memory entry types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A stored memory entry retrieved from the database.
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

/// Parameters for creating a new memory entry.
#[derive(Debug, Clone)]
pub struct NewMemory {
    pub content: String,
    pub memory_type: String,
    pub source: Option<String>,
    pub metadata: serde_json::Value,
    pub embedding: Vec<f32>,
}

/// Filters for memory search queries.
#[derive(Debug, Clone, Default)]
pub struct MemoryFilters {
    pub memory_type: Option<String>,
    pub source: Option<String>,
    pub since: Option<DateTime<Utc>>,
}
