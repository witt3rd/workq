# Code Review Issues

Overall assessment: **strong initial implementation**. Clean architecture, correct state machine, sound dedup, good test coverage.

---

## Critical (3)

### 1. Submit is not transactional

**File:** `src/engine.rs:50-118`

The current flow is: insert the item in `Created` state, then check for dedup matches, then either merge or transition to `Queued`. The problem is that two concurrent calls to `submit()` with the same dedup key can both insert, both see only their own item (the other hasn't been committed yet), and both proceed to `Queued`. This defeats the work-once guarantee.

Currently, `Engine` takes `&mut self`, which prevents this at the Rust level for single-threaded use. However, the design spec envisions this as a daemon with IPC, and the `Storage` module exposes `pub` constructors -- so nothing prevents a future consumer from wrapping `Engine` in a `Mutex` or having multiple `Storage` instances against the same database.

**Recommendation:** Wrap the insert + dedup check + state transition in a single SQLite transaction. This is also important for crash safety -- if the process dies between the insert and the state update, you have an orphaned `Created` item that will never be processed.

```rust
// In storage.rs, add a transactional submit:
pub fn submit_with_dedup(&mut self, item: &WorkItem, dedup_key: Option<&str>) -> Result<Option<WorkId>> {
    let tx = self.conn.transaction()?;
    // INSERT the item
    // If dedup_key, SELECT matching active items (excluding this one)
    // If match found, UPDATE to merged; return canonical_id
    // Otherwise, UPDATE to queued
    tx.commit()?;
    // ...
}
```

### 2. `merge_work_item()` bypasses `State::can_transition_to()` validation

**File:** `src/storage.rs:237-261`

```rust
// Line 255-258: raw SQL UPDATE, no validation
self.conn.execute(
    "UPDATE work_items SET state = 'merged', merged_into = ?1, ...",
    ...
)?;
```

This directly writes `state = 'merged'` without calling `update_state()`, which means the `can_transition_to()` check is skipped. If `merge_work_item` is ever called on an item that is not in `Created` state, the state machine invariant is broken silently. The CLAUDE.md convention says "Storage enforces valid state transitions via `State::can_transition_to()`" -- this method violates that.

**Recommendation:** Call `self.update_state(id, State::Merged)?;` and then separately update `merged_into` and `completed_at`, or at minimum add an explicit state check inside `merge_work_item`.

### 3. `unwrap()` calls in library code parsing paths

**File:** `src/storage.rs`

Several `unwrap()` calls exist in the storage parsing layer that can panic on malformed database data:

- Line 130: `serde_json::to_string(&item.params).unwrap_or_default()` -- This is fine (serialization of valid JSON won't fail).
- Line 323: `row.get::<_, String>(0)?.parse().unwrap()` -- Panics if a work_id in the logs table is not a valid UUID.
- Line 357: `serde_json::to_string(&event.kind).unwrap_or_default()` -- Fine.
- Line 379: `serde_json::from_str(&kind_str).unwrap_or(EventKind::WorkCreated { ... })` -- The fallback fabricates a fake `WorkCreated` event with a random ID. This silently corrupts the event stream rather than surfacing the error.

**Recommendation:** Replace `unwrap()` with proper error propagation. For the event deserialization fallback, either propagate the error or use a dedicated `EventKind::Unknown { raw: String }` variant so consumers can handle it.

---

## Important (7)

### 4. `completed_at` is set for `Dead` and `Merged` states, not just `Completed`

**File:** `src/storage.rs:156-159`

```rust
let completed_at = if new_state.is_terminal() {
    Some(now.clone())
} else {
    None
};
```

`is_terminal()` returns true for `Completed`, `Dead`, and `Merged`. This means `completed_at` gets set when an item goes `Dead` (exhausted retries) or `Merged` (dedup). The field name `completed_at` semantically implies successful completion. This could confuse consumers who query `completed_at IS NOT NULL` expecting to find successfully finished work.

**Recommendation:** Either rename to `ended_at` / `resolved_at`, or only set it for `State::Completed`. The spec lifecycle diagram treats these as distinct terminal states with different semantics.

### 5. `claim()` loads all queued items to get the first one

**File:** `src/engine.rs:132-146`

```rust
pub fn claim(&mut self, worker_id: &str) -> Result<Option<WorkItem>> {
    let queued = self.storage.list_by_state(State::Queued)?;
    let Some(item) = queued.into_iter().next() else {
        return Ok(None);
    };
    // ...
}
```

`list_by_state(State::Queued)` fetches and deserializes every queued item, then takes only the first. With a large queue, this is wasteful.

**Recommendation:** Add a `claim_next()` method to `Storage` that uses `LIMIT 1` in the SQL query. The `idx_queued` partial index already exists for exactly this query.

```rust
// In storage.rs:
pub fn claim_next(&self) -> Result<Option<WorkItem>> {
    // SELECT * FROM work_items WHERE state = 'queued'
    // ORDER BY priority DESC, created_at ASC LIMIT 1
}
```

### 6. No transactions around multi-step operations in `engine.rs`

**File:** `src/engine.rs`

The `complete()`, `fail()`, `start()`, and `claim()` methods each perform multiple storage operations without a transaction. For example, `fail()` (lines 178-213) does:

1. `get_work_item(id)`
2. `update_state(id, State::Failed)`
3. `record_event(WorkFailed)`
4. `update_state(id, State::Dead)` or `update_state(id, State::Queued)`
5. `record_event(WorkDead)` or `record_event(WorkQueued)`

If the process crashes between steps 2 and 4, the item is stuck in `Failed` with no path forward -- it is not `Dead` and not `Queued`. The `Failed -> Failed` transition is not in `can_transition_to()`, so manual intervention is required.

**Recommendation:** Expose `Connection::transaction()` through `Storage` and wrap multi-step engine operations in transactions. This is especially important for `fail()` and `submit()`.

### 7. `event_seq` in `Storage` can diverge from the database

**File:** `src/storage.rs:342-361`

```rust
pub fn record_event(&mut self, kind: EventKind) -> Result<Event> {
    self.event_seq += 1;
    // ...
    self.conn.execute(
        "INSERT INTO events (seq, timestamp, kind) VALUES (?1, ?2, ?3)",
        params![event.seq as i64, ...],
    )?;
```

The `event_seq` field is incremented in memory before the INSERT. If the INSERT fails (e.g., disk full), the in-memory counter is already advanced but no row exists. Subsequent events will have a gap in their sequence numbers. Worse, if two `Storage` instances point at the same database file, their `event_seq` counters will collide.

**Recommendation:** Use `AUTOINCREMENT` on the `seq` column (it already has it as `INTEGER PRIMARY KEY AUTOINCREMENT`) and let SQLite assign the sequence. Read back the assigned value with `last_insert_rowid()`. Remove the in-memory `event_seq` field entirely.

### 8. `outcome_ms` cast from `u64` to `i64` can overflow

**File:** `src/storage.rs:289`

```rust
outcome.duration_ms as i64,
```

If `duration_ms` exceeds `i64::MAX` (unlikely but possible for extremely long-running work), this silently wraps to a negative value. Since this is a library, defensive coding matters.

**Recommendation:** Use `i64::try_from(outcome.duration_ms).unwrap_or(i64::MAX)` or store as `u64` text.

### 9. `Storage` fields and methods are `pub` -- breaks encapsulation

**File:** `src/storage.rs`

The `Storage` struct and all its methods are `pub`. The CLAUDE.md says "All state transitions go through Engine -- never mutate storage directly from outside." But because `storage` is a public module with public types and methods, any consumer can do:

```rust
use workq::storage::Storage;
let mut s = Storage::in_memory()?;
s.update_state(id, State::Completed)?; // bypasses Engine
```

**Recommendation:** Make `Storage` `pub(crate)` and its methods `pub(crate)`. Only `Engine` should be the public API surface.

### 10. `row_to_work_item` uses positional column indexes -- fragile

**File:** `src/storage.rs:398-437`

```rust
fn row_to_work_item(row: &rusqlite::Row) -> std::result::Result<WorkItem, String> {
    let id_str: String = row.get(0).map_err(|e| e.to_string())?;
    let params_str: String = row.get(5).map_err(|e| e.to_string())?;
    let state_str: String = row.get(7).map_err(|e| e.to_string())?;
    // ...
    let created_str: String = row.get(15).map_err(|e| e.to_string())?;
```

This uses `SELECT *` combined with hardcoded column indexes (0, 5, 7, 15, 16, 17). If anyone adds or reorders a column in the schema, every index silently shifts and produces incorrect data or panics. This is the most dangerous pattern in SQL row mapping.

**Recommendation:** Either (a) use `SELECT id, work_type, dedup_key, ...` with explicit columns and keep the positional indexes matching, or (b) use named column access: `row.get::<_, String>("id")`. Option (b) is rusqlite-idiomatic and self-documenting.

---

## Minor (4)

### 11. `tempfile` dev-dependency is unused

**File:** `Cargo.toml:17`

```toml
[dev-dependencies]
tempfile = "3"
```

The tests use `Engine::in_memory()` exclusively. The `tempfile` crate is not referenced anywhere in the test files. It should be removed to keep dependencies minimal, or a test using file-backed storage should be added (which would also improve coverage of `Engine::open()`).

### 12. `Engine::open()` should accept `impl AsRef<Path>`, not `&str`

**File:** `src/engine.rs:42`

```rust
pub fn open(path: &str) -> Result<Self> {
```

Idiomatic Rust file-opening APIs accept `impl AsRef<Path>` to work with `&str`, `String`, `PathBuf`, and `&Path`. This is a minor ergonomics point.

### 13. `NewWorkItem` has all `pub` fields -- builder pattern partially undermined

**File:** `src/model.rs:217-225`

The builder pattern (fluent `.dedup_key().priority()` API) is good, but all fields are `pub`, meaning callers can construct `NewWorkItem` directly without the builder. This is fine for now since there are no invariants to enforce on `NewWorkItem`, but if you later add validation (e.g., work_type must be non-empty), the pub fields provide a bypass.

**Recommendation:** Consider making fields `pub(crate)` and adding getters if external consumers need read access.

### 14. Edition 2024 limits compatibility to Rust 1.85+

**File:** `Cargo.toml:4`

Rust edition 2024 is very recent. This is fine if you control all consumers and their toolchains, but it limits compatibility with older Rust versions. Just be aware that users of this library must have Rust 1.85+.
