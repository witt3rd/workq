# MicroClaw Agent Loop — Deep Research

> Source: `/Users/dothomps/src/ext/microclaw/` (v0.0.129, Rust 2021)
> Date: 2026-03-01

## Table of Contents

1. [Overview](#overview)
2. [Architecture & Crate Structure](#architecture--crate-structure)
3. [Entry Point & Startup](#entry-point--startup)
4. [Message Ingestion — Channel to Engine](#message-ingestion--channel-to-engine)
5. [Pre-Loop Setup](#pre-loop-setup)
6. [The Agentic Tool-Use Loop](#the-agentic-tool-use-loop)
7. [LLM Integration](#llm-integration)
8. [Tool System](#tool-system)
9. [Permission & Approval Model](#permission--approval-model)
10. [Hook System](#hook-system)
11. [Halting Conditions](#halting-conditions)
12. [State Management & Session Persistence](#state-management--session-persistence)
13. [Context Compaction](#context-compaction)
14. [Sub-Agent Spawning](#sub-agent-spawning)
15. [Telemetry & Observability](#telemetry--observability)
16. [Scheduler & Reflector](#scheduler--reflector)
17. [Key Types](#key-types)
18. [Comparison with animus-rs](#comparison-with-animus-rs)

---

## Overview

MicroClaw is a channel-agnostic agentic chat bot. A single shared agent loop (`process_with_agent` in `src/agent_engine.rs`) is reused across 14 channel adapters (Telegram, Discord, Slack, Feishu/Lark, Matrix, WhatsApp, iMessage, Email, Nostr, Signal, DingTalk, QQ, IRC, Web), a scheduler, and recursive sub-agents. The architecture is:

```
Platform message
  → Store in SQLite
  → Determine if response needed (private=always, group=@mention)
  → Typing indicator
  → Load/build session
  → Build system prompt
  → Compact if needed
  → Agentic loop (LLM call → tool_use? → execute tools → loop)
  → Strip images → Save session
  → Abort typing → Send response → Store bot response
```

Postgres-free. SQLite-only (`rusqlite` bundled). Fully async on Tokio. `reqwest` for HTTP to LLM APIs.

---

## Architecture & Crate Structure

7 workspace crates:

| Crate | Path | Role |
|---|---|---|
| `microclaw` | `.` (root) | Binary + wiring layer |
| `microclaw-core` | `crates/microclaw-core/` | Shared types, errors, LLM wire format |
| `microclaw-storage` | `crates/microclaw-storage/` | SQLite persistence (messages, sessions, memory, tasks, auth) |
| `microclaw-tools` | `crates/microclaw-tools/` | Tool trait, runtime primitives, sandbox, path guard |
| `microclaw-channels` | `crates/microclaw-channels/` | Channel adapter trait, registry, routing |
| `microclaw-app` | `crates/microclaw-app/` | Built-in skills, logging, transcription |
| `microclaw-clawhub` | `crates/microclaw-clawhub/` | Remote skill registry client (search/install/lockfile) |

Dependency graph:

```
microclaw (root binary)
  ├── microclaw-core          (no internal deps — foundation)
  ├── microclaw-storage       → microclaw-core
  ├── microclaw-tools         → microclaw-core
  ├── microclaw-channels      → microclaw-storage → microclaw-core
  ├── microclaw-app           (standalone)
  └── microclaw-clawhub       → microclaw-core
```

### Root Source File Map

| File | Purpose |
|---|---|
| `src/main.rs` | CLI entry: `start`, `setup`, `doctor`, `gateway`, `skill`, `hooks`, `web` |
| `src/runtime.rs` | `AppState` assembly, `run()`, channel startup wiring |
| `src/agent_engine.rs` | **The core agent loop** — `process_with_agent`, tool execution, compaction |
| `src/config.rs` | `Config` struct (YAML deserialization, directory helpers) |
| `src/llm.rs` | `LlmProvider` trait + Anthropic/OpenAI-compatible implementations |
| `src/memory.rs` | `MemoryManager` — file-based memory (`AGENTS.md`) |
| `src/memory_backend.rs` | `MemoryBackend` — unified structured + file memory interface |
| `src/scheduler.rs` | Scheduled task runner + memory reflector loops |
| `src/hooks.rs` | `HookManager` — hook discovery, execution, CLI |
| `src/skills.rs` | `SkillManager` — skill discovery/activation |
| `src/mcp.rs` | `McpManager` — MCP server/tool federation |
| `src/otlp.rs` | OTLP metrics exporter (HTTP/protobuf) |
| `src/embedding.rs` | `EmbeddingProvider` trait + provider factory |
| `src/plugins.rs` | Dynamic plugin tool definitions and execution |
| `src/tools/` | 21+ built-in tools |
| `src/channels/` | 14 platform adapters |

---

## Entry Point & Startup

**File:** `src/main.rs`

`#[tokio::main] async fn main()` parses CLI via `clap`. The `Start` subcommand calls `runtime::run(config, db, memory_manager, skill_manager, mcp_manager)`.

**File:** `src/runtime.rs`

`run()` is the supervisor. It:

1. Builds the LLM provider (`llm::create_provider(&config)`)
2. Builds the embedding provider (optional, for sqlite-vec)
3. Builds the channel registry — one `Arc<dyn ChannelAdapter>` per enabled platform
4. Builds the `ToolRegistry` — all built-in tools + MCP tools + plugin tools
5. Builds the `HookManager` — discovers hook scripts from `data_dir/hooks/` and `./hooks/`
6. Assembles a single `Arc<AppState>` — the shared read-only runtime state:

```rust
pub struct AppState {
    pub config: Config,
    pub channel_registry: Arc<ChannelRegistry>,
    pub db: Arc<Database>,
    pub memory: MemoryManager,
    pub skills: SkillManager,
    pub hooks: Arc<HookManager>,
    pub llm: Box<dyn LlmProvider>,
    pub llm_model_overrides: HashMap<String, String>,
    pub embedding: Option<Arc<dyn EmbeddingProvider>>,
    pub memory_backend: Arc<MemoryBackend>,
    pub tools: ToolRegistry,
}
```

7. Spawns one tokio task per enabled channel adapter (`spawn_channel_runtimes`)
8. Spawns the scheduler tick loop and reflector loop
9. Blocks on `tokio::signal::ctrl_c()` — main thread holds the process alive; all work in spawned tasks

---

## Message Ingestion — Channel to Engine

Each channel adapter receives a platform message, stores it in SQLite via `db.store_message(...)`, then calls into the shared engine:

```rust
process_with_agent_with_events(
    state,
    AgentRequestContext { caller_channel, chat_id, chat_type },
    override_prompt,  // None for user messages, Some(...) for scheduler
    image_data,
    event_tx,         // streaming events channel
)
```

**File:** `src/agent_engine.rs` (line ~106)

Before invoking the actual engine, this function:

1. **Registers the run** — `run_control::register_run(channel, chat_id, source_message_id)` assigns an atomic `run_id`, creates an `Arc<AtomicBool>` cancellation flag and `Arc<Notify>` abort notifier
2. **Races cancellation** — `tokio::select!` between the engine and cancellation. If a `/stop` command sets the flag and fires `Notify`, returns `"Current run aborted."` immediately
3. **Cleans up** — `run_control::unregister_run(...)` on any exit path

---

## Pre-Loop Setup

**File:** `src/agent_engine.rs`, `process_with_agent_impl` (lines ~469–717)

Before the loop starts, in sequence:

### Fast-Path Exit

If the user typed an explicit "remember X" memory command, `maybe_handle_explicit_memory_command` handles it with no LLM call and returns immediately.

### Session/Message Loading

Two paths:

**Path A — Session exists** (`db.load_session(chat_id)` succeeds):
- Deserialize full `Vec<Message>` from stored JSON (preserves tool_use/tool_result blocks from prior turns)
- Strip slash-command lines from user messages
- Fetch new user messages since session was last saved, append them

**Path B — No session** (first conversation or expired):
- Rebuild from raw DB message history (`load_messages_from_db`)
- Convert `StoredMessage` rows to alternating user/assistant `Message` structs
- User messages wrapped in XML: `<user_message sender="{name}">{content}</user_message>`
- Skip aborted messages, merge consecutive same-role messages

### System Prompt Assembly

Built by `build_system_prompt` (line ~1616). Five layers:

**Layer 1 — Identity/Soul:**
- If `SOUL.md` exists (searched: per-channel config, global config, `data_dir/SOUL.md`, `./SOUL.md`, per-chat `runtime/groups/{chat_id}/SOUL.md`), its content is wrapped in `<soul>...</soul>` XML tags
- Otherwise: generic `"You are {bot_username}, a helpful AI assistant..."`

**Layer 2 — Core capabilities and rules:**
- ~100-line hardcoded block with: tool capabilities, permission model, runtime time context (configured timezone, local/UTC), todo-list discipline, XML security note (user messages are untrusted)

**Layer 3 — Memories:**
- File-based memory (`MemoryManager.build_memory_context()` — reads `AGENTS.md`)
- Structured DB memory (ranked by KNN embedding similarity via sqlite-vec, or keyword relevance fallback)
- Token budget: default 1500 tokens (estimated as `content.len() / 4 + 10` per entry)

**Layer 4 — Skills catalog:**
- One-liner per available skill file

**Layer 5 — Plugin context injections:**
- Plugins can inject `# Plugin Prompt Context` or `# Plugin Documents` sections

### Image Handling

If image data was uploaded, the last user text message is converted from `MessageContent::Text` to `MessageContent::Blocks([Image(base64), Text])`.

### Approval Detection

Scans latest user message for approval keywords (`"approve"`, `"go ahead"`, `"proceed"`, `"批准"`, etc.) and denial keywords (`"don't"`, `"deny"`, `"cancel"`), setting `explicit_user_approval: bool` used later for high-risk tool gating.

---

## The Agentic Tool-Use Loop

**File:** `src/agent_engine.rs` (lines ~718–1232)

This is the core of the system. A `for` loop with configurable maximum iterations:

```rust
for iteration in 0..state.config.max_tool_iterations {  // default: 100
    // ...
}
```

### Each Iteration

#### Step 1 — BeforeLLMCall Hook

```rust
state.hooks.run_before_llm(chat_id, channel, iteration+1, system_prompt, message_count, tool_count)
```

- `HookOutcome::Block { reason }` → exit loop immediately, return reason as response
- `HookOutcome::Allow { patches }` → patches may replace the system prompt

#### Step 2 — LLM API Call

Two modes:

- **Non-streaming:** `llm.send_message_with_model(system_prompt, messages.clone(), Some(tool_defs), Some(model))`
- **Streaming** (when `event_tx` present): `llm.send_message_stream_with_model(...)` — spawns a forwarder task re-emitting each token delta as `AgentEvent::TextDelta { delta }`

Token usage (input + output) logged to SQLite as `"agent_loop"` entries after each call.

#### Step 3 — Inspect `stop_reason`

The response carries `stop_reason: Option<String>`. Three cases:

**Case A — `"end_turn"` or `"max_tokens"` (normal completion):**
1. Extract all `ResponseContentBlock::Text` blocks, join into single `text`
2. Strip `<think>...</think>` blocks if `config.show_thinking` is false
3. **Empty reply retry:** If `display_text.trim().is_empty()` and first attempt, push empty assistant message + synthetic user `[runtime_guard]` message, `continue` for one retry
4. Push assistant message onto `messages`
5. Persist session to SQLite (images stripped)
6. Append tool-failure summary footer if any tools failed
7. Emit `AgentEvent::FinalResponse { text }`
8. **Return** — loop done

**Case B — `"tool_use"` (continuation):**

For each `ResponseContentBlock::ToolUse { id, name, input }` in the response:

1. Push `Message { role: "assistant", content: Blocks(all_content_blocks) }` onto messages
2. For each tool call:
   - a. Run `hooks.run_before_tool(...)` — `Block` injects error tool result and skips execution; `Allow` patches may rewrite `effective_input`
   - b. Emit `AgentEvent::ToolStart { name, input }`
   - c. Call `tools.execute_with_auth(name, input, tool_auth)`
   - d. **Approval gate:** If `error_type == "approval_required"`, check config/user approval (see [Permission Model](#permission--approval-model))
   - e. If `activate_skill` succeeded, extract skill env file, update `tool_auth.env_files`
   - f. Run `hooks.run_after_tool(...)` — `Block` forces `result.is_error = true`
   - g. Track failures in `failed_tools` BTreeSet and `failed_tool_details`
   - h. Emit `AgentEvent::ToolResult { name, is_error, preview, duration_ms, ... }`
   - i. Collect `ContentBlock::ToolResult { tool_use_id, content, is_error }`
3. Push `Message { role: "user", content: Blocks(tool_results) }` onto messages
4. If `waiting_for_user_approval`: persist session, return confirmation prompt, exit loop
5. `continue` — next iteration

**Case C — Any other stop reason:**
Extract text, push assistant message, persist session, return text (or `"(no response)"` if empty).

### Visual Summary

```
┌─────────────────────────────────────────────────────────────┐
│  for iteration in 0..max_tool_iterations (default 100)      │
│                                                             │
│  ┌─── BeforeLLMCall hook ──────────────────────────────┐    │
│  │  Block? → return reason                             │    │
│  │  Allow? → maybe patch system_prompt                 │    │
│  └─────────────────────────────────────────────────────┘    │
│                         │                                   │
│  ┌─── LLM API call ───────────────────────────────────┐    │
│  │  send system_prompt + messages + tool_defs          │    │
│  │  stream tokens if event_tx present                  │    │
│  │  log token usage to SQLite                          │    │
│  └─────────────────────────────────────────────────────┘    │
│                         │                                   │
│  ┌─── Check stop_reason ──────────────────────────────┐    │
│  │                                                     │    │
│  │  "end_turn"/"max_tokens" ──→ extract text, RETURN   │    │
│  │                                                     │    │
│  │  "tool_use" ──→ for each tool call:                 │    │
│  │    ├── BeforeToolCall hook (block/patch)             │    │
│  │    ├── execute_with_auth(name, input, auth)         │    │
│  │    ├── approval gate if high-risk                   │    │
│  │    ├── AfterToolCall hook (block/patch result)      │    │
│  │    └── collect ToolResult                           │    │
│  │  push tool_results as user message                  │    │
│  │  CONTINUE loop ──→                                  │    │
│  │                                                     │    │
│  │  other ──→ extract text, RETURN                     │    │
│  └─────────────────────────────────────────────────────┘    │
│                                                             │
│  (post-loop) max iterations exhausted → synthetic message   │
└─────────────────────────────────────────────────────────────┘
```

---

## LLM Integration

**File:** `src/llm.rs`

### Provider Trait

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn send_message(&self, system: &str, messages: Vec<Message>,
                          tools: Option<Vec<ToolDefinition>>) -> Result<MessagesResponse>;
    async fn send_message_with_model(..., model_override: Option<&str>) -> ...;
    async fn send_message_stream(..., text_tx: Option<&UnboundedSender<String>>) -> ...;
    async fn send_message_stream_with_model(...) -> ...;
}
```

### Provider Selection

```rust
pub fn create_provider(config: &Config) -> Box<dyn LlmProvider> {
    match config.llm_provider.trim().to_lowercase().as_str() {
        "anthropic" => Box::new(AnthropicProvider::new(config)),
        _ => Box::new(OpenAiProvider::new(config)),
    }
}
```

Default is `"anthropic"`.

### AnthropicProvider

- Direct raw HTTP via `reqwest::Client`
- POST to `https://api.anthropic.com/v1/messages` (or configurable `llm_base_url`)
- Headers: `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`
- Retries up to 3x on HTTP 429 with exponential backoff (2^n seconds)
- Request body matches the Anthropic Messages API: `model`, `max_tokens`, `system`, `messages`, `tools`, `stream`

### OpenAiProvider

- Covers OpenAI, OpenRouter, DeepSeek, Groq, Ollama, and `openai-codex`
- POST to `{base}/chat/completions` (or `{base}/responses` for Codex)
- Auth: `Authorization: Bearer {api_key}`
- Special handling for DeepSeek: `enable_reasoning_content_bridge`, `enable_thinking_param`
- `openai_compat_body_overrides` / `_by_provider` / `_by_model` allow injecting arbitrary JSON keys into the request body

### Streaming

Both providers use an `SseEventParser` (lines 107–179 in `llm.rs`) that buffers partial lines and emits complete `data:` payloads.

**Anthropic streaming** processes event types:
- `content_block_start` — initialize text or tool_use slot by index
- `content_block_delta` / `text_delta` — append to text, send via `text_tx`
- `content_block_delta` / `input_json_delta` — append partial JSON to tool block
- `message_delta` — captures `stop_reason` and usage
- `message_start` — captures initial usage

**OpenAI streaming** processes:
- `choices[0].delta.content` — text deltas via `text_tx`
- `choices[0].delta.reasoning_content` — DeepSeek extended thinking
- `choices[0].delta.tool_calls[].function.arguments` — partial tool JSON

### Message Sanitization

`sanitize_messages` (lines 27–105 in `llm.rs`) removes orphaned `ToolResult` blocks (those whose `tool_use_id` doesn't match a pending `ToolUse` from the preceding assistant turn). Prevents API errors after compaction or history reconstruction.

---

## Tool System

### The Tool Trait

**File:** `crates/microclaw-tools/src/runtime.rs`

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn definition(&self) -> ToolDefinition;
    async fn execute(&self, input: serde_json::Value) -> ToolResult;
}
```

`ToolDefinition` (from `crates/microclaw-core/src/llm_types.rs`) is sent to the LLM:

```rust
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,  // JSON Schema
}
```

`ToolResult` carries the execution outcome:

```rust
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    pub status_code: Option<i32>,
    pub bytes: usize,
    pub duration_ms: Option<u128>,
    pub error_type: Option<String>,
    pub metadata: Option<serde_json::Value>,
}
```

### Tool Registry

**File:** `src/tools/mod.rs`

`ToolRegistry` holds `Vec<Box<dyn Tool>>`. All built-in tools registered eagerly at startup:

```rust
let mut tools: Vec<Box<dyn Tool>> = vec![
    Box::new(bash::BashTool::new_with_isolation(...)),
    Box::new(read_file::ReadFileTool::new_with_isolation(...)),
    Box::new(write_file::WriteFileTool::new_with_isolation(...)),
    Box::new(edit_file::EditFileTool::new_with_isolation(...)),
    Box::new(glob::GlobTool::new_with_isolation(...)),
    Box::new(grep::GrepTool::new_with_isolation(...)),
    Box::new(memory::ReadMemoryTool::new(...)),
    Box::new(memory::WriteMemoryTool::new(...)),
    Box::new(web_fetch::WebFetchTool::new(...)),
    Box::new(web_search::WebSearchTool::new(...)),
    Box::new(time_math::GetCurrentTimeTool::new(...)),
    Box::new(send_message::SendMessageTool::new(...)),
    Box::new(schedule::ScheduleTaskTool::new(...)),
    Box::new(sub_agent::SubAgentTool::new(...)),
    Box::new(activate_skill::ActivateSkillTool::new(...)),
    Box::new(todo::TodoReadTool::new(...)),
    Box::new(structured_memory::StructuredMemorySearchTool::new(...)),
    // ... more
];
```

Additional tool sources:
- **MCP tools:** Each connected MCP server's tools become `McpTool` instances, namespaced as `mcp_{server}_{tool}`
- **Plugin tools:** Loaded dynamically from YAML manifests, executed as subprocesses
- **ClawHub tools:** Optional `clawhub_search`, `clawhub_install`

### Built-in Tool Inventory

| Tool | Module | Risk |
|---|---|---|
| `bash` | `src/tools/bash.rs` | High |
| `read_file` | `src/tools/read_file.rs` | Low |
| `write_file` | `src/tools/write_file.rs` | Medium |
| `edit_file` | `src/tools/edit_file.rs` | Medium |
| `glob` | `src/tools/glob.rs` | Low |
| `grep` | `src/tools/grep.rs` | Low |
| `browser` | `src/tools/browser.rs` | Low |
| `read_memory` | `src/tools/memory.rs` | Low |
| `write_memory` | `src/tools/memory.rs` | Medium |
| `web_fetch` | `src/tools/web_fetch.rs` | Low |
| `web_search` | `src/tools/web_search.rs` | Low |
| `get_current_time`, `compare_time`, `calculate` | `src/tools/time_math.rs` | Low |
| `send_message` | `src/tools/send_message.rs` | Medium |
| `schedule_task`, `list_scheduled_tasks`, etc. | `src/tools/schedule.rs` | Medium |
| `export_chat` | `src/tools/export_chat.rs` | Low |
| `sub_agent` | `src/tools/sub_agent.rs` | Low |
| `activate_skill` | `src/tools/activate_skill.rs` | Low |
| `sync_skills` | `src/tools/sync_skills.rs` | Medium |
| `todo_read`, `todo_write` | `src/tools/todo.rs` | Low/Medium |
| `structured_memory_search/delete/update` | `src/tools/structured_memory.rs` | Low/Medium |

### Tool Execution Pipeline

**File:** `src/tools/mod.rs`, `execute_with_auth` (lines ~370–404)

```
execute_with_auth(name, input, auth)
  │
  ├── 1. Validate execution policy (sandbox vs host)
  │     → error_type: "execution_policy_blocked"
  │
  ├── 2. Check high-risk approval gate
  │     → error_type: "approval_required"
  │
  ├── 3. Inject default chat_id for memory/todo tools
  │
  ├── 4. Inject full auth context (__microclaw_auth key)
  │
  ├── 5. Execute tool (linear scan over tools vec, match by name)
  │     → fallback to dynamic plugin execution if "unknown_tool"
  │
  └── 6. Return ToolResult { content, is_error, error_type, ... }
```

### Error Types

| `error_type` | Meaning |
|---|---|
| `"tool_error"` | Generic tool failure |
| `"unknown_tool"` | Tool name not found |
| `"approval_required"` | High-risk tool needs explicit approval |
| `"execution_policy_blocked"` | Sandbox policy prevents execution |
| `"path_policy_blocked"` | Bash tried to use `/tmp/` absolute path |
| `"env_access_blocked"` | Bash tried to access `.env` while skill env active |
| `"timeout"` | Command/request exceeded timeout |
| `"spawn_error"` | Failed to spawn process |
| `"process_exit"` | Non-zero exit code |
| `"hook_blocked"` | After-tool hook blocked the result |
| `"mcp_error"` | MCP server error |
| `"mcp_rate_limited"` | MCP rate limited |
| `"mcp_circuit_open"` | MCP circuit breaker tripped |

All tool errors surface as `ToolResult { is_error: true }`. The LLM receives the full error content so it can reason about it and try a different approach. Failed tools are tracked across the turn; at the end, the final response is annotated with a failure summary footer.

---

## Permission & Approval Model

### Risk Classification

**File:** `crates/microclaw-tools/src/runtime.rs`

```rust
pub fn tool_risk(name: &str) -> ToolRisk {
    match name {
        "bash" => ToolRisk::High,
        "write_file" | "edit_file" | "write_memory" | "send_message"
        | "sync_skills" | "schedule_task" | ... => ToolRisk::Medium,
        _ => ToolRisk::Low,
    }
}
```

### High-Risk Gate (Bash)

Only `bash` is `High` risk. For callers via `web` channel or a `control_chat`, bash requires `__microclaw_high_risk_approved: true` in the input JSON. Without it:

1. Tool returns `error_type: "approval_required"` without executing
2. Agent loop checks `config.high_risk_tool_user_confirmation_required`:
   - If `false` (auto-approve): immediately retry with approval marker injected
   - If `true`: check `explicit_user_approval` (parsed from user's last message)
     - If user said "approve"/"go ahead"/etc: retry with marker
     - If not: set `waiting_for_user_approval = true`, persist session, return confirmation prompt

```rust
// Approval keyword detection
fn is_explicit_user_approval(text: &str) -> bool {
    // "approve", "go ahead", "proceed", "确认", "批准", etc.
    // Blocked by: "don't", "deny", "cancel", etc.
}
```

### Execution Policy

Separate from risk. Tools have execution policies:
- `HostOnly` — runs on host (most tools)
- `SandboxOnly` — requires Docker sandbox
- `Dual` — can run either way (`bash` only currently)

### Path Guard

**File:** `crates/microclaw-tools/src/path_guard.rs`

`read_file`, `write_file`, `edit_file`, `glob`, `grep` all call `check_path()` before acting. Blocks:
- `.ssh`, `.aws`, `.gnupg`, `.kube`, `.config/gcloud`
- `.env` files, SSH keys, `/etc/shadow`
- Symlinks in paths (rejected entirely)

### Chat Access Control

`ToolAuthContext` carries `caller_chat_id` and `control_chat_ids`. Memory, todo, and message tools call `authorize_chat_access()` to prevent cross-chat data access. Control chats bypass this restriction.

---

## Hook System

**File:** `src/hooks.rs`

### Three Hook Events

| Event | When | Can Modify |
|---|---|---|
| `BeforeLLMCall` | Before every LLM API call | `system_prompt` |
| `BeforeToolCall` | Before each tool execution | `tool_input` |
| `AfterToolCall` | After each tool returns | `content`, `is_error`, `error_type`, `status_code` |

### Hook Discovery

Scans `data_dir/hooks/` and `./hooks/` directories. Each hook is a directory with a `HOOK.md` file containing YAML frontmatter:

```yaml
name: block-bash
description: Example hook
events: [BeforeToolCall]
command: "sh hook.sh"     # runs in hook's directory
enabled: false
timeout_ms: 1000
priority: 50              # lower priority runs first
```

### Hook Execution

1. Spawns `sh -lc "{command}"` in the hook's directory
2. Writes JSON payload to stdin (varies by event type):
   - `BeforeLLMCall`: `{event, chat_id, caller_channel, iteration, system_prompt, messages_len, tools_len}`
   - `BeforeToolCall`: `{event, ..., tool_name, tool_input}`
   - `AfterToolCall`: `{event, ..., tool_name, tool_input, result: {content, is_error, ...}}`
3. Sets `MICROCLAW_HOOK_EVENT` and `MICROCLAW_HOOK_NAME` env vars
4. Timeout: `timeout_ms.clamp(10, 120_000)` milliseconds
5. Reads stdout as JSON

### Hook Outcomes

```rust
pub enum HookOutcome {
    Allow { patches: Vec<serde_json::Value> },
    Block { reason: String },
}
```

- `allow` — proceed; patches collected and applied
- `block` — abort with reason (for `BeforeLLMCall`/`BeforeToolCall`) or force error (for `AfterToolCall`)
- `modify` — collected as patches, applied after all hooks run

### Built-in Example Hooks

| Hook | Event | Default | Purpose |
|---|---|---|---|
| `block-bash` | BeforeToolCall | disabled | Blocks all `bash` tool calls |
| `block-global-memory` | BeforeToolCall | enabled | Blocks `read_memory`/`write_memory` with `scope=global` |
| `filter-global-structured-memory` | AfterToolCall | enabled | Strips `[global]` lines from `structured_memory_search` output |
| `redact-tool-output` | AfterToolCall | disabled | Replaces `read_file` content with `[redacted]` |

---

## Halting Conditions

There are exactly five ways the agent loop terminates:

| # | Condition | Location | Behavior |
|---|---|---|---|
| 1 | `stop_reason == "end_turn"` or `"max_tokens"` | Loop body | Normal: persist session, return final text |
| 2 | Max iterations exhausted (default 100) | Post-loop | Synthetic `"I reached the maximum number of tool iterations..."`, persist session |
| 3 | `BeforeLLMCall` hook blocks | Loop body | Returns hook reason immediately, no LLM call |
| 4 | Cancellation signal (`/stop` command) | `process_with_agent_with_events` | `tokio::select!` cancel branch wins, returns `"Current run aborted."` |
| 5 | High-risk tool awaiting approval | Loop body | Exits mid-loop, saves session, returns confirmation prompt; resumes on next user message |

---

## State Management & Session Persistence

MicroClaw has **no persistent in-memory conversation state** between requests. Everything is serialized to SQLite.

### Session Persistence

Key function: `persist_session_with_skill_env_files` (line ~276)

1. `strip_images_for_session` — removes base64 data from image blocks (replaces with `[image was sent]`)
2. Serializes full `Vec<Message>` as JSON into `db.save_session_with_meta(chat_id, json, ...)`
3. Session saved on: normal end_turn, max iterations, unknown stop reason, approval-waiting

### Session Reload

On next request for the same `chat_id`:
1. Load session JSON from SQLite
2. Deserialize `Vec<Message>` (including all tool_use/tool_result blocks)
3. Append any new user messages since last save
4. Continue from where it left off

### Per-Request In-Memory State

| State | Type | Purpose |
|---|---|---|
| `messages` | `Vec<Message>` | Accumulating conversation (grows each iteration) |
| `failed_tools` | `BTreeSet<String>` | Tool names that errored |
| `failed_tool_details` | `Vec<String>` | Formatted error strings for footer |
| `skill_env_files` | `Vec<String>` | `.env` paths for activated skills |
| `tool_auth` | `ToolAuthContext` | Channel, chat_id, control_chat_ids, env_files |
| `effective_model` | `String` | Per-channel model override or config default |
| `empty_visible_reply_retry_attempted` | `bool` | Ensures empty-reply retry fires at most once |
| `explicit_user_approval` | `bool` | Computed once from latest user message |

---

## Context Compaction

**Trigger:** `messages.len() > config.max_session_messages` (default 40)

**File:** `src/agent_engine.rs`, `compact_messages` (lines ~1968–2103)

### Algorithm

1. **Split** at `total - compact_keep_recent` (default: keep last 20 verbatim)
2. **Archive** the full conversation to a markdown file
3. **Build summary input** from old messages: `[{role}]: {text}` joined by `\n\n`
4. **Truncate** summary input to 20,000 chars at a char boundary
5. **LLM call** with system `"You are a helpful summarizer."` and prompt:
   ```
   Summarize the following conversation concisely, preserving key facts,
   decisions, tool results, and context needed to continue the conversation.
   Be brief but thorough.
   ```
6. **On timeout** (`compaction_timeout_secs`, default 180s) or error: fallback to keeping just the recent messages
7. **Assemble** compact output:
   - Synthetic `[user]: [Conversation Summary]\n{summary}`
   - Synthetic `[assistant]: Understood, I have the conversation context.`
   - Recent messages appended after
   - Role alternation enforced

### Invariants

After compaction or history loading, these invariants are enforced:
- Must start with a `user` message (leading assistant messages dropped)
- Must end with a `user` message (trailing assistant messages dropped)
- No consecutive same-role messages (merged by appending with `\n`)

---

## Sub-Agent Spawning

**File:** `src/tools/sub_agent.rs`

The `sub_agent` tool is available to the main agent. When invoked, it spins up a **completely separate** agentic loop **inline** (synchronous within tool execution, not a new tokio task):

- Fresh LLM provider instance
- **Restricted `ToolRegistry`** — excludes `sub_agent` (no recursion), `send_message`, `write_memory`, `schedule_task`, and other side-effecting tools
- Fresh `messages: Vec<Message>` starting from just the task prompt
- Hard limit: `MAX_SUB_AGENT_ITERATIONS = 10`
- Same `stop_reason` logic as the main loop
- **No session persistence** — fully ephemeral
- Returns its final text as the tool result to the parent loop

This prevents unbounded recursive spawning while allowing delegation of focused sub-tasks.

---

## Telemetry & Observability

### Tracing

`tracing` crate used throughout. Every agent loop iteration logs structured fields:
- `chat_id`, `channel`, `iteration`, `stop_reason`
- `input_tokens`, `output_tokens`, `duration_ms`

### Token/Usage Logging to SQLite

After each LLM call (agent loop and compaction):

```rust
db.log_llm_usage(chat_id, channel, provider, model,
                 input_tokens, output_tokens, "agent_loop"|"compaction")
```

### OTLP Metrics Exporter

**File:** `src/otlp.rs`

- Configured via `channels.observability.otlp_enabled: true` + `otlp_endpoint`
- Exports cumulative sum/gauge metrics via protobuf (OTLP/HTTP) in batches
- Metrics tracked:

| Metric | Type |
|---|---|
| `http_requests` | Counter |
| `llm_completions` | Counter |
| `llm_input_tokens` | Counter |
| `llm_output_tokens` | Counter |
| `tool_executions` | Counter |
| `mcp_calls` | Counter |
| `mcp_rate_limited_rejections` | Counter |
| `mcp_bulkhead_rejections` | Counter |
| `mcp_circuit_open_rejections` | Counter |
| `active_sessions` | Gauge |

- Background worker with configurable batch size (default 32), batch max delay (default 1s), retry attempts (default 3) with exponential backoff

### Hook Audit Log

Hook outcomes (allow/block/error) persisted to `db.log_audit_event("hook", name, event, ...)`.

### Memory Injection Log

```rust
db.log_memory_injection(chat_id, retrieval_method, candidate_count,
                        selected_count, omitted, used_tokens)
```

---

## Scheduler & Reflector

**File:** `src/scheduler.rs`

### Scheduler

`spawn_scheduler` runs a tokio task on a **minute-aligned tick**. Each tick calls `run_due_tasks`. For each due task:

1. Calls `process_with_agent(state, ctx, Some(task.prompt), None)` — same entry point as a user message, but with `override_prompt`
2. The scheduler appends `[scheduler]: {prompt}` as a synthetic user message
3. Sends the agent's response back to the chat via `deliver_and_store_bot_message`

Cron expressions parsed by the `cron` crate (0.13).

### Reflector

`spawn_reflector` is a background task that periodically runs LLM-based memory quality reflection across stored memories. Uses the LLM provider directly (not through `process_with_agent`). Evaluates memory relevance, consistency, and confidence.

---

## Key Types

### Core Wire Types (`microclaw-core/src/llm_types.rs`)

```rust
pub struct Message { pub role: String, pub content: MessageContent }
pub enum MessageContent { Text(String), Blocks(Vec<ContentBlock>) }
pub enum ContentBlock {
    Text { text },
    Image { source: ImageSource },
    ToolUse { id, name, input: serde_json::Value },
    ToolResult { tool_use_id, content: String, is_error: bool },
}
pub struct MessagesRequest { model, max_tokens, system, messages, tools, stream }
pub struct MessagesResponse { content: Vec<ResponseContentBlock>, stop_reason, usage }
pub enum ResponseContentBlock { Text { text }, ToolUse { id, name, input }, Other }
pub struct Usage { input_tokens: u32, output_tokens: u32 }
```

### Error Types (`microclaw-core/src/error.rs`)

```rust
pub enum MicroClawError {
    LlmApi(String), RateLimited, Database(rusqlite::Error),
    Http(reqwest::Error), Json(serde_json::Error), Io(std::io::Error),
    ToolExecution(String), Config(String), MaxIterations(usize),
}
```

### Channel Types (`microclaw-channels/src/`)

```rust
pub enum ConversationKind { Private, Group }
pub struct ChatRouting { pub channel_name: String, pub conversation: ConversationKind }

#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    fn name(&self) -> &str;
    fn chat_type_routes(&self) -> Vec<(&str, ConversationKind)>;
    fn is_local_only(&self) -> bool;
    fn allows_cross_chat(&self) -> bool;
    async fn send_text(&self, external_chat_id: &str, text: &str) -> Result<(), String>;
    async fn send_attachment(...) -> Result<String, String>;
}
```

---

## Comparison with animus-rs

| Aspect | MicroClaw | animus-rs |
|---|---|---|
| **Database** | SQLite (bundled rusqlite) | Postgres (SQLx + pgmq + pgvector) |
| **Async runtime** | Tokio | Tokio |
| **LLM** | Raw HTTP (reqwest) to Anthropic/OpenAI | rig-core (Anthropic provider) |
| **Agent loop** | Single `for` loop in `agent_engine.rs` (up to 100 iterations) | Faculty-driven focus lifecycle in `engine/focus.rs` |
| **Tool system** | `Tool` trait with `ToolRegistry`, 21+ built-in tools | External faculty hooks (not built-in tool registry) |
| **Work routing** | Channel adapters → shared `process_with_agent` | pgmq queues → ControlPlane → Faculty → Focus |
| **State machine** | Implicit (loop iteration + stop_reason) | Explicit (Created→Queued→Claimed→Running→Completed/Failed/Dead) |
| **Dedup** | Not structural (session-based replay prevention) | Structural on `(work_type, dedup_key)` — transactional |
| **Memory** | File (AGENTS.md) + SQLite structured + optional sqlite-vec | pgvector (embedding + hybrid BM25+vector) |
| **Observability** | tracing + SQLite usage log + OTLP metrics | OTel three-signal (traces, metrics, logs) via Collector |
| **Hook system** | Shell script hooks with BeforeLLM/BeforeTool/AfterTool events | Faculty hooks (pre-focus, post-focus) |
| **Permissions** | Risk levels (High/Medium/Low) + approval gate + path guard | N/A (trusted faculty system) |
| **Session persistence** | Full `Vec<Message>` serialized to SQLite JSON | N/A (work items + queue state in Postgres) |
| **Context management** | LLM-based compaction when > 40 messages | N/A (per-work-item context, not conversational) |
| **Deployment** | Single binary, SQLite file, optional Docker sandbox | Docker Compose appliance (Postgres + OTel stack) |
| **Channel support** | 14 chat platforms + web | Internal queue-driven (no external chat channels) |
| **Sub-agents** | Built-in `sub_agent` tool (restricted, ephemeral, 10 iterations) | N/A (faculties are the unit of cognitive specialization) |

### Key Architectural Differences

**MicroClaw** is a **conversational agent** — its unit of work is a chat message, and the agent loop runs per-message with session continuity across messages. Tools are built-in and execute within the same process. The system is designed for multi-platform chat bot deployment.

**animus-rs** is a **substrate for relational beings** — its unit of work is a `WorkItem` routed through queues, and faculties are pluggable cognitive specializations that run in isolated focus directories. The system is designed for autonomous agents that persist and evolve, not for chat-driven interaction.

Both share the fundamental pattern: an LLM-driven loop that calls tools/faculties until a stopping condition is met. MicroClaw's implementation is more detailed because it handles the full chat UX lifecycle (streaming, typing indicators, cross-platform delivery, session management, context compaction), while animus-rs delegates that to the faculty system.
