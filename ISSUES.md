# Remaining Issues

### 9. `Storage` fields and methods are `pub` -- breaks encapsulation

**File:** `src/storage.rs`

The `Storage` struct and all its methods are `pub`. The CLAUDE.md says "All state transitions go through Engine -- never mutate storage directly from outside." But because `storage` is a public module with public types and methods, any consumer can do:

```rust
use workq::storage::Storage;
let mut s = Storage::in_memory()?;
s.update_state(id, State::Completed)?; // bypasses Engine
```

**Recommendation:** Make `Storage` `pub(crate)` and its methods `pub(crate)`. Only `Engine` should be the public API surface.
