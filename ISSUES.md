# Code Review Issues

Overall assessment: **strong initial implementation**. Clean architecture, correct state machine, sound dedup, good test coverage.

---

## Parallelization Guide

The remaining issues fall into independent groups that can be worked on simultaneously without merge conflicts.

**Group A** — Standalone, zero conflict with anything (model.rs, Cargo.toml, engine.rs signature):
- #11, #12, #13 — all touch different files with no overlap

**Group B** — Storage parsing/safety (localized to distinct functions in storage.rs):
- #3 (unwrap in get_logs, get_events_since)
- #8 (outcome_ms overflow in set_outcome)
- #10 (positional indexes in row_to_work_item + queries)

These touch different functions but share storage.rs, so merge conflicts are possible but minor.

**Group C** — Storage architecture (broader structural changes to storage.rs):
- #7 (event_seq → AUTOINCREMENT) — changes Storage struct, record_event_on, TxContext
- #9 (pub → pub(crate)) — sweeping visibility change, do last or on its own branch

**Group D** — Engine operations (engine.rs + new storage methods):
- #4 (completed_at rename) — schema + update_state_on
- #5 (claim LIMIT 1) — new storage method + engine change
- #6 (remaining transactions) — wraps fail/complete/start/claim with with_transaction

**Dependencies:**
- #7 should land before #6 (event_seq fix affects TxContext, which #6 relies on heavily)
- #9 should land last (touches every method signature, high conflict surface)
- Groups A, B, and D can all run in parallel
- Within Group D, #5 and #6 both touch claim() — do #5 first, then #6

---

## Critical (3)

### ~~1. Submit is not transactional~~ ✓ FIXED

**Fixed in:** `witt3rd/fix-submit-txn` branch

`Engine::submit()` now runs the entire flow (insert + dedup check + merge-or-queue + event recording) within a single SQLite transaction via `Storage::with_transaction()` and a `TxContext` helper. Crash safety and concurrent-access correctness are both addressed. The `TxContext` pattern is reusable for wrapping other multi-step engine operations (see Issue #6).

### ~~2. `merge_work_item()` bypasses `State::can_transition_to()` validation~~ ✓ FIXED

**Fixed in:** `witt3rd/fix-submit-txn` branch

`merge_work_item_on()` now validates `can_transition_to(State::Merged)` before writing and uses `State::Merged.to_string()` instead of a hardcoded string literal.

### ~~3. `unwrap()` calls in library code parsing paths~~ ✓ FIXED

**Fixed in:** `witt3rd/fix-unwrap-parsing` branch

The UUID `.parse().unwrap()` in `get_logs()` now uses `.map_err()` to propagate the error as `rusqlite::Error::FromSqlConversionFailure`. The event deserialization fallback in `get_events_since()` now uses `EventKind::Unknown { raw: String }` instead of fabricating a fake `WorkCreated` event, preserving the original string for consumers to handle.

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

**Partially addressed:** `submit()` is now transactional (see Issue #1 fix). The `TxContext` + `with_transaction` infrastructure is in place and reusable.

**Remaining:** `complete()`, `fail()`, `start()`, and `claim()` still perform multiple storage operations without a transaction. For example, `fail()` does:

1. `get_work_item(id)`
2. `update_state(id, State::Failed)`
3. `record_event(WorkFailed)`
4. `update_state(id, State::Dead)` or `update_state(id, State::Queued)`
5. `record_event(WorkDead)` or `record_event(WorkQueued)`

If the process crashes between steps 2 and 4, the item is stuck in `Failed` with no path forward -- it is not `Dead` and not `Queued`. The `Failed -> Failed` transition is not in `can_transition_to()`, so manual intervention is required.

**Recommendation:** Wrap these methods using the existing `storage.with_transaction()` pattern. This is especially important for `fail()`.

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
