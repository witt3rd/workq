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

### ~~5. `claim()` loads all queued items to get the first one~~ ✓ FIXED

**Fixed in:** `witt3rd/claim-limit-one` branch

Added a `claim_next()` method to `Storage` that uses `SELECT ... WHERE state = 'queued' ORDER BY priority DESC, created_at ASC LIMIT 1`, hitting the existing `idx_queued` partial index. `Engine::claim()` now calls `claim_next()` instead of `list_by_state(State::Queued)`, avoiding loading and deserializing the entire queue.

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

### ~~7. `event_seq` in `Storage` can diverge from the database~~ ✓ FIXED

**Fixed in:** `witt3rd/update-issues-md` branch

Removed the in-memory `event_seq` field from `Storage` and `TxContext`. Event sequence numbers are now assigned by SQLite's `AUTOINCREMENT` and read back via `last_insert_rowid()`. The `with_transaction` snapshot/restore logic was also simplified since there's no in-memory counter to manage.

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

### ~~11. `tempfile` dev-dependency is unused~~ ✓ FIXED

**Fixed in:** `witt3rd/rm-tempfile-dep` branch

Removed the unused `tempfile` dev-dependency from `Cargo.toml`. Tests use `Engine::in_memory()` exclusively and never reference `tempfile`.

### ~~12. `Engine::open()` should accept `impl AsRef<Path>`, not `&str`~~ ✓ FIXED

**Fixed in:** `witt3rd/open-accept-path` branch

Both `Engine::open()` and `Storage::open()` now accept `impl AsRef<std::path::Path>` instead of `&str`, matching idiomatic Rust file-opening APIs. Callers can pass `&str`, `String`, `PathBuf`, or `&Path` without conversion.

### ~~13. `NewWorkItem` has all `pub` fields -- builder pattern partially undermined~~ ✓ FIXED

**Fixed in:** `witt3rd/encapsulate-new-work-item` branch

All `NewWorkItem` fields are now `pub(crate)`. The builder pattern is the only external construction path.

### ~~14. Edition 2024 limits compatibility to Rust 1.85+~~ ✗ WON'T FIX

All consumers use current toolchains. No compatibility concern.
