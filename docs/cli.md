---
status: active
milestone: kernel
spec: null
code: src/bin/animus.rs
---

# CLI Design

*The operator interface to the animus appliance.*

## Principle

The CLI is how you interact with a running animus instance. It connects to the same Postgres database as the control plane daemon and operates on the same tables. No separate API server — the CLI talks directly to the database.

This means the CLI works whether the daemon is running or not. You can submit work, inspect state, and read the ledger even when the control plane is down. The database is the source of truth, not the daemon.

## Commands

### `animus serve`

Run the control plane daemon. This is the existing behavior — watches queues, routes work to faculties, spawns foci.

```
animus serve [--faculties DIR] [--max-concurrent N]
```

| Flag | Default | Description |
|---|---|---|
| `--faculties` | `./faculties` | Directory containing faculty TOML configs |
| `--max-concurrent` | `4` | Global maximum concurrent foci |

### `animus work submit`

Submit a work item to the queue.

```
animus work submit <work_type> <source> [OPTIONS]
```

| Argument / Flag | Required | Description |
|---|---|---|
| `<work_type>` | yes | The work type (determines faculty routing) |
| `<source>` | yes | Provenance source (e.g., "bootstrap", "heartbeat", "user") |
| `--dedup-key` | no | Structural dedup key |
| `--trigger` | no | Provenance trigger info |
| `--params` | no | JSON object with work parameters |
| `--priority` | no | Priority (default: 0, higher = more urgent) |

```sh
# Submit the first bootstrap work item
animus work submit implement bootstrap \
  --dedup-key "milestone=M4-work-ledger" \
  --trigger "PLAN.md" \
  --priority 10 \
  --params '{"milestone": "M4", "title": "Work Ledger", "spec": "docs/ledger.md"}'
```

Output: the work item ID and whether it was created or merged.

### `animus work list`

List work items.

```
animus work list [OPTIONS]
```

| Flag | Default | Description |
|---|---|---|
| `--state` | all | Filter by state (queued, running, completed, failed, dead, merged) |
| `--type` | all | Filter by work_type |
| `--limit` | 20 | Max items to show |
| `--parent` | none | Show children of a specific work item |

```sh
animus work list
animus work list --state queued
animus work list --type implement
```

Output: table with id (short), work_type, state, dedup_key, created_at.

### `animus work show`

Show full details of a work item.

```
animus work show <id>
```

Shows: all fields, provenance, outcome (if terminal), parent/child links, and ledger entries (once the ledger exists).

### `animus ledger show`

Show ledger entries for a work item.

```
animus ledger show <work_item_id> [OPTIONS]
```

| Flag | Default | Description |
|---|---|---|
| `--type` | all | Filter by entry type (plan, finding, decision, step, error, note) |
| `--last` | all | Show only the last N entries |
| `--formatted` | false | Grouped by type (the compaction format) |

```sh
animus ledger show abc123
animus ledger show abc123 --type finding
animus ledger show abc123 --formatted
```

*Available after M4 (work ledger) is implemented.*

### `animus ledger append`

Manually append a ledger entry. Useful during bootstrap when the engage loop doesn't exist yet.

```
animus ledger append <work_item_id> <entry_type> <content>
```

```sh
animus ledger append abc123 decision "Using reqwest instead of rig-core for LLM calls"
animus ledger append abc123 finding "SSE parser needs to handle partial JSON across chunks"
animus ledger append abc123 step "Implemented SseParser::feed(), unit tests green"
```

*Available after M4.*

### `animus faculty list`

List registered faculties and their accepted work types.

```
animus faculty list [--dir DIR]
```

```sh
$ animus faculty list
NAME        ACCEPTS                              CONCURRENT  ISOLATION
engineer    implement, fix, refactor, test       true        worktree
social      engage, respond, check-in            false       -
transform   transform                            false       -
```

### `animus status`

Show the appliance status: database connectivity, queue depth, active foci, registered faculties, unroutable work types.

```
animus status
```

```
Database:    connected (13 work items, 2 memories)
Queue:       2 messages (1 visible, 1 in-flight)
Faculties:   1 registered (transform)
Active foci: 0 / 4
Unroutable:  1 work type (implement — no faculty)
```

---

## Implementation

Uses `clap` with derive macros for the subcommand structure:

```rust
#[derive(Parser)]
#[command(name = "animus", about = "Substrate for relational beings")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the control plane daemon
    Serve {
        #[arg(long, default_value = "./faculties")]
        faculties: PathBuf,
        #[arg(long, default_value_t = 4)]
        max_concurrent: usize,
    },
    /// Work item operations
    Work {
        #[command(subcommand)]
        action: WorkAction,
    },
    /// Ledger operations
    Ledger {
        #[command(subcommand)]
        action: LedgerAction,
    },
    /// List registered faculties
    Faculty {
        #[command(subcommand)]
        action: FacultyAction,
    },
    /// Show appliance status
    Status,
}

#[derive(Subcommand)]
enum WorkAction {
    Submit { ... },
    List { ... },
    Show { id: String },
}

#[derive(Subcommand)]
enum LedgerAction {
    Show { work_item_id: String, ... },
    Append { work_item_id: String, entry_type: String, content: String },
}
```

Every command connects to Postgres via `DATABASE_URL`, runs migrations, and operates directly on the tables. No daemon needed for CLI commands (except `serve`).

---

## What to Build Now

For the immediate bootstrap, we need three commands:

1. **`animus work submit`** — so we can submit our first real work item
2. **`animus work list`** — so we can see what's in the queue
3. **`animus serve`** — already exists, just needs to become a subcommand

Everything else (`work show`, `ledger *`, `faculty list`, `status`) can be added as we need it. The CLI grows with the system.

---

## Dependencies

```toml
clap = { version = "4", features = ["derive"] }
```
