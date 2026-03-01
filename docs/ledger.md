# Work Ledger Design

*Durable working memory for the agentic loop, managed at the database layer.*

## Context

Each focus (a single activation of a faculty on a work item) runs an agentic tool-use loop. The loop iterates: LLM call → tool use → tool result → LLM call → ... until the work is done or a halting condition is met.

The problem: context windows are finite, but work may be long. Every iteration adds an assistant response (reasoning + tool_use blocks) and tool results (potentially large — file contents, search results, command output). At ~4k tokens per round-trip, 50 iterations is 200k tokens. With a lifted upper bound on iterations, this fills even the largest context windows.

MicroClaw (see `docs/research/microclaw/agent.md`) solves this with LLM-based summarization of old messages. But that approach is designed for conversations — preserving social context across chat sessions. animus-rs needs something designed for work execution — preserving progress state within a single atomic task.

## The Insight

The agent loop's context has two kinds of information:

1. **Durable state** — the plan, key findings, decisions made, completed steps, errors encountered. This is what matters for coherence across a long execution.
2. **Transient detail** — raw tool outputs, intermediate reasoning, verbose file contents. Useful for the current step but not needed verbatim 20 iterations later.

A skilled human handles long tasks by keeping notes — a running document of what they've done, what they've learned, what's next. They don't try to hold every detail in their head. Their notes are the durable state; their short-term memory handles the current step.

The **work ledger** is this notebook. The agent maintains it during the loop using dedicated tools. The engine reads it for compaction. The consolidate hook reads it for post-processing. It persists in Postgres alongside the work item it belongs to.

## Why Postgres, Not the Filesystem

The focus directory (`/tmp/animus/foci/{focus-id}/`) is ephemeral scratch space — it exists for one focus execution and gets cleaned up. But the ledger is *the record of how work got done*. It has the same lifecycle as the work item:

- Created when work starts running
- Updated during the agentic loop
- Read by the consolidate hook after completion
- Queryable after completion (debugging, observability, auditing)
- Joins naturally to `work_items` by `work_item_id`

One `SELECT` gets you the full picture of what happened. No filesystem parsing, no cleanup race conditions, no lost state if the process crashes mid-focus.

## Schema

```sql
-- Migration: Add work ledger
CREATE TABLE work_ledger (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    work_item_id  UUID NOT NULL REFERENCES work_items(id),
    seq           INTEGER NOT NULL,
    entry_type    TEXT NOT NULL,
    content       TEXT NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),

    UNIQUE (work_item_id, seq)
);

CREATE INDEX idx_work_ledger_work_item ON work_ledger(work_item_id, seq);
```

Append-only within a focus execution. Each entry is typed. Sequence-ordered within its work item. The agent adds entries; the engine reads them for compaction. No updates, no deletes during execution — the ledger is a log.

### Entry Types

| Type | Purpose | Example |
|---|---|---|
| `plan` | Current plan or revision | "1. Read config 2. Validate schema 3. Fix timezone field" |
| `finding` | Something learned from a tool result | "Config uses TOML, not YAML. Timezone field is on line 47." |
| `decision` | A choice made with rationale | "Using edit_file instead of full rewrite — smaller diff, less risk" |
| `step` | Completed action with outcome | "Edited config.toml line 47: timezone = 'UTC' → 'America/New_York'" |
| `error` | Something that failed and why | "write_file failed: permission denied on /etc/config.toml" |
| `note` | Anything else worth remembering | "User prefers snake_case for all config keys" |

The type set is deliberately small and general. Faculties don't define custom types — the six types cover the universal structure of doing work. A social faculty and a computer-use faculty both make plans, discover findings, make decisions, complete steps, encounter errors, and take notes.

## Agent Tools

Two tools, deliberately simple. The agent interacts with its ledger through these — never through raw SQL or file operations.

### `ledger_append`

```json
{
  "name": "ledger_append",
  "description": "Record an entry in your work ledger. Use this to track your plan, findings, decisions, completed steps, and errors. Your ledger persists across context compactions — anything not in the ledger may be lost.",
  "input_schema": {
    "type": "object",
    "properties": {
      "entry_type": {
        "type": "string",
        "enum": ["plan", "finding", "decision", "step", "error", "note"]
      },
      "content": {
        "type": "string",
        "description": "What to record. Be concise but complete — this is your durable working memory."
      }
    },
    "required": ["entry_type", "content"]
  }
}
```

The engine assigns `work_item_id` (from the current focus) and `seq` (monotonic counter) automatically. The agent never sees or manages these.

### `ledger_read`

```json
{
  "name": "ledger_read",
  "description": "Read your work ledger. Returns all entries in order. Use this to review your progress, especially after context compaction.",
  "input_schema": {
    "type": "object",
    "properties": {
      "entry_type": {
        "type": "string",
        "enum": ["plan", "finding", "decision", "step", "error", "note"],
        "description": "Optional: filter to a specific entry type."
      },
      "last_n": {
        "type": "integer",
        "description": "Optional: return only the last N entries."
      }
    }
  }
}
```

Returns a clean, LLM-friendly format:

```
[1] plan: 1. Read config 2. Validate schema 3. Fix timezone field
[2] finding: Config uses TOML, not YAML. Timezone field is on line 47.
[3] step: Edited config.toml line 47: timezone = 'UTC' → 'America/New_York'
[4] decision: Skipping backup — file is version-controlled.
[5] error: clippy found unused import on line 3 — will fix in next step.
[6] step: Removed unused import. clippy clean.
```

## Context Management in the Engage Loop

> **See also:** `docs/engage.md` for the full engage phase architecture, including bounded sub-contexts, parallel tool execution, child work items, and the awareness digest.

Three layers of context management, from most granular to most aggressive. The primary mechanism is **bounded sub-contexts** — agent-declared scope based on `step` entries. The other layers serve as safety nets.

### Layer 1: Bounded Sub-Contexts (agent-declared, continuous)

The agent's work has natural structure: decide → call tools → reason about results → record progress → decide next. A `ledger_append` with `entry_type: "step"` is a **boundary marker** — it closes the current context block.

```
┌─ Block 1 (closed) ────────────────────┐
│  tool calls, reasoning, intermediate   │  → replaced with:
│  ledger_append(step: "Found config")   │    [completed step 1: Found config]
├─ Block 2 (closed) ────────────────────┤
│  tool calls, reasoning, intermediate   │  → replaced with:
│  ledger_append(step: "Fixed timezone") │    [completed step 2: Fixed timezone]
├─ Block 3 (open — current step) ───────┤
│  tool calls, reasoning in progress     │  → PRESERVED VERBATIM
│  (no step entry yet)                   │
└────────────────────────────────────────┘
```

**Closed blocks** are replaced with their ledger entry stub. **The open block** (current step) is always preserved in full. This is:

- **Semantic, not positional** — follows the structure of the work, not an arbitrary window
- **Agent-driven** — the agent controls when blocks close by writing `step` entries
- **Incremental** — no bulk compaction event; each `step` entry makes the preceding block eligible immediately
- **Lossless** — the agent wrote the summary itself; it captures exactly what it thought was important

### Layer 2: Ledger-Based Compaction (threshold-triggered safety net)

If bounded sub-contexts aren't enough (e.g., the open block itself is enormous, or the agent writes very few `step` entries), the engine falls back to threshold-based compaction:

When estimated token count exceeds the threshold (configurable, default: 70% of context window):

1. **Read the ledger** from Postgres: `SELECT entry_type, content FROM work_ledger WHERE work_item_id = $1 ORDER BY seq`

2. **Format as a structured context block:**
   ```
   === WORK LEDGER (your durable working memory) ===

   PLAN:
   - 1. Read config 2. Validate schema 3. Fix timezone field

   FINDINGS:
   - Config uses TOML, not YAML. Timezone field is on line 47.

   STEPS COMPLETED:
   - Edited config.toml line 47: timezone = 'UTC' → 'America/New_York'
   - Removed unused import. clippy clean.

   DECISIONS:
   - Skipping backup — file is version-controlled.

   ERRORS:
   - clippy found unused import on line 3 — will fix in next step.
   ```

3. **Replace old messages** with:
   ```
   [system prompt]           — unchanged
   [orient context]          — unchanged, always preserved
   [ledger context message]  — formatted from DB
   [last N messages]         — verbatim, recent working context
   ```

   Where N is configurable per-faculty (`compact_keep_recent`, default: 10 messages / 5 iterations).

No LLM summarization call needed. The agent has been summarizing incrementally all along.

### Layer 3: Emergency LLM Summarization (fallback)

If the agent hasn't maintained its ledger (no `ledger_append` calls, or very few relative to iteration count) and context pressure hits, the engine falls back to MicroClaw-style LLM summarization:

1. Split messages at `total - keep_recent`
2. Send older messages to the LLM: *"Summarize the progress of this work execution. Preserve: plan, key findings, decisions, completed steps, errors."*
3. Replace old messages with the summary + recent tail

This is the safety net, not the primary path. Faculties whose agents consistently trigger emergency summarization should tune their system prompts to reinforce ledger discipline.

## Engine Nudge

The engine tracks iterations without a `ledger_append` call. This is trivial — a counter incremented each iteration, reset on any `ledger_append` tool call. No ledger parsing needed.

When the counter exceeds a threshold:

```rust
if iterations_since_last_ledger_write >= faculty.engage.ledger_nudge_interval {
    messages.push(Message::system(
        "[engine] You have completed several iterations without updating your ledger. \
         Consider recording your progress — findings, completed steps, or plan updates. \
         Your ledger persists across context compactions; conversation history does not."
    ));
    iterations_since_last_ledger_write = 0;
}
```

Configurable per-faculty:

```toml
[faculty.engage]
ledger_nudge_interval = 5   # nudge every 5 iterations without a ledger write
                             # 0 = disabled
```

The nudge is a system message, not a tool call — it doesn't consume a tool-use turn. It's a gentle reminder, not a hard requirement.

## Faculty Configuration

The engage section of a faculty TOML gains ledger-related fields:

```toml
[faculty.engage]
model = "claude-sonnet-4-5-20250514"
system_prompt_file = "prompts/social.md"
tools = ["memory-search", "calendar", "send-message"]
max_turns = 50

# Context management
compact_threshold = 0.7         # compact when messages exceed 70% of context window
compact_keep_recent = 10        # keep last 10 messages verbatim after compaction
ledger_nudge_interval = 5       # nudge every 5 iterations without a ledger write (0 = off)
truncate_processed_results = true  # replace processed tool results with stubs
```

The `ledger_append` and `ledger_read` tools are always available in the engage phase — they don't need to be listed in `tools`. They're part of the engine, not the faculty's tool set.

## System Prompt Integration

The engage loop's system prompt template includes a section on ledger use:

```
## Working Memory

You have a work ledger — a durable record of your progress that persists even when
older conversation history is compacted to save context space.

Use `ledger_append` to record:
- Your plan (entry_type: "plan") — update when your approach changes
- Key findings (entry_type: "finding") — what you learn from tool results
- Decisions (entry_type: "decision") — choices made and why
- Completed steps (entry_type: "step") — what you did and the outcome
- Errors (entry_type: "error") — what failed and why

Use `ledger_read` to review your progress, especially if your context feels incomplete.

Be concise but complete. Your ledger is your lifeline for long tasks.
```

This is part of the engine's prompt template, injected before the faculty-specific system prompt. The faculty prompt can reinforce or extend these instructions but shouldn't need to repeat them.

## Observability

### Queryable Progress

The ledger is in Postgres. Standard queries:

```sql
-- What did this focus decide and why?
SELECT content FROM work_ledger
WHERE work_item_id = $1 AND entry_type = 'decision'
ORDER BY seq;

-- How many steps did it take?
SELECT count(*) FROM work_ledger
WHERE work_item_id = $1 AND entry_type = 'step';

-- What errors did it hit?
SELECT content FROM work_ledger
WHERE work_item_id = $1 AND entry_type = 'error'
ORDER BY seq;

-- Full ledger for a work item
SELECT seq, entry_type, content, created_at FROM work_ledger
WHERE work_item_id = $1
ORDER BY seq;
```

### OTel Integration

Each `ledger_append` is a tool call, which means it already gets an OTel span from the engage loop's tool execution tracing. Additional attributes on the span:

- `work.ledger.entry_type` — the entry type
- `work.ledger.seq` — the sequence number
- `work.ledger.work_item_id` — the work item

Compaction events get their own spans:

- `work.context.compaction` — with attributes for `trigger` (threshold/emergency), `messages_before`, `messages_after`, `ledger_entries_used`

### Metrics

| Metric | Type | Description |
|---|---|---|
| `work.ledger.entries` | Counter | Total ledger entries, by entry_type and faculty |
| `work.context.compactions` | Counter | Compaction events, by trigger type and faculty |
| `work.context.compaction.messages_removed` | Histogram | Messages removed per compaction |
| `work.context.emergency_summarizations` | Counter | Fallback LLM summarizations (indicates poor ledger discipline) |

A high `emergency_summarizations` rate for a faculty signals that its system prompt needs better ledger reinforcement.

## Downstream Consumers

### Consolidate Hook

The consolidate hook runs after the engage phase completes. Instead of parsing engage output, it queries the ledger:

```sql
-- Get all findings to store as memories
SELECT content FROM work_ledger
WHERE work_item_id = $1 AND entry_type = 'finding';

-- Get the final plan state (last plan entry)
SELECT content FROM work_ledger
WHERE work_item_id = $1 AND entry_type = 'plan'
ORDER BY seq DESC LIMIT 1;

-- Get the full execution narrative
SELECT entry_type, content FROM work_ledger
WHERE work_item_id = $1 ORDER BY seq;
```

The consolidate hook receives the `work_item_id` in its context — that's all it needs to access the ledger.

### Recover Hook

When a focus fails, the recover hook reads the ledger to understand where the work was and what was tried:

```sql
-- What was the last thing attempted?
SELECT entry_type, content FROM work_ledger
WHERE work_item_id = $1
ORDER BY seq DESC LIMIT 5;

-- Were there repeated errors?
SELECT content, count(*) FROM work_ledger
WHERE work_item_id = $1 AND entry_type = 'error'
GROUP BY content ORDER BY count(*) DESC;
```

This informs the retry decision: retry from scratch (clear ledger, start fresh) or retry with accumulated context (keep ledger, resume).

### The Awareness Digest (Cross-Faculty Coherence)

The ledger's value extends beyond individual foci. Ledger entries — especially `finding`, `plan`, and `error` — are the raw material for the **awareness digest**, an engine-level mechanism that gives each focus peripheral vision of what else is happening across the system.

At orient time, the engine queries recent ledger entries and work item states across all foci, assembling a digest that is injected into the new focus's context. A finding recorded by one focus becomes peripheral awareness for all other foci. This is how the being maintains coherence across concurrent and sequential operations.

The ledger is the nervous system of the being. Not just working memory for one task, but the substrate for integrated awareness.

See `docs/engage.md` § "The Awareness Digest" for the full design.

### Debugging and Auditing

After completion, the ledger is a complete, structured log of how work was done. Combined with OTel traces, it provides two complementary views:

- **OTel traces**: timing, token usage, tool call sequences, span hierarchy — the mechanical view
- **Work ledger**: plans, decisions, findings, errors — the cognitive view

## Rust Types

```rust
/// A single entry in the work ledger.
pub struct LedgerEntry {
    pub id: Uuid,
    pub work_item_id: Uuid,
    pub seq: i32,
    pub entry_type: LedgerEntryType,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

/// The six universal entry types.
pub enum LedgerEntryType {
    Plan,
    Finding,
    Decision,
    Step,
    Error,
    Note,
}

impl LedgerEntryType {
    pub fn as_str(&self) -> &'static str { ... }
    pub fn from_str(s: &str) -> Result<Self> { ... }
}
```

### Db API

```rust
impl Db {
    /// Append an entry to the work ledger. Assigns the next seq automatically.
    pub async fn ledger_append(
        &self,
        work_item_id: Uuid,
        entry_type: LedgerEntryType,
        content: &str,
    ) -> Result<LedgerEntry>;

    /// Read ledger entries for a work item, ordered by seq.
    pub async fn ledger_read(
        &self,
        work_item_id: Uuid,
        filter: Option<LedgerEntryType>,
        last_n: Option<i32>,
    ) -> Result<Vec<LedgerEntry>>;

    /// Read all ledger entries, formatted for injection into the LLM context.
    /// Groups entries by type for readability.
    pub async fn ledger_read_formatted(
        &self,
        work_item_id: Uuid,
    ) -> Result<String>;
}
```

`ledger_append` uses a subquery for seq assignment:

```sql
INSERT INTO work_ledger (work_item_id, seq, entry_type, content)
VALUES (
    $1,
    COALESCE((SELECT MAX(seq) FROM work_ledger WHERE work_item_id = $1), 0) + 1,
    $2,
    $3
)
RETURNING *;
```

## Open Questions

- **Ledger size limits**: Should there be a max number of entries per work item? A very long focus could produce hundreds of entries. Probably not a concern in practice — even 500 entries at ~100 bytes each is 50KB, trivial for Postgres. But worth monitoring.

- **Ledger retention**: After work items are completed and aged out, should their ledger entries be cleaned up? A retention policy on `work_items` should cascade to `work_ledger` via the foreign key (with `ON DELETE CASCADE`). Need to decide if this should be in the schema or handled by a separate cleanup process.

- **Plan versioning**: When the agent writes a new `plan` entry, it supersedes the previous one. Should `ledger_read_formatted` show only the latest plan, or all plan revisions? Probably just the latest for compaction context, but all revisions for debugging. The formatted view for compaction should use `DISTINCT ON` or `ORDER BY seq DESC LIMIT 1` for plan entries.

- **Structured content**: Should ledger entry content be plain text or support structured formats (JSON, markdown)? Plain text is simplest and LLM-friendly. JSON would enable programmatic queries on entry content. Leaning toward plain text with a convention that the agent can use markdown formatting within it.

- **Cross-focus ledger access**: When work is retried (Failed → Queued → new focus), should the new focus see the previous focus's ledger? Probably yes — the ledger entries remain linked to the work item, not the focus instance. A retried focus starts with the accumulated ledger from all prior attempts, giving it the benefit of what was already tried and learned.
