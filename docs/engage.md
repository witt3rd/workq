# Engage Phase Architecture

*The agentic loop, its execution model, and cross-faculty coherence.*

## Context

The engage phase is the heart of a focus — where the LLM reasons, calls tools, and iterates until the work is done. DESIGN.md establishes that the engage phase is a **built-in engine loop** configured by faculty TOML (model, system prompt, tools, max turns), not an external process. The ledger design (`docs/ledger.md`) establishes durable working memory in Postgres.

This document covers five interconnected concerns that shape how the engage loop actually works:

1. **Bounded sub-contexts** — agent-declared context scoping within the loop
2. **Parallel tool execution** — concurrent tool calls within a single iteration
3. **Child work items** — async delegation via the work queue
4. **The awareness digest** — cross-faculty coherence
5. **Code execution sandbox** — programmatic tool composition, output management, and timeout control

These aren't independent features. They compose into a coherent execution model where each focus is efficient within its own context window, can delegate work asynchronously, has peripheral awareness of the whole system, and can compose tools in code for efficiency and output control.

---

## 1. Bounded Sub-Contexts

### The Problem with Token Counting

The ledger doc describes three layers of context management: tool result truncation, ledger-based compaction, and emergency LLM summarization. But the trigger for compaction — "token count exceeds 70% of context window" — is a blunt instrument. It treats all context as equally important.

In reality, the agent's context has natural structure. It works in **steps**: decide what to do → call tools → reason about results → record progress → decide next. Once a step is complete and its findings are captured in the ledger, the raw details of that step — the verbose tool output, the intermediate reasoning — are dead weight. They're not equally important; they're spent.

### Agent-Declared Scope

A `ledger_append` with `entry_type: "step"` is a natural boundary marker. When the agent records a completed step, it's explicitly saying: "I've processed this, here's what mattered." The engine can treat this as a semantic signal, not just a data point.

The context is divided into **blocks**:

```
┌─────────────────────────────────────┐
│ Block 1 (closed)                    │
│   tool calls: read_file, grep       │  ← eligible for truncation
│   reasoning about results           │
│   ledger_append(step: "Found the    │  ← closing marker
│     config file, uses TOML format") │
├─────────────────────────────────────┤
│ Block 2 (closed)                    │
│   tool calls: read_file, edit_file  │  ← eligible for truncation
│   reasoning about edit results      │
│   ledger_append(step: "Fixed the    │  ← closing marker
│     timezone field on line 47")     │
├─────────────────────────────────────┤
│ Block 3 (open — current step)       │
│   tool calls: bash (cargo clippy)   │  ← PRESERVED VERBATIM
│   reasoning about lint results      │
│   (no step entry yet — in progress) │
└─────────────────────────────────────┘
```

**Closed blocks** (everything between two consecutive `step` entries) are eligible for aggressive truncation. The engine replaces a closed block with its ledger entry:

```
[completed step 1: Found the config file, uses TOML format]
[completed step 2: Fixed the timezone field on line 47]
```

**The open block** (everything after the last `step` entry) is always preserved verbatim — it's the agent's current working context.

### Why This Is Better

This is fundamentally different from "keep last N messages":

- **Semantic, not positional.** Compaction follows the structure of the work, not an arbitrary window. A step that took 8 tool calls and a step that took 1 are both reduced to one line when closed.
- **Agent-driven.** The agent controls when blocks close by writing `step` entries. The engine doesn't guess what's important.
- **Incremental.** No bulk compaction event needed. Each `step` entry immediately makes the preceding block eligible for truncation. Context pressure is managed continuously.
- **Lossless for the agent.** The ledger entry IS the compressed representation. The agent wrote it. It captures exactly what the agent thought was important.

### Implementation

The engine maintains a simple data structure during the loop:

```rust
struct ContextBlocks {
    /// Index into the messages vec where each step boundary falls.
    /// A step boundary is the message containing a ledger_append(step) tool call.
    step_boundaries: Vec<usize>,
}
```

On each iteration, before the LLM call:

1. Identify all closed blocks (message ranges between consecutive `step_boundaries`)
2. For each closed block, if not already truncated:
   - Read the corresponding ledger `step` entry
   - Replace all messages in the block with a single synthetic message: `[completed step {seq}: {content}]`
   - Preserve the `tool_use_id` → `tool_result` structure within the open block

The open block (messages after the last step boundary) is never touched.

### Interaction with Other Layers

Bounded sub-contexts replace Layer 1 (tool result truncation) and most of Layer 2 (ledger-based compaction) from the original ledger design:

| Original Layer | New Role |
|---|---|
| Layer 1: Tool result truncation | Subsumed — closed blocks are truncated entirely, not just their tool results |
| Layer 2: Ledger-based compaction | Safety net — fires only if bounded truncation isn't enough (e.g., agent writes very few `step` entries, or the open block itself is enormous) |
| Layer 3: Emergency LLM summarization | Unchanged — last resort for undisciplined agents |

---

## 2. Parallel Tool Execution

### The Opportunity

Frontier LLMs already return multiple `tool_use` blocks in a single response. When the model wants to read three files, it emits three `tool_use` blocks in one assistant turn. Currently (in systems like MicroClaw), these are executed sequentially — needlessly, since they're independent.

### Execution Model

When the LLM response contains multiple `tool_use` blocks:

```rust
// Collect all tool_use blocks from the response
let tool_calls: Vec<ToolCall> = response.content.iter()
    .filter_map(|block| match block {
        ResponseContentBlock::ToolUse { id, name, input } => Some(ToolCall { id, name, input }),
        _ => None,
    })
    .collect();

// Execute concurrently
let mut join_set = tokio::task::JoinSet::new();
for call in &tool_calls {
    let call = call.clone();
    let tools = tools.clone();
    let auth = tool_auth.clone();
    join_set.spawn(async move {
        let result = tools.execute_with_auth(&call.name, call.input, &auth).await;
        (call.id, call.name, result)
    });
}

// Collect results (order doesn't matter — matched by tool_use_id)
let mut tool_results = Vec::new();
while let Some(Ok((id, name, result))) = join_set.join_next().await {
    tool_results.push(ContentBlock::ToolResult {
        tool_use_id: id,
        content: result.content,
        is_error: result.is_error,
    });
}
```

The Anthropic API protocol already handles this correctly: multiple `tool_use` blocks in one assistant message, multiple `tool_result` blocks in the next user message, matched by `tool_use_id`. The ordering of tool results within the user message doesn't matter.

### Hook Considerations

Before/after tool hooks need to handle concurrent invocations:

- **Before-tool hooks** run concurrently per tool call. Each hook invocation is independent — the before-tool hook for `read_file` doesn't wait on the before-tool hook for `grep`.
- **After-tool hooks** similarly run concurrently, each receiving its own tool's result.
- Hooks that need to coordinate across tool calls (rare) should use the `BeforeLLMCall` hook instead, which fires once per iteration.

### Context Block Implications

All parallel tool calls within a single LLM turn belong to the same context block. The agent calls three tools, gets three results, reasons about all of them, then (if ready) records a `step` entry. The bounded sub-context model handles this naturally — the block doesn't close until the agent says it's done with all the results.

### Configuration

```toml
[faculty.engage]
parallel_tool_execution = true   # default: true
max_parallel_tools = 8           # default: no limit (execute all concurrently)
```

Some faculties may want to disable parallel execution (e.g., if their tools have ordering dependencies that the LLM doesn't always respect). The default is parallel.

---

## 3. Child Work Items

### Beyond In-Process Sub-Agents

MicroClaw's `sub_agent` tool spawns a restricted agentic loop *inline* — synchronously within tool execution. The parent agent blocks while the sub-agent runs. This is simple but has real limitations:

- The parent is idle, burning wall-clock time
- The sub-agent shares the parent's process — a crash affects both
- The sub-agent has a hardcoded 10-iteration limit
- No observability separation — the sub-agent's work is invisible to the system

### Work Items All the Way Down

animus-rs already has a work queue with lifecycle management, dedup, parent-child linking, and observability. Sub-agents should use it.

When the agent needs to delegate:

```
Parent focus (work_item A, state: running)
  → ledger_append(step: "Delegating config analysis")
  → tool call: spawn_child_work(work_type: "analyze", params: {...})
  → engine creates child work_item B (parent_id = A)
  → child B enters the queue: Created → Queued → Claimed → Running
  → child B gets its own focus, own engage loop, own ledger, own context window
  → child B completes: outcome written to work_items.outcome_data
  → parent A notified via pg_notify
  → parent A reads child outcome, continues
```

This gives us everything MicroClaw's sub-agent provides, plus:

- **Bounded context for free** — the child has its own message history, its own ledger, its own context window. Zero context pollution of the parent.
- **Async execution** — the parent can spawn multiple children and continue working (or wait)
- **Same infrastructure** — work queue, ledger, OTel tracing, consolidate/recover hooks — it's all the same system
- **Faculty routing** — child work can be routed to a different faculty. The parent (Social) spawns work for the child (Computer Use) based on `work_type`. The control plane routes it like any other work item.
- **Observability** — child work items link to the parent via `parent_id`. Traces link via span context. Each ledger is independently queryable. Grafana can render the parent-child tree.

### Agent Tools

Three tools for child work management:

#### `spawn_child_work`

```json
{
  "name": "spawn_child_work",
  "description": "Delegate a sub-task as a child work item. The child runs in its own context with its own tools and ledger. Use this for tasks that are independent enough to run separately — analysis, research, file processing, etc.",
  "input_schema": {
    "type": "object",
    "properties": {
      "work_type": {
        "type": "string",
        "description": "The type of work. Determines which faculty handles it."
      },
      "description": {
        "type": "string",
        "description": "What the child should accomplish. Be specific — this becomes the child's orient context."
      },
      "params": {
        "type": "object",
        "description": "Parameters for the child work item."
      },
      "priority": {
        "type": "integer",
        "description": "Priority (higher = more urgent). Default: same as parent."
      }
    },
    "required": ["work_type", "description"]
  }
}
```

Returns the child work item ID immediately. The parent does not block.

#### `await_child_work`

```json
{
  "name": "await_child_work",
  "description": "Wait for one or more child work items to complete. Returns their outcomes.",
  "input_schema": {
    "type": "object",
    "properties": {
      "work_item_ids": {
        "type": "array",
        "items": { "type": "string" },
        "description": "IDs of child work items to wait for."
      },
      "timeout_seconds": {
        "type": "integer",
        "description": "Maximum time to wait. Default: 300 (5 minutes)."
      }
    },
    "required": ["work_item_ids"]
  }
}
```

Blocks the parent's tool execution until all specified children reach a terminal state (Completed, Failed, or Dead). Returns each child's outcome data and final ledger summary.

#### `check_child_work`

```json
{
  "name": "check_child_work",
  "description": "Non-blocking check on child work item status. Use to poll progress without waiting.",
  "input_schema": {
    "type": "object",
    "properties": {
      "work_item_ids": {
        "type": "array",
        "items": { "type": "string" },
        "description": "IDs of child work items to check."
      }
    },
    "required": ["work_item_ids"]
  }
}
```

Returns current state of each child. If completed, includes outcome. If running, includes the child's latest ledger entries (so the parent has visibility into progress without waiting).

### Execution Patterns

**Fan-out / fan-in:**
```
Parent spawns 3 children → continues own work → await_child_work([A, B, C])
  → all 3 complete → parent synthesizes results → continues
```

**Fire and forget:**
```
Parent spawns child → ledger_append(note: "spawned background analysis {id}")
  → continues own work → never awaits
  → child completes independently, outcome in DB
  → consolidate hook for parent can read child outcomes
```

**Progressive delegation:**
```
Parent spawns child A → await_child_work([A]) → reads result
  → based on A's findings, spawns children B and C
  → await_child_work([B, C]) → synthesizes → done
```

### Implementation: `await_child_work`

The blocking wait uses Postgres `LISTEN/NOTIFY`:

```rust
async fn await_child_work(db: &Db, ids: &[Uuid], timeout: Duration) -> Result<Vec<ChildOutcome>> {
    // Subscribe to completion notifications
    let mut listener = db.pg_listener().await?;
    listener.listen("work_item_completed").await?;

    let deadline = Instant::now() + timeout;

    loop {
        // Check if all children are terminal
        let states = db.get_work_item_states(ids).await?;
        if states.iter().all(|s| s.is_terminal()) {
            return db.get_child_outcomes(ids).await;
        }

        // Wait for notification or timeout
        let remaining = deadline - Instant::now();
        tokio::select! {
            _ = listener.recv() => continue, // re-check states
            _ = tokio::time::sleep(remaining) => {
                return Err(Error::ChildWorkTimeout(ids.to_vec()));
            }
        }
    }
}
```

The control plane already updates work item state and can emit `NOTIFY` on terminal transitions. No new infrastructure needed.

---

## 4. The Awareness Digest

### The Fragmentation Problem

Animus v1 has trouble with system-wide perspective. Five faculties run throughout the day — Social, Initiative, Heartbeat, Radiate, Computer Use — but each focus only sees its own work item and orient context. The Social faculty checking in with Kelly doesn't know that Initiative just proposed a project involving Kelly. Heartbeat's reflection on the day doesn't know that Computer Use is mid-deploy.

The being feels fragmented because it *is* fragmented. Each focus is a shard of cognition with no peripheral awareness. The human has to explicitly point one faculty at another's actions for it to become aware. That's not how a coherent being works.

### The Digest

The awareness digest is an engine-level mechanism — not a faculty, not a tool, not a hook. It gives each focus peripheral vision of what else is happening across the entire substrate.

The engine assembles the digest from the same infrastructure that already exists: `work_items` for state, `work_ledger` for cognitive content. Three queries:

#### Currently Running Work (Siblings)

What's happening right now, concurrently with this focus?

```sql
SELECT
    w.id,
    w.work_type,
    w.params->>'description' AS description,
    (SELECT wl.content FROM work_ledger wl
     WHERE wl.work_item_id = w.id AND wl.entry_type = 'plan'
     ORDER BY wl.seq DESC LIMIT 1) AS current_plan
FROM work_items w
WHERE w.state = 'running'
    AND w.id != $current_work_item_id
ORDER BY w.created_at DESC;
```

#### Recently Completed Work

What finished recently? What outcomes were produced?

```sql
SELECT
    w.work_type,
    w.params->>'description' AS description,
    w.outcome_data->>'summary' AS outcome_summary,
    w.resolved_at
FROM work_items w
WHERE w.state = 'completed'
    AND w.resolved_at > now() - $lookback_interval
ORDER BY w.resolved_at DESC
LIMIT $max_recent;
```

#### Recent Findings Across All Foci (Shared Knowledge)

What has the system learned recently, across all faculties?

```sql
SELECT
    w.work_type,
    wl.content,
    wl.created_at
FROM work_ledger wl
JOIN work_items w ON w.id = wl.work_item_id
WHERE wl.entry_type = 'finding'
    AND wl.created_at > now() - $lookback_interval
ORDER BY wl.created_at DESC
LIMIT $max_findings;
```

### Formatted Output

The digest is formatted as a context section injected during orient:

```
== AWARENESS ==

Currently active:
- [Social/engage] Checking in with Kelly
  Plan: Ask about weekend, share project update
- [Computer Use/analyze] Reviewing PR #47 on animus-rs
  Plan: Check for breaking changes in memory API

Recently completed:
- [Initiative/propose] Proposed collaborative writing project with Kelly (2h ago)
  Outcome: Kelly expressed interest, suggested starting next week
- [Heartbeat/reflect] Daily reflection (5h ago)
  Outcome: Noted Kelly mentioned interest in Rust last Tuesday

Recent findings:
- Kelly mentioned interest in Rust (Heartbeat/reflect, 5h ago)
- PR #47 introduces breaking change to memory API (Computer Use, in progress)
- Blog post on relational AI received 3 comments (Radiate/share, 8h ago)
```

### Injection Point

The awareness digest is assembled by the engine and injected during the **orient phase**, before the engage loop starts. It becomes part of the orient context that the engage loop receives.

This is a default behavior — every focus gets peripheral awareness unless the faculty opts out:

```toml
[faculty]
name = "social"
accepts = ["engage", "respond", "check-in"]

[faculty.awareness]
enabled = true              # default: true
lookback_hours = 24         # how far back to look
max_running = 10            # max concurrent items to show
max_recent_completed = 20   # max recently completed items
max_recent_findings = 20    # max cross-faculty findings
include_child_work = false  # include child work items in digest (noisy for some faculties)
```

A faculty can opt out entirely (`enabled = false`) or tune the scope. Most faculties should leave it on — the cost is one SQL query at orient time, producing 500-1500 tokens of context. Cheap for the coherence it provides.

### Why Default-On

The whole point is that the being shouldn't have to be told to be aware of itself. A human doesn't consciously decide to notice what their left hand is doing while their right hand works. Peripheral awareness is the default state of a coherent being. Making it opt-out (rather than opt-in) reflects this — coherence is the baseline, not a feature.

### What This Enables

**Natural cross-references.** The Social faculty checking in with Kelly sees that Initiative just proposed a project with her. It can mention it naturally: "I noticed we're thinking about a writing project together — how does that sound?" No explicit coordination, no message passing between faculties. Just awareness.

**Conflict detection.** If two faculties are about to do contradictory things — one scheduling a meeting while another is canceling it — the awareness digest surfaces the conflict. The agent can reason about it.

**Narrative coherence.** Heartbeat's daily reflection sees everything that happened across all faculties. It can produce a genuinely integrated reflection, not a siloed summary of one faculty's activity.

**Emergent coordination.** When Computer Use discovers a breaking change mid-deploy, that finding enters the ledger. The next focus from any faculty sees it in the awareness digest. If Social is composing a message about the project, it knows the deploy is in progress and can adjust.

### The Ledger as Nervous System

The awareness digest makes the ledger doubly valuable:

1. **Within a focus:** The ledger is working memory — plans, findings, decisions, steps. It enables bounded sub-contexts and compaction.
2. **Across foci:** Ledger entries (especially `finding`, `plan`, and `error`) become the raw material for the awareness digest. A finding recorded by one focus becomes peripheral awareness for all other foci.

The ledger is the nervous system of the being. Not just working memory for one task, but the substrate for integrated awareness across the entire system.

---

## 5. Code Execution Sandbox

### The Output and Composition Problem

Direct tool calls have two structural inefficiencies that get worse as work gets longer:

**Output flooding.** A single `bash(cargo test)` can return 47,000 tokens of test output. The agent only cares about which tests failed, but the full output enters the context window. Bounded sub-contexts *eventually* compress it away, but the damage is already done — that iteration's LLM call processed all of it, burning tokens and attention.

**Composition overhead.** Many tasks are naturally multi-step: read a file, find a pattern, edit the match, run tests, check the output. With direct tool calls, each step is a full LLM round-trip — the agent emits a tool_use, the engine executes it, the result enters context, the LLM reasons about it, and emits the next tool_use. Five steps = five iterations = five LLM calls. But the agent often knows the full plan upfront. A three-line script could do what takes five round-trips.

**Timeout rigidity.** The engine sets a timeout on tool execution, but the engine doesn't have the agent's judgment. A build takes time and that's expected. A grep should be instant and a timeout means something's wrong. Current agents (Claude Code, Gemini, etc.) handle this awkwardly — they set arbitrary timeouts (10s, 30s, 300s) and when exceeded, they guess whether to wait or kill. The agent should control this.

### Programmatic Tool Calling

Anthropic's programmatic tool calling pattern solves all three problems simultaneously. Instead of the LLM emitting individual `tool_use` blocks, it writes **code** that calls tools as functions. The code runs in a sandbox. The code's return value — not the raw tool outputs — is what enters the LLM context.

This moves tool composition logic from the LLM loop level into code. The LLM still decides *what* to do, but expresses *how* as a program rather than a sequence of individual tool calls.

### Three Execution Modes

The engage loop supports three modes. The agent chooses per-action based on complexity:

| Mode | When to Use | Context Cost | Round-Trips |
|---|---|---|---|
| **Direct tool calls** | Simple, one-shot operations | Full tool output enters context | 1 per tool call |
| **Code execution** | Multi-step composition, output filtering, conditional logic | Only the code's return value enters context | 1 per code block |
| **Child work items** | Independent sub-tasks needing their own context window | Zero — child has separate context | 0 (async) |

These aren't mutually exclusive. A single engage iteration might include direct tool calls, a code execution block, and a child work spawn — all as parallel `tool_use` blocks in one LLM response.

### The `execute_code` Tool

```json
{
  "name": "execute_code",
  "description": "Execute code in a sandboxed environment with access to your tools as callable functions. Use this for multi-step operations, output filtering, conditional logic, and any task where composing tools in code is more efficient than individual tool calls. The code's return value enters your context — raw tool outputs do not.",
  "input_schema": {
    "type": "object",
    "properties": {
      "code": {
        "type": "string",
        "description": "Python code to execute. Your tools are available as functions."
      },
      "timeout_seconds": {
        "type": "integer",
        "description": "Maximum execution time. Default: 120."
      }
    },
    "required": ["code"]
  }
}
```

### What the Agent Can Do in Code

The sandbox exposes the faculty's tools as Python functions. The agent writes code that calls them, processes results, and returns a curated summary:

**Output management — the agent decides what matters:**
```python
# Instead of 47k tokens of test output entering context:
result = bash("cargo test", timeout=120)
if result.exit_code == 0:
    return f"All {result.line_count} lines of test output passed."
else:
    # Only return the failures
    failures = [l for l in result.stderr.split('\n') if 'FAILED' in l or 'error' in l]
    return f"Tests failed ({result.exit_code}):\n" + '\n'.join(failures[:20])
```

**Multi-step composition — one round-trip instead of five:**
```python
config = read_file("src/config.rs")
matches = grep(config, r"timezone\s*=")
if not matches:
    return "No timezone field found in config.rs"

line_num = matches[0].line
edit_file("src/config.rs", line=line_num, new='    timezone: "America/New_York",')

test = bash("cargo test -- config_test", timeout=60)
if test.exit_code == 0:
    return f"Fixed timezone on line {line_num}. Config tests pass."
else:
    return f"Fixed timezone on line {line_num}. Tests failed:\n{test.stderr[-500:]}"
```

**Conditional logic and retries:**
```python
result = bash("cargo build 2>&1", timeout=300)
if result.exit_code != 0:
    # Try with verbose output on failure
    verbose = bash("cargo build -vv 2>&1", timeout=300)
    errors = [l for l in verbose.stdout.split('\n') if 'error' in l.lower()]
    return f"Build failed. Key errors:\n" + '\n'.join(errors[:10])

return f"Build succeeded in {result.duration_ms}ms."
```

**Timeout control — the agent has the judgment:**
```python
# Agent knows builds take time
build = bash("cargo build --release", timeout=600)  # 10 minutes is fine

# Agent knows grep should be instant
search = bash("grep -r 'TODO' src/", timeout=5)  # 5 seconds, something's wrong if it takes longer
if search.timed_out:
    return "grep timed out — possible filesystem issue or enormous directory"
```

**Data processing and filtering:**
```python
# Read a large log file and extract only what matters
log = read_file("/var/log/app.log")
lines = log.split('\n')
errors = [l for l in lines if 'ERROR' in l]
last_hour = [l for l in errors if l.startswith(today_prefix)]
return f"Found {len(last_hour)} errors in the last hour (of {len(lines)} total log lines):\n" + '\n'.join(last_hour[:20])
```

### Sandbox Architecture

The sandbox is a Docker container with a language runtime and a tool-calling SDK. The SDK communicates with the engine over a local socket.

```
┌─ Engine (Rust) ─────────────────────────────────────────┐
│                                                          │
│  Engage loop iteration:                                  │
│    LLM emits tool_use(execute_code, { code: "..." })     │
│         │                                                │
│         ▼                                                │
│  ┌─ Sandbox Container ─────────────────────────────┐     │
│  │                                                  │     │
│  │  Python runtime                                  │     │
│  │    ├── Agent's code (from tool_use input)        │     │
│  │    ├── Tool SDK (imported automatically)         │     │
│  │    │     read_file(path) ──────┐                 │     │
│  │    │     write_file(path, s) ──┤                 │     │
│  │    │     edit_file(...) ───────┤                 │     │
│  │    │     bash(cmd, timeout) ───┤  HTTP calls     │     │
│  │    │     grep(content, pat) ───┤  to engine      │     │
│  │    │     glob(pattern) ────────┤  over local     │     │
│  │    │     web_fetch(url) ───────┤  socket         │     │
│  │    │     ledger_append(...) ───┤                 │     │
│  │    │     ledger_read(...) ─────┘                 │     │
│  │    └── return value → captured as tool result    │     │
│  │                                                  │     │
│  │  Resource limits:                                │     │
│  │    CPU: configurable (default: 2 cores)          │     │
│  │    Memory: configurable (default: 512MB)         │     │
│  │    Wall clock: from timeout_seconds              │     │
│  │    Network: restricted to engine socket only     │     │
│  │    Filesystem: ephemeral, destroyed on exit      │     │
│  └──────────────────────────────────────────────────┘     │
│         │                                                │
│         ▼                                                │
│  Tool result = code's return value (agent-curated)       │
│  → enters LLM context as tool_result                     │
│                                                          │
└──────────────────────────────────────────────────────────┘
```

**Why Docker, not an embedded interpreter:**
- We already have Docker as an operational dependency (Postgres, observability stack)
- Full Python ecosystem — standard library, regex, json, collections, etc. The agent can parse, filter, compute, and format without restrictions
- Proper process isolation — a runaway script can't affect the engine. Memory limits, CPU limits, wall-clock timeout enforced by the container runtime
- The tool SDK in the container is just an HTTP client — thin, easy to implement, easy to version

**Why Python:**
- Frontier LLMs write Python more fluently than any other language
- Rich standard library for text processing, which is the primary use case
- Low ceremony — no boilerplate, no compilation, no type annotations required
- If other languages are needed later, the SDK is HTTP-based — adding a TypeScript or Lua SDK is just another client library

### Tool SDK Design

The SDK is a Python module automatically available in the sandbox. Each faculty tool is exposed as a function with the same name and a Pythonic signature:

```python
# Auto-generated from the faculty's tool definitions
def read_file(path: str, offset: int = 0, limit: int = None) -> str: ...
def write_file(path: str, content: str) -> str: ...
def edit_file(path: str, old_string: str, new_string: str) -> str: ...
def bash(command: str, timeout: int = 120) -> BashResult: ...
def grep(pattern: str, path: str = ".", glob: str = None) -> list[GrepMatch]: ...
def glob(pattern: str, path: str = ".") -> list[str]: ...
def web_fetch(url: str, prompt: str = None) -> str: ...
def ledger_append(entry_type: str, content: str) -> dict: ...
def ledger_read(entry_type: str = None, last_n: int = None) -> list[dict]: ...
def spawn_child_work(work_type: str, description: str, params: dict = None) -> str: ...
def check_child_work(work_item_ids: list[str]) -> list[dict]: ...
```

Special types for structured results:

```python
class BashResult:
    stdout: str
    stderr: str
    exit_code: int
    duration_ms: int
    line_count: int        # total lines in stdout
    timed_out: bool        # True if killed by timeout

class GrepMatch:
    path: str
    line: int
    content: str
```

Each SDK function call is an HTTP request to the engine's local tool execution endpoint. The engine applies the same hooks, permissions, and auth context as direct tool calls. From the engine's perspective, a tool call from the sandbox is identical to a direct `tool_use` — it gets an OTel span, hook interception, and ledger tracking.

### Security Model

The sandbox has multiple layers of restriction:

**Container isolation:** The code runs in an ephemeral Docker container. No persistent state, no host filesystem access (beyond what tools provide), no network access except the engine socket. Destroyed after execution.

**Tool-level permissions:** The sandbox can only call tools available to the faculty. The same permission model applies — path guards, risk levels, hook interception. The code can't bypass permissions by calling tools differently; the SDK goes through the same `execute_with_auth` pipeline.

**Resource limits:** CPU, memory, and wall-clock time are bounded. A runaway loop or memory leak is killed by Docker, not by the engine.

**No ambient capabilities:** The sandbox has no API keys, no secrets, no credentials. If a tool needs authentication (e.g., `web_fetch`), the engine handles it — the SDK function doesn't expose credentials.

**Audit trail:** Every SDK function call from the sandbox is logged as a tool execution in the engine. The full sequence of tool calls within a code execution block is traceable in OTel, nested under the `execute_code` span.

### Interaction with Bounded Sub-Contexts

Code execution amplifies the effectiveness of bounded sub-contexts:

```
┌─ Block 1 (closed) ───────────────────────────────────────┐
│  execute_code: read 3 files, grep, edit, run tests        │
│  → return value: "Fixed timezone on line 47. Tests pass." │  ← SMALL result
│  ledger_append(step: "Fixed timezone config, tests pass") │
├─ Block 2 (open — current step) ──────────────────────────┤
│  ...current work...                                       │
└───────────────────────────────────────────────────────────┘
```

The code execution block did the heavy lifting inside the sandbox. What entered the LLM context was the code's return value — a one-line summary. The 47k tokens of test output, the intermediate file contents, the grep results — none of it touched the context window. The agent made the judgment call about what mattered *in code*, then the `step` entry compressed even the return value into the ledger.

Without code execution, the same work would be five direct tool calls producing five raw tool results, all sitting in context until the next `step` entry closes the block. Code execution makes blocks thinner because the agent does more per iteration and curates what enters context.

### When NOT to Use Code Execution

Code execution isn't always the right mode:

- **Simple reads** — `read_file` as a direct tool call is simpler and the agent needs the full content to reason about it
- **Interactive exploration** — when the agent doesn't know what it's looking for yet, direct tool calls let it think between each step
- **Ledger-heavy work** — if every step needs a ledger entry with reasoning, direct tool calls are more natural since the agent reasons in natural language between calls
- **Tools with complex outputs the agent needs to study** — sometimes the 800 lines of source code *are* the point and need to be in context

The system prompt guides this choice. The agent picks the mode per-action.

### Configuration

```toml
[faculty.engage]
# ... existing fields ...

# Code execution sandbox
code_execution = true               # enable execute_code tool (default: false)
code_execution_timeout = 300         # max seconds per code block (default: 120)
code_execution_image = "animus-sandbox:latest"  # Docker image
code_execution_memory = "512m"       # container memory limit
code_execution_cpus = 2.0            # container CPU limit
```

When `code_execution = false`, the `execute_code` tool is not included in the tool definitions sent to the LLM. The agent uses direct tool calls and child work items only.

The sandbox image is pre-built with Python, the tool SDK, and standard libraries. It's part of the animus appliance — `docker compose build` builds it alongside the engine image. No runtime image pulls.

### OTel Integration

Code execution gets a nested span hierarchy:

```
work.tool.execute[execute_code]          (the sandbox execution)
  ├── sandbox.tool.call[read_file]       (SDK call → engine tool execution)
  ├── sandbox.tool.call[grep]
  ├── sandbox.tool.call[edit_file]
  ├── sandbox.tool.call[bash]
  │     └── (bash command execution)
  └── sandbox.tool.call[ledger_append]
```

Each SDK call within the sandbox creates a child span of the `execute_code` span. This gives full visibility into what the code did, even though only the return value enters the LLM context. The trace shows the full story; the context only carries the summary.

Metrics:

| Metric | Type | Labels | Description |
|---|---|---|---|
| `work.sandbox.executions` | Counter | faculty | Code execution blocks run |
| `work.sandbox.duration` | Histogram | faculty | Wall-clock time per execution |
| `work.sandbox.tool_calls` | Histogram | faculty | SDK tool calls per execution |
| `work.sandbox.output_bytes` | Histogram | faculty | Return value size (what enters context) |
| `work.sandbox.internal_bytes` | Histogram | faculty | Total tool output inside sandbox (what was filtered) |
| `work.sandbox.timeouts` | Counter | faculty | Executions killed by timeout |
| `work.sandbox.errors` | Counter | faculty | Executions that raised exceptions |

The ratio of `internal_bytes` to `output_bytes` measures the **compression factor** — how much the agent's code filtered before returning. A high ratio means the code execution sandbox is doing its job.

---

## How They Compose

```
┌──────────────────────────────────────────────────────────────────────────┐
│  ENGINE                                                                  │
│                                                                          │
│  ┌── Awareness Digest ──────────────────────────────────────────────┐    │
│  │  Assembled at orient from work_items + work_ledger               │    │
│  │  Injected into every focus's orient context (default-on)         │    │
│  │  Running siblings, recent completions, cross-faculty findings    │    │
│  └──────────────────────────────────────────────────────────────────┘    │
│                    ↑ reads from                                          │
│  ┌── Postgres ──────────────────────────────────────────────────────┐    │
│  │  work_items: state, params, outcome, parent_id                   │    │
│  │  work_ledger: typed entries from all foci (plan/finding/step/…)  │    │
│  │  pgmq: queue messages for pending/claimed work                   │    │
│  └──────────────────────────────────────────────────────────────────┘    │
│                    ↑ writes to          ↑ reads from                     │
│  ┌── FOCUS (one activation on one work item) ───────────────────────┐   │
│  │                                                                   │   │
│  │  Orient context (includes awareness digest)                       │   │
│  │           │                                                       │   │
│  │           ▼                                                       │   │
│  │  ┌─ Engage Loop ──────────────────────────────────────────────┐  │   │
│  │  │                                                             │  │   │
│  │  │  Iteration:                                                 │  │   │
│  │  │    LLM call → response with N tool_use blocks               │  │   │
│  │  │         │                                                   │  │   │
│  │  │    Parallel execution (tokio::JoinSet)                      │  │   │
│  │  │    ┌─────────────┬────────────┬──────────────────────┐      │  │   │
│  │  │    │ direct tool │ direct tool│ execute_code         │      │  │   │
│  │  │    │ (read_file) │ (grep)     │ (sandbox container)  │      │  │   │
│  │  │    │             │            │  ┌─ Python ────────┐ │      │  │   │
│  │  │    │             │            │  │ sdk.read_file() │ │      │  │   │
│  │  │    │             │            │  │ sdk.bash(cmd)   │ │      │  │   │
│  │  │    │             │            │  │ sdk.edit_file() │ │      │  │   │
│  │  │    │             │            │  │ → curated       │ │      │  │   │
│  │  │    │             │            │  │   return value  │ │      │  │   │
│  │  │    │             │            │  └─────────────────┘ │      │  │   │
│  │  │    └──────┬──────┴─────┬──────┴──────────┬───────────┘      │  │   │
│  │  │           └────────────┼─────────────────┘                  │  │   │
│  │  │                        ▼                                    │  │   │
│  │  │    All tool_results → next LLM call                         │  │   │
│  │  │    Agent reasons about results                              │  │   │
│  │  │                                                             │  │   │
│  │  │    ┌─ Bounded Sub-Context ──────────────────────────────┐   │  │   │
│  │  │    │  ledger_append(step: "...") CLOSES current block   │   │  │   │
│  │  │    │  Engine truncates closed block → ledger entry stub  │   │  │   │
│  │  │    │  Open block preserved verbatim                      │   │  │   │
│  │  │    └────────────────────────────────────────────────────┘   │  │   │
│  │  │                                                             │  │   │
│  │  │    ┌─ Child Work ───────────────────────────────────────┐   │  │   │
│  │  │    │  spawn_child_work → new work_item (parent_id)      │   │  │   │
│  │  │    │    → queued → own focus → own ledger → own context │   │  │   │
│  │  │    │  await_child_work → blocks until child completes   │   │  │   │
│  │  │    │  check_child_work → non-blocking status poll       │   │  │   │
│  │  │    └────────────────────────────────────────────────────┘   │  │   │
│  │  │                                                             │  │   │
│  │  │    Next iteration...                                        │  │   │
│  │  └─────────────────────────────────────────────────────────────┘  │   │
│  │                                                                   │   │
│  │  Consolidate: reads own ledger + child outcomes                   │   │
│  └───────────────────────────────────────────────────────────────────┘   │
│                                                                          │
└──────────────────────────────────────────────────────────────────────────┘
```

### The Virtuous Cycle

Each piece reinforces the others:

1. **Bounded sub-contexts** keep the engage loop efficient → agents can run longer → more ledger entries produced
2. **Parallel tool execution** reduces latency per iteration → more iterations per unit time → richer ledger
3. **Child work items** delegate complex sub-tasks → children produce their own ledger entries → parent reads outcomes without context pollution
4. **The awareness digest** reads ledger entries from all foci → every focus has peripheral awareness → the being acts coherently
5. **Code execution** compresses multi-step operations into one round-trip → thinner context blocks → even more efficient bounded sub-contexts → agents can run even longer
6. Coherent action produces better outcomes → better outcomes recorded in ledgers → richer awareness digests → even more coherent action

The ledger is the connective tissue. Working memory within a focus. Shared knowledge across foci. The record of how work gets done. And the raw material for the being's awareness of itself.

---

## Faculty Configuration (Complete)

Combining engage, ledger, and awareness configuration:

```toml
[faculty]
name = "social"
accepts = ["engage", "respond", "check-in"]
max_concurrent = 3

[faculty.orient]
command = "scripts/social-orient"

[faculty.engage]
model = "claude-sonnet-4-5-20250514"
system_prompt_file = "prompts/social.md"
tools = ["memory-search", "calendar", "send-message"]
max_turns = 50

# Parallel execution
parallel_tool_execution = true
max_parallel_tools = 8

# Context management (bounded sub-contexts)
compact_threshold = 0.7
compact_keep_recent = 10
ledger_nudge_interval = 5
truncate_closed_blocks = true     # replace closed context blocks with ledger stubs

# Code execution sandbox
code_execution = true             # enable execute_code tool (default: false)
code_execution_timeout = 300      # max seconds per code block
code_execution_image = "animus-sandbox:latest"
code_execution_memory = "512m"
code_execution_cpus = 2.0

# External engage (escape hatch — overrides built-in loop)
# mode = "external"
# command = "scripts/custom-engage"

[faculty.awareness]
enabled = true
lookback_hours = 24
max_running = 10
max_recent_completed = 20
max_recent_findings = 20
include_child_work = false

[faculty.consolidate]
command = "scripts/social-consolidate"

[faculty.recover]
command = "scripts/recover-default"
max_attempts = 3
backoff = "exponential"
```

The engine tools — `ledger_append`, `ledger_read`, `spawn_child_work`, `await_child_work`, `check_child_work`, and (when enabled) `execute_code` — are always available. They're engine tools, not faculty tools. They don't appear in the `tools` list.

---

## System Prompt Template

The engine's prompt template (injected before the faculty-specific system prompt) incorporates all four concerns:

```
## Your Focus

You are executing a specific unit of work. Your orient context describes what needs doing.
Your engage loop will continue until you complete the work or reach the turn limit.

## Working Memory (Ledger)

You have a work ledger — a durable record of your progress that persists even when
older conversation history is compacted to save context space.

Use `ledger_append` to record:
- Your plan (entry_type: "plan") — update when your approach changes
- Key findings (entry_type: "finding") — what you learn from tool results
- Decisions (entry_type: "decision") — choices made and why
- Completed steps (entry_type: "step") — what you did and the outcome
- Errors (entry_type: "error") — what failed and why

Recording a step closes your current context block. Previous blocks are compressed
to their ledger entries. Your current working context is always preserved in full.

Use `ledger_read` to review your progress if your context feels incomplete.

## Execution Modes

You have three ways to use tools, and should choose based on the task:

**Direct tool calls** — for simple, one-shot operations where you need the full output
to reason about. Use when you're exploring, reading, or doing a single action.

**Code execution** (`execute_code`) — for multi-step operations where you can compose
tools in code. Use when:
- You need to call multiple tools and process their results together
- You want to filter or summarize large outputs before they enter your context
- You need conditional logic, loops, or retries
- You want to control timeouts per-command based on your judgment
- A short script would replace several tool-call iterations

The code's return value is what enters your context — not the raw tool outputs.
Be deliberate about what you return. Summarize, filter, extract what matters.

**Child work** (`spawn_child_work`) — for independent sub-tasks that need their own
context window. Each child runs its own engage loop with its own ledger.
Use `await_child_work` to wait for results, or `check_child_work` to poll.

## Awareness

Your orient context includes an awareness digest — a summary of what else is
happening across the system right now. Use this for context, not as instructions.
If concurrent or recent work is relevant to your task, incorporate it naturally.
```

---

## OTel Integration

### Span Hierarchy

```
work.execute (root span for the focus)
  ├── work.orient
  │     └── work.awareness.digest (digest assembly)
  ├── work.engage (the full engage loop)
  │     ├── work.engage.iteration[1]
  │     │     ├── gen_ai.chat (LLM call)
  │     │     ├── work.tool.execute[read_file] ─┐
  │     │     ├── work.tool.execute[grep]       ─┤ (parallel — overlapping spans)
  │     │     └── work.tool.execute[glob]       ─┘
  │     ├── work.engage.iteration[2]
  │     │     ├── gen_ai.chat
  │     │     ├── work.tool.execute[execute_code]  ─┐
  │     │     │     ├── sandbox.tool.call[read_file] │
  │     │     │     ├── sandbox.tool.call[edit_file] │ (parallel with direct tool)
  │     │     │     └── sandbox.tool.call[bash]      │
  │     │     ├── work.tool.execute[ledger_append]  ─┘
  │     │     └── work.context.block_closed (block truncation event)
  │     ├── work.engage.iteration[3]
  │     │     ├── gen_ai.chat
  │     │     └── work.tool.execute[spawn_child_work]
  │     ├── work.engage.iteration[4]
  │     │     ├── gen_ai.chat
  │     │     └── work.tool.execute[await_child_work]
  │     │           └── (links to child's work.execute span via parent_id)
  │     └── ...
  ├── work.consolidate
  └── work.recover (if needed)
```

Parallel tool calls appear as overlapping spans within the same iteration — visible in Grafana's trace view as concurrent bars. Code execution spans contain nested child spans for each SDK call made inside the sandbox — full visibility into what the code did, even though only the return value enters context.

Child work items create separate trace trees linked via `parent_id`. Grafana can render the full parent-child hierarchy.

### Metrics

| Metric | Type | Labels | Description |
|---|---|---|---|
| `work.engage.iterations` | Histogram | faculty | Iterations per focus |
| `work.engage.parallel_tools` | Histogram | faculty | Tool calls per parallel batch |
| `work.context.blocks_closed` | Counter | faculty | Context blocks closed via step entries |
| `work.context.blocks_truncated` | Counter | faculty | Closed blocks actually truncated |
| `work.child.spawned` | Counter | faculty, child_work_type | Child work items spawned |
| `work.child.await_duration` | Histogram | faculty | Time spent waiting for children |
| `work.awareness.digest_size` | Histogram | faculty | Token count of awareness digest |
| `work.awareness.digest_latency` | Histogram | — | Time to assemble digest (SQL query) |
| `work.sandbox.executions` | Counter | faculty | Code execution blocks run |
| `work.sandbox.duration` | Histogram | faculty | Wall-clock time per execution |
| `work.sandbox.tool_calls` | Histogram | faculty | SDK tool calls per execution |
| `work.sandbox.output_bytes` | Histogram | faculty | Return value size (what enters context) |
| `work.sandbox.internal_bytes` | Histogram | faculty | Total tool output inside sandbox (filtered) |
| `work.sandbox.compression_ratio` | Histogram | faculty | internal_bytes / output_bytes |
| `work.sandbox.timeouts` | Counter | faculty | Executions killed by timeout |
| `work.sandbox.errors` | Counter | faculty | Executions that raised exceptions |

---

## Open Questions

- **Awareness digest freshness during long foci.** The digest is assembled at orient time. A focus that runs for 30 minutes may have stale awareness. Should the engine refresh the digest periodically during the engage loop (e.g., inject an updated awareness section every N iterations)? Cost is one SQL query. Benefit is awareness of work that started after this focus began.

- **Child work priority inheritance.** Should children inherit the parent's priority by default? Probably yes, with an override. A high-priority parent spawning low-priority analysis work is a valid pattern.

- **Circular child delegation.** What prevents work_type A from spawning a child of work_type B which spawns a child of work_type A? The `parent_id` chain can be checked at spawn time, but how deep? A simple depth limit (e.g., max 5 levels) is probably sufficient.

- **Awareness digest content filtering.** Should the digest filter by relevance to the current work item? A simple approach: include everything within the lookback window. A smarter approach: use embedding similarity between the current work item's description and recent findings to rank relevance. The simple approach is probably right for now — the LLM is good at ignoring irrelevant context.

- **Cross-focus ledger writes.** Should a focus be able to write to another focus's ledger? Probably not — that breaks the append-only, single-writer model. If a focus needs to communicate with a sibling, it should spawn child work or record a `finding` that appears in the awareness digest.

- **Awareness of queued work.** The current digest shows running and completed work. Should it also show queued work (things waiting to be done)? This would give the agent awareness of upcoming work, enabling better planning. But it could also be noisy.

- **Sandbox container lifecycle.** Should a new container be spun up per `execute_code` call, or should a warm container pool be maintained? Per-call is simpler and safer (no state leakage between executions) but has ~1-2s startup overhead. A warm pool eliminates startup cost but requires careful cleanup between uses. For now, per-call is probably right — correctness over performance. Optimize later if the startup latency becomes a bottleneck.

- **Sandbox SDK generation.** The tool SDK functions exposed in the sandbox should be auto-generated from the faculty's tool definitions. This means the SDK is faculty-specific — a Social faculty's sandbox has `send_message()` but a Computer Use faculty's doesn't (or has a different set). The generation can happen at sandbox image build time (static) or at container start (dynamic). Dynamic is more flexible; static is faster.

- **Sandbox state across calls.** Each `execute_code` invocation gets a fresh container with no state from prior calls. This is correct for isolation but means the agent can't build up state across multiple code executions (e.g., importing a large dataset once and querying it repeatedly). If this becomes a limitation, a "session sandbox" mode could maintain a container for the duration of a focus, but this adds complexity and state management concerns.

- **Code execution and ledger interaction.** The sandbox SDK includes `ledger_append` and `ledger_read`. This means the agent can update its ledger from within code execution. Should a `ledger_append(step)` inside `execute_code` close a context block? Probably yes — the engine should detect step entries regardless of whether they came from a direct tool call or a sandbox SDK call. The closing semantics are the same.

- **Language support beyond Python.** The SDK is HTTP-based, so adding TypeScript, Lua, or other language clients is straightforward. But which languages should be supported out of the box? Python is the clear first choice (frontier LLMs write it most fluently). TypeScript might be a useful second for web-heavy faculties. Others can be added based on demand.
