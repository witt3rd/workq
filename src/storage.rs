//! SQLite storage layer.
//!
//! Single source of truth for all work item state, logs, and events.
//! WAL mode for concurrent read access. All writes go through the engine.

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};

use crate::error::{Error, Result};
use crate::event::{Event, EventKind};
use crate::model::*;

/// Storage backend. Owns the SQLite connection.
pub struct Storage {
    conn: Connection,
    event_seq: u64,
}

impl Storage {
    /// Open or create a database at the given path.
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        let mut storage = Self { conn, event_seq: 0 };
        storage.init()?;
        Ok(storage)
    }

    /// Create an in-memory database (for testing).
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        let mut storage = Self { conn, event_seq: 0 };
        storage.init()?;
        Ok(storage)
    }

    fn init(&mut self) -> Result<()> {
        // WAL mode for concurrent readers
        self.conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        self.conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS work_items (
                id              TEXT PRIMARY KEY,
                work_type       TEXT NOT NULL,
                dedup_key       TEXT,
                source          TEXT NOT NULL,
                trigger_        TEXT,
                params          TEXT NOT NULL DEFAULT '{}',
                priority        INTEGER NOT NULL DEFAULT 0,
                state           TEXT NOT NULL DEFAULT 'created',
                merged_into     TEXT REFERENCES work_items(id),
                parent_id       TEXT REFERENCES work_items(id),
                attempts        INTEGER NOT NULL DEFAULT 0,
                max_attempts    INTEGER,
                outcome_data    TEXT,
                outcome_error   TEXT,
                outcome_ms      INTEGER,
                created_at      TEXT NOT NULL,
                updated_at      TEXT NOT NULL,
                completed_at    TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_work_type ON work_items(work_type);
            CREATE INDEX IF NOT EXISTS idx_state ON work_items(state);
            CREATE INDEX IF NOT EXISTS idx_dedup ON work_items(work_type, dedup_key)
                WHERE dedup_key IS NOT NULL AND state NOT IN ('completed', 'dead', 'merged');
            CREATE INDEX IF NOT EXISTS idx_parent ON work_items(parent_id)
                WHERE parent_id IS NOT NULL;
            CREATE INDEX IF NOT EXISTS idx_queued ON work_items(priority DESC, created_at ASC)
                WHERE state = 'queued';

            CREATE TABLE IF NOT EXISTS logs (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                work_id     TEXT NOT NULL REFERENCES work_items(id),
                timestamp   TEXT NOT NULL,
                level       TEXT NOT NULL,
                message     TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_logs_work ON logs(work_id, timestamp);

            CREATE TABLE IF NOT EXISTS events (
                seq         INTEGER PRIMARY KEY AUTOINCREMENT,
                timestamp   TEXT NOT NULL,
                kind        TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS merged_provenance (
                id          INTEGER PRIMARY KEY AUTOINCREMENT,
                work_id     TEXT NOT NULL REFERENCES work_items(id),
                source      TEXT NOT NULL,
                trigger_    TEXT,
                created_at  TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_merged_prov ON merged_provenance(work_id);
            ",
        )?;

        // Initialize event sequence from DB
        let max_seq: Option<u64> = self
            .conn
            .query_row("SELECT MAX(seq) FROM events", [], |row| row.get(0))
            .optional()?
            .flatten();
        self.event_seq = max_seq.unwrap_or(0);

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Work Items
    // -----------------------------------------------------------------------

    /// Insert a new work item.
    pub fn insert_work_item(&mut self, item: &WorkItem) -> Result<()> {
        self.conn.execute(
            "INSERT INTO work_items (
                id, work_type, dedup_key, source, trigger_, params, priority,
                state, merged_into, parent_id, attempts, max_attempts,
                created_at, updated_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                item.id.0.to_string(),
                item.work_type,
                item.dedup_key,
                item.provenance.source,
                item.provenance.trigger,
                serde_json::to_string(&item.params).unwrap_or_default(),
                item.priority,
                item.state.to_string(),
                item.merged_into.map(|id| id.0.to_string()),
                item.parent_id.map(|id| id.0.to_string()),
                item.attempts,
                item.max_attempts,
                item.created_at.to_rfc3339(),
                item.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Update a work item's state. Returns the previous state.
    pub fn update_state(&mut self, id: WorkId, new_state: State) -> Result<State> {
        let old_state = self.get_state(id)?;

        if !old_state.can_transition_to(new_state) {
            return Err(Error::InvalidTransition {
                from: old_state,
                to: new_state,
            });
        }

        let now = Utc::now().to_rfc3339();
        let completed_at = if new_state.is_terminal() {
            Some(now.clone())
        } else {
            None
        };

        self.conn.execute(
            "UPDATE work_items SET state = ?1, updated_at = ?2, completed_at = COALESCE(?3, completed_at) WHERE id = ?4",
            params![new_state.to_string(), now, completed_at, id.0.to_string()],
        )?;

        Ok(old_state)
    }

    /// Get the current state of a work item.
    fn get_state(&self, id: WorkId) -> Result<State> {
        let state_str: String = self
            .conn
            .query_row(
                "SELECT state FROM work_items WHERE id = ?1",
                params![id.0.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(id.to_string()))?;

        parse_state(&state_str)
    }

    /// Get a work item by ID.
    pub fn get_work_item(&self, id: WorkId) -> Result<WorkItem> {
        self.conn
            .query_row(
                "SELECT * FROM work_items WHERE id = ?1",
                params![id.0.to_string()],
                |row| Ok(row_to_work_item(row)),
            )?
            .map_err(|e| Error::Other(format!("failed to parse work item: {e}")))
    }

    /// Find active (non-terminal) work items matching a dedup key.
    pub fn find_active_by_dedup(&self, work_type: &str, dedup_key: &str) -> Result<Vec<WorkItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM work_items
             WHERE work_type = ?1 AND dedup_key = ?2
             AND state NOT IN ('completed', 'dead', 'merged')
             ORDER BY created_at ASC",
        )?;

        let items = stmt
            .query_map(params![work_type, dedup_key], |row| {
                Ok(row_to_work_item(row))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        // Unwrap the inner Results
        let mut result = Vec::new();
        for item in items {
            result.push(item.map_err(|e| Error::Other(format!("parse error: {e}")))?);
        }
        Ok(result)
    }

    /// List work items by state.
    pub fn list_by_state(&self, state: State) -> Result<Vec<WorkItem>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM work_items WHERE state = ?1 ORDER BY priority DESC, created_at ASC",
        )?;

        let items = stmt
            .query_map(params![state.to_string()], |row| Ok(row_to_work_item(row)))?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut result = Vec::new();
        for item in items {
            result.push(item.map_err(|e| Error::Other(format!("parse error: {e}")))?);
        }
        Ok(result)
    }

    /// Set merged_into and record the merged provenance.
    pub fn merge_work_item(&mut self, id: WorkId, canonical_id: WorkId) -> Result<()> {
        // Get the item being merged so we can preserve its provenance
        let item = self.get_work_item(id)?;

        // Record merged provenance on the canonical item
        self.conn.execute(
            "INSERT INTO merged_provenance (work_id, source, trigger_, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                canonical_id.0.to_string(),
                item.provenance.source,
                item.provenance.trigger,
                item.created_at.to_rfc3339(),
            ],
        )?;

        // Update the merged item
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE work_items SET state = 'merged', merged_into = ?1, updated_at = ?2, completed_at = ?2 WHERE id = ?3",
            params![canonical_id.0.to_string(), now, id.0.to_string()],
        )?;

        Ok(())
    }

    /// Increment attempt count.
    pub fn increment_attempts(&mut self, id: WorkId) -> Result<u32> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE work_items SET attempts = attempts + 1, updated_at = ?1 WHERE id = ?2",
            params![now, id.0.to_string()],
        )?;

        let attempts: u32 = self.conn.query_row(
            "SELECT attempts FROM work_items WHERE id = ?1",
            params![id.0.to_string()],
            |row| row.get(0),
        )?;

        Ok(attempts)
    }

    /// Store an outcome on a work item.
    pub fn set_outcome(&mut self, id: WorkId, outcome: &Outcome) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        self.conn.execute(
            "UPDATE work_items SET outcome_data = ?1, outcome_error = ?2, outcome_ms = ?3, updated_at = ?4 WHERE id = ?5",
            params![
                outcome.data.as_ref().map(|d| serde_json::to_string(d).unwrap_or_default()),
                outcome.error,
                outcome.duration_ms as i64,
                now,
                id.0.to_string(),
            ],
        )?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Logs
    // -----------------------------------------------------------------------

    /// Append a log entry for a work item.
    pub fn append_log(&mut self, entry: &LogEntry) -> Result<()> {
        self.conn.execute(
            "INSERT INTO logs (work_id, timestamp, level, message) VALUES (?1, ?2, ?3, ?4)",
            params![
                entry.work_id.0.to_string(),
                entry.timestamp.to_rfc3339(),
                entry.level.to_string(),
                entry.message,
            ],
        )?;
        Ok(())
    }

    /// Get logs for a work item, ordered by timestamp.
    pub fn get_logs(&self, work_id: WorkId) -> Result<Vec<LogEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT work_id, timestamp, level, message FROM logs WHERE work_id = ?1 ORDER BY timestamp ASC",
        )?;

        let entries = stmt
            .query_map(params![work_id.0.to_string()], |row| {
                Ok(LogEntry {
                    work_id: WorkId(row.get::<_, String>(0)?.parse().unwrap()),
                    timestamp: row
                        .get::<_, String>(1)?
                        .parse()
                        .unwrap_or_else(|_| Utc::now()),
                    level: parse_log_level(&row.get::<_, String>(2)?),
                    message: row.get(3)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(entries)
    }

    // -----------------------------------------------------------------------
    // Events
    // -----------------------------------------------------------------------

    /// Record an event and return it with its sequence number.
    pub fn record_event(&mut self, kind: EventKind) -> Result<Event> {
        self.event_seq += 1;
        let now = Utc::now();

        let event = Event {
            seq: self.event_seq,
            timestamp: now,
            kind: kind.clone(),
        };

        self.conn.execute(
            "INSERT INTO events (seq, timestamp, kind) VALUES (?1, ?2, ?3)",
            params![
                event.seq as i64,
                event.timestamp.to_rfc3339(),
                serde_json::to_string(&event.kind).unwrap_or_default(),
            ],
        )?;

        Ok(event)
    }

    /// Get events since a sequence number.
    pub fn get_events_since(&self, since_seq: u64) -> Result<Vec<Event>> {
        let mut stmt = self
            .conn
            .prepare("SELECT seq, timestamp, kind FROM events WHERE seq > ?1 ORDER BY seq ASC")?;

        let events = stmt
            .query_map(params![since_seq as i64], |row| {
                let kind_str: String = row.get(2)?;
                Ok(Event {
                    seq: row.get::<_, i64>(0)? as u64,
                    timestamp: row
                        .get::<_, String>(1)?
                        .parse()
                        .unwrap_or_else(|_| Utc::now()),
                    kind: serde_json::from_str(&kind_str).unwrap_or(EventKind::WorkCreated {
                        id: WorkId::new(),
                        work_type: "?".into(),
                        dedup_key: None,
                        priority: 0,
                        source: "?".into(),
                    }),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(events)
    }
}

// ---------------------------------------------------------------------------
// Row parsing helpers
// ---------------------------------------------------------------------------

fn row_to_work_item(row: &rusqlite::Row) -> std::result::Result<WorkItem, String> {
    let id_str: String = row.get(0).map_err(|e| e.to_string())?;
    let params_str: String = row.get(5).map_err(|e| e.to_string())?;
    let state_str: String = row.get(7).map_err(|e| e.to_string())?;
    let merged_str: Option<String> = row.get(8).map_err(|e| e.to_string())?;
    let parent_str: Option<String> = row.get(9).map_err(|e| e.to_string())?;
    let created_str: String = row.get(15).map_err(|e| e.to_string())?;
    let updated_str: String = row.get(16).map_err(|e| e.to_string())?;
    let completed_str: Option<String> = row.get(17).map_err(|e| e.to_string())?;

    Ok(WorkItem {
        id: WorkId(id_str.parse().map_err(|e: uuid::Error| e.to_string())?),
        work_type: row.get(1).map_err(|e| e.to_string())?,
        dedup_key: row.get(2).map_err(|e| e.to_string())?,
        provenance: Provenance {
            source: row.get(3).map_err(|e| e.to_string())?,
            trigger: row.get(4).map_err(|e| e.to_string())?,
        },
        params: serde_json::from_str(&params_str).unwrap_or(serde_json::Value::Null),
        priority: row.get(6).map_err(|e| e.to_string())?,
        state: parse_state(&state_str).map_err(|e| e.to_string())?,
        merged_into: merged_str
            .map(|s| s.parse().map(WorkId))
            .transpose()
            .map_err(|e: uuid::Error| e.to_string())?,
        parent_id: parent_str
            .map(|s| s.parse().map(WorkId))
            .transpose()
            .map_err(|e: uuid::Error| e.to_string())?,
        attempts: row.get(10).map_err(|e| e.to_string())?,
        max_attempts: row.get(11).map_err(|e| e.to_string())?,
        created_at: created_str
            .parse()
            .map_err(|_| "invalid created_at".to_string())?,
        updated_at: updated_str
            .parse()
            .map_err(|_| "invalid updated_at".to_string())?,
        completed_at: completed_str.and_then(|s| s.parse().ok()),
    })
}

fn parse_state(s: &str) -> Result<State> {
    match s {
        "created" => Ok(State::Created),
        "queued" => Ok(State::Queued),
        "claimed" => Ok(State::Claimed),
        "running" => Ok(State::Running),
        "completed" => Ok(State::Completed),
        "failed" => Ok(State::Failed),
        "dead" => Ok(State::Dead),
        "merged" => Ok(State::Merged),
        _ => Err(Error::Other(format!("unknown state: {s}"))),
    }
}

fn parse_log_level(s: &str) -> LogLevel {
    match s {
        "DEBUG" => LogLevel::Debug,
        "INFO" => LogLevel::Info,
        "WARN" => LogLevel::Warn,
        "ERROR" => LogLevel::Error,
        _ => LogLevel::Info,
    }
}
