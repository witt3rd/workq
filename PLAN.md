# Implementation Plan

*Tests first. Implementation earns its existence by making red tests green.*

## The Bootstrap

Animus builds itself. The design docs are skills. PLAN.md is the work queue. The test signatures are acceptance criteria. Each milestone is a work item.

Before animus exists, Claude Code proxies the engage loop — reading design docs as skills, writing tests first, implementing to green, recording findings that validate the designs. The human is the orient + consolidate hooks: preparing context, reviewing results, deciding what's next.

Every friction point during implementation is a design signal. If context is lost mid-implementation, that's a finding about compaction. If a design doc doesn't guide implementation well enough, that's a finding about skill structure. If cross-milestone dependencies are hard to track, that's a finding about the awareness digest. We validate the model by living it.

### The Kernel (Human + Claude)

M3 (LLM client) + M4 (work ledger) + M5a (minimal engage loop) must be built by the human-Claude collaboration. This is the bootstrap kernel — the smallest thing that can do work.

### Self-Construction (Animus)

Once the kernel exists, remaining milestones become work items in animus's own queue. Design docs become skills animus activates. Each milestone it completes makes it more capable of completing the next. The system proves its own design by executing its own design.

### The Proxy Model

Even during kernel construction, we follow animus patterns:

| Animus concept | During bootstrap (Claude as proxy) |
|---|---|
| Work item | PLAN.md milestone |
| Orient | Human prepares context ("implement M3") |
| Engage loop | Claude iterates: read skill → write tests → implement → green |
| Ledger entries | Findings recorded in commits, session notes, doc updates |
| Skills | Design docs activated for the relevant milestone |
| Consolidate | Human reviews, decides next work item |
| Awareness digest | Claude reads PLAN.md + recent commits for cross-milestone context |

## What We Accomplished (2026-03-01)

### Research
- Deep analysis of MicroClaw's agent loop (`docs/research/microclaw/agent.md`)

### Design Decisions
1. Engage phase is a built-in loop (orient/consolidate/recover remain hooks)
2. Work ledger in Postgres (append-only typed entries, agent-maintained)
3. Bounded sub-contexts via ledger step entries (not token counting)
4. Parallel tool execution via `tokio::JoinSet`
5. Child work items via the work queue (not in-process sub-agents)
6. Awareness digest for cross-faculty coherence (default-on)
7. Code execution sandbox (Docker, Python, tool SDK)
8. Skills with progressive discovery and autopoietic creation
9. Drop rig-core for LLM calls (thin `LlmClient` via `reqwest` + SSE)
10. DESIGN.md restructured as overview pointing to subsystem docs

### Design Documents
| Document | Covers |
|---|---|
| `docs/engage.md` | Engage loop: sub-contexts, parallel tools, child work, awareness, sandbox |
| `docs/ledger.md` | Work ledger schema, tools, context management |
| `docs/skills.md` | Skill discovery, activation, autopoiesis |
| `docs/llm.md` | LlmClient trait, Anthropic/OpenAI providers |
| `docs/research/microclaw/agent.md` | MicroClaw agent loop analysis |

---

## TDD Approach

Every milestone is defined by its tests. The workflow for each:

1. **Write the tests first.** The tests ARE the spec. They define what "done" means in executable form.
2. **Watch them fail.** Red tests confirm the test is actually testing something.
3. **Write the minimum implementation** to make the tests green.
4. **Refactor** with confidence — the tests catch regressions.
5. **Commit.** Green tests = shippable increment.

Tests come in two tiers:
- **Unit tests** (`#[test]`) — always run, no external dependencies. These are the primary driver.
- **Integration tests** (`#[test] #[ignore]`) — need Postgres/Docker/API keys. Run manually or in CI.

---

## Milestone 3: LLM Client

**Replace rig-core with thin provider clients.**

### Tests First

```rust
// tests/llm_types_test.rs — unit, always runs

#[test]
fn completion_request_serializes_to_anthropic_format() {
    // A CompletionRequest with system prompt, messages, and tools
    // serializes to valid Anthropic Messages API JSON
}

#[test]
fn completion_request_serializes_to_openai_format() {
    // Same request serializes to valid OpenAI Chat Completions JSON
    // System prompt becomes a system-role message
}

#[test]
fn anthropic_response_parses_text_and_tool_use() {
    // Raw Anthropic JSON with text + tool_use blocks
    // parses into CompletionResponse with correct ContentBlocks
}

#[test]
fn anthropic_response_parses_stop_reasons() {
    // "end_turn" → StopReason::EndTurn
    // "tool_use" → StopReason::ToolUse
    // "max_tokens" → StopReason::MaxTokens
}

#[test]
fn openai_response_parses_tool_calls() {
    // OpenAI format with choices[0].message.tool_calls
    // maps to ContentBlock::ToolUse with correct id/name/input
}

#[test]
fn sse_parser_handles_partial_chunks() {
    // SSE data split across multiple chunks reassembles correctly
}

#[test]
fn sse_parser_handles_anthropic_stream_events() {
    // content_block_start, content_block_delta (text + tool JSON),
    // message_delta (stop_reason + usage) → correct StreamEvents
}

#[test]
fn rate_limit_error_extracts_retry_after() {
    // 429 response with retry-after header → LlmError::RateLimited
}

#[test]
fn api_error_preserves_status_and_message() {
    // 400 response with error body → LlmError::Api { status: 400, message: "..." }
}
```

```rust
// tests/llm_integration_test.rs — ignored, needs API key

#[test]
#[ignore]
fn anthropic_complete_returns_text() {
    // Real API call with a simple prompt, no tools
    // Returns CompletionResponse with StopReason::EndTurn and non-empty text
}

#[test]
#[ignore]
fn anthropic_complete_with_tools_returns_tool_use() {
    // Real API call with a tool definition and a prompt that triggers tool use
    // Returns CompletionResponse with StopReason::ToolUse and a ToolUse block
}

#[test]
#[ignore]
fn anthropic_stream_emits_text_deltas() {
    // Real streaming API call
    // StreamEvents arrive via the channel, final response matches non-streaming
}
```

### Then Implement
- `src/llm/types.rs` — `CompletionRequest`, `CompletionResponse`, `Message`, `ContentBlock`, `StopReason`, `Usage`, `ToolDefinition`, `StreamEvent`, `LlmError`
- `src/llm/sse.rs` — `SseParser`
- `src/llm/anthropic.rs` — `AnthropicClient` implementing `LlmClient`
- `src/llm/openai.rs` — `OpenAiClient` implementing `LlmClient`
- `src/llm/mod.rs` — `LlmClient` trait, `create_client` factory

**Design doc:** [docs/llm.md](docs/llm.md)

---

## Milestone 4: Work Ledger

**Postgres-backed durable working memory.**

### Tests First

```rust
// tests/ledger_test.rs — ignored, needs Postgres

#[test]
#[ignore]
fn ledger_append_assigns_sequential_ids() {
    // Append 3 entries to a work item
    // seq values are 1, 2, 3
}

#[test]
#[ignore]
fn ledger_append_rejects_invalid_entry_type() {
    // entry_type "banana" → error
}

#[test]
#[ignore]
fn ledger_read_returns_entries_in_order() {
    // Append plan, finding, step, error
    // Read all → returns in seq order
}

#[test]
#[ignore]
fn ledger_read_filters_by_entry_type() {
    // Append plan, finding, step, finding, step
    // Read with filter=finding → returns 2 entries
}

#[test]
#[ignore]
fn ledger_read_last_n_returns_most_recent() {
    // Append 10 entries
    // Read with last_n=3 → returns entries 8, 9, 10
}

#[test]
#[ignore]
fn ledger_read_formatted_groups_by_type() {
    // Append plan, finding, step, decision, error
    // Read formatted → string with PLAN, FINDINGS, STEPS, DECISIONS, ERRORS sections
}

#[test]
#[ignore]
fn ledger_read_formatted_shows_only_latest_plan() {
    // Append plan v1, finding, plan v2
    // Formatted output shows plan v2 only (not v1)
}

#[test]
#[ignore]
fn ledger_entries_cascade_delete_with_work_item() {
    // Create work item, append ledger entries
    // Delete work item → ledger entries gone
}

#[test]
#[ignore]
fn ledger_entries_isolated_between_work_items() {
    // Append to work_item_a and work_item_b
    // Read for work_item_a → only its entries
}
```

```rust
// Unit tests for types — always runs

#[test]
fn ledger_entry_type_roundtrips_through_string() {
    // Plan → "plan" → Plan, Finding → "finding" → Finding, etc.
}
```

### Then Implement
- Migration: `work_ledger` table with `ON DELETE CASCADE`
- `src/model/ledger.rs` — `LedgerEntry`, `LedgerEntryType`
- `src/db/ledger.rs` — `Db::ledger_append`, `Db::ledger_read`, `Db::ledger_read_formatted`

**Design doc:** [docs/ledger.md](docs/ledger.md)

---

## Milestone 5a: Minimal Engage Loop

**The simplest viable agentic loop.**

Depends on: M3 (LLM client), M4 (ledger).

### Tests First

```rust
// tests/engage_test.rs

#[test]
fn tool_registry_returns_definitions_for_registered_tools() {
    // Register ledger_append and ledger_read
    // definitions() returns both with correct JSON schemas
}

#[test]
fn tool_registry_executes_tool_by_name() {
    // Register a mock tool that returns "hello"
    // execute("mock_tool", json!({})) → ToolResult { content: "hello", is_error: false }
}

#[test]
fn tool_registry_returns_error_for_unknown_tool() {
    // execute("nonexistent", json!({})) → ToolResult { is_error: true }
}

#[test]
#[ignore] // needs Postgres + LLM
fn engage_loop_terminates_on_end_turn() {
    // Faculty with max_turns=10, prompt that requires no tools
    // Loop runs 1 iteration, returns text, stop_reason=EndTurn
}

#[test]
#[ignore]
fn engage_loop_executes_tool_and_continues() {
    // Prompt that triggers ledger_append tool call
    // Loop runs 2+ iterations: tool call → tool result → final text
    // Ledger entry exists in DB after
}

#[test]
#[ignore]
fn engage_loop_respects_max_turns() {
    // Faculty with max_turns=3, prompt that always calls tools
    // Loop runs exactly 3 iterations, returns max-turns error/message
}

#[test]
#[ignore]
fn engage_loop_handles_tool_error_gracefully() {
    // Tool that always returns is_error=true
    // LLM receives error in tool_result, can reason about it
    // Loop doesn't crash
}

#[test]
fn system_prompt_includes_ledger_instructions() {
    // Build system prompt for a faculty
    // Contains "Working Memory" section with ledger_append guidance
}

#[test]
fn system_prompt_includes_focus_context() {
    // Build system prompt with orient context
    // Contains the orient output
}

#[test]
fn messages_alternate_user_assistant_roles() {
    // After tool execution, messages vec has:
    // assistant (with tool_use), user (with tool_result)
    // Validates the Anthropic protocol structure
}
```

### Then Implement
- `src/tools/mod.rs` — `Tool` trait, `ToolDefinition`, `ToolResult`, `ToolRegistry`
- `src/tools/ledger.rs` — `LedgerAppendTool`, `LedgerReadTool` (wired to `Db`)
- `src/engine/engage.rs` — `EngageLoop::run()`: the iteration, LLM call, tool dispatch
- `src/engine/prompt.rs` — System prompt template assembly
- Wire into `src/engine/focus.rs`

**Design doc:** [docs/engage.md](docs/engage.md) §§ 1-2

---

## Milestone 5b: Parallel Tool Execution

Depends on: M5a.

### Tests First

```rust
#[test]
#[ignore]
fn parallel_tools_execute_concurrently() {
    // Register 3 tools, each with a 100ms sleep
    // LLM returns 3 tool_use blocks
    // Total execution time < 200ms (not 300ms)
}

#[test]
#[ignore]
fn parallel_tool_results_match_by_id() {
    // 3 parallel tool calls with distinct IDs
    // Each tool_result has the correct tool_use_id
}

#[test]
fn parallel_tools_disabled_executes_sequentially() {
    // Faculty config: parallel_tool_execution = false
    // 3 tools execute in order (total time ≈ 3x single)
}
```

### Then Implement
- `tokio::JoinSet` dispatch in `EngageLoop::run()`
- `parallel_tool_execution` and `max_parallel_tools` config

---

## Milestone 5c: Bounded Sub-Contexts

Depends on: M5a.

### Tests First

```rust
#[test]
fn step_entry_closes_context_block() {
    // Messages: [iter1 tool_use, iter1 tool_result, iter2 assistant(ledger_append step)]
    // ContextBlocks identifies boundary at iter2
}

#[test]
fn closed_block_replaced_with_stub() {
    // Messages with one closed block (plan + tool calls + step entry)
    // After truncation: closed block → "[completed step 1: ...]"
    // Open block preserved verbatim
}

#[test]
fn multiple_closed_blocks_all_truncated() {
    // 3 closed blocks + 1 open
    // After truncation: 3 stubs + open block verbatim
}

#[test]
fn open_block_never_truncated() {
    // Even if open block is large, it's preserved in full
}

#[test]
fn ledger_nudge_fires_after_threshold() {
    // nudge_interval = 3
    // 3 iterations without ledger_append → system message injected
}

#[test]
fn ledger_nudge_resets_on_ledger_write() {
    // 2 iterations, ledger_append, 2 more iterations → no nudge
    // 2 iterations, ledger_append, 3 more → nudge
}

#[test]
#[ignore]
fn compaction_fallback_fires_on_token_pressure() {
    // Messages exceed compact_threshold
    // No step entries → ledger-based compaction fires
    // Messages replaced with ledger summary + recent tail
}
```

### Then Implement
- `src/engine/context.rs` — `ContextBlocks`, truncation, compaction fallback
- Nudge logic in `EngageLoop`

**Design doc:** [docs/engage.md](docs/engage.md) § 1, [docs/ledger.md](docs/ledger.md)

---

## Milestone 6: Awareness Digest

Depends on: M4 (ledger), M5a (engage loop).

### Tests First

```rust
#[test]
#[ignore]
fn digest_includes_running_siblings() {
    // Work items A (running) and B (running, current focus)
    // Digest for B includes A with its latest plan
}

#[test]
#[ignore]
fn digest_includes_recently_completed_work() {
    // Work item completed 2 hours ago
    // Digest includes it with outcome summary
}

#[test]
#[ignore]
fn digest_includes_cross_faculty_findings() {
    // Ledger finding from a different faculty's focus
    // Appears in current focus's digest
}

#[test]
#[ignore]
fn digest_excludes_current_work_item() {
    // Current focus's own work item doesn't appear in "running" section
}

#[test]
#[ignore]
fn digest_respects_lookback_hours() {
    // lookback_hours = 1
    // Work completed 2 hours ago excluded
}

#[test]
#[ignore]
fn digest_disabled_returns_empty() {
    // Faculty config: awareness.enabled = false
    // Digest is empty string
}

#[test]
fn digest_format_is_readable() {
    // Given structured data, formatted output has
    // "Currently active:", "Recently completed:", "Recent findings:" sections
}
```

### Then Implement
- `src/engine/awareness.rs` — digest assembly and formatting
- Inject in `src/engine/focus.rs` during orient

**Design doc:** [docs/engage.md](docs/engage.md) § 4

---

## Milestone 7: Child Work Items

Depends on: M5a, existing work queue.

### Tests First

```rust
#[test]
#[ignore]
fn spawn_child_creates_work_item_with_parent_id() {
    // spawn_child_work("analyze", "check the config", {})
    // New work item in DB with parent_id = current work item
}

#[test]
#[ignore]
fn await_child_blocks_until_completion() {
    // Spawn child, child completes after 1 second
    // await_child_work returns outcome data
}

#[test]
#[ignore]
fn await_child_times_out() {
    // Spawn child, child never completes
    // await_child_work with timeout=1s → timeout error
}

#[test]
#[ignore]
fn await_multiple_children() {
    // Spawn 3 children, all complete
    // await_child_work([a, b, c]) → 3 outcomes
}

#[test]
#[ignore]
fn check_child_returns_state_without_blocking() {
    // Spawn child (still running)
    // check_child_work → state: "running", no block
}

#[test]
#[ignore]
fn child_depth_limit_enforced() {
    // Work item at depth 5
    // spawn_child_work → error (max depth exceeded)
}

#[test]
#[ignore]
fn child_outcome_includes_ledger_summary() {
    // Child runs, writes ledger entries, completes
    // await_child_work → outcome includes formatted ledger
}
```

### Then Implement
- `src/tools/child_work.rs`
- `NOTIFY` on terminal state transitions in `src/db/work.rs`

**Design doc:** [docs/engage.md](docs/engage.md) § 3

---

## Milestone 8: Code Execution Sandbox

Depends on: M5a, Docker.

### Tests First

```rust
#[test]
#[ignore] // needs Docker
fn sandbox_executes_python_and_returns_output() {
    // execute_code("return 2 + 2") → "4"
}

#[test]
#[ignore]
fn sandbox_tool_sdk_calls_engine_tools() {
    // execute_code("result = read_file('test.txt'); return result[:10]")
    // read_file tool is called via SDK, result returned
}

#[test]
#[ignore]
fn sandbox_timeout_kills_execution() {
    // execute_code("import time; time.sleep(999)", timeout=1)
    // Returns timeout error
}

#[test]
#[ignore]
fn sandbox_memory_limit_enforced() {
    // Code that allocates >512MB → killed
}

#[test]
#[ignore]
fn sandbox_return_value_enters_context_not_raw_output() {
    // execute_code that calls bash(long command)
    // Tool result is the return value, not the bash output
}

#[test]
#[ignore]
fn sandbox_has_no_network_access_beyond_engine() {
    // execute_code("import urllib.request; urllib.request.urlopen('http://example.com')")
    // Fails — no network except engine socket
}

#[test]
#[ignore]
fn sandbox_ledger_append_from_code_closes_context_block() {
    // execute_code("ledger_append('step', 'did the thing')")
    // Engine detects step entry, closes context block
}
```

### Then Implement
- `docker/sandbox/` — Dockerfile, Python SDK
- `src/engine/sandbox.rs` — container lifecycle, HTTP endpoint
- `src/tools/sandbox.rs` — `execute_code` tool

**Design doc:** [docs/engage.md](docs/engage.md) § 5

---

## Milestone 9: Skills System

Depends on: M5a, M4.

### Tests First

```rust
// Phase 9a: Discovery and Activation

#[test]
fn skill_frontmatter_parses_from_yaml() {
    // SKILL.md with YAML frontmatter
    // Parses name, description, triggers, faculties, auto_activate
}

#[test]
fn skill_manager_discovers_skills_from_directory() {
    // skills/ dir with 3 skill directories
    // SkillManager.catalog() → 3 entries with frontmatter
}

#[test]
fn skill_trigger_matches_work_type() {
    // Skill with triggers.work_types = ["engage"]
    // matches("engage") → true, matches("analyze") → false
}

#[test]
fn skill_trigger_matches_keywords() {
    // Skill with triggers.keywords = ["check in", "catch up"]
    // matches_keywords("time to check in with Kelly") → true
}

#[test]
fn skill_activation_returns_full_content() {
    // Activate a skill → returns SKILL.md body + prompt.md content
}

#[test]
fn auto_activate_respects_max_limit() {
    // 10 skills all match, max_auto_activated = 3
    // Only 3 activated
}

// Phase 9b: Autopoiesis

#[test]
fn create_skill_writes_valid_skill_md() {
    // create_skill(name, description, faculties, content)
    // Writes skills/{name}/SKILL.md with valid YAML frontmatter + body
}

#[test]
fn created_skill_is_immediately_discoverable() {
    // Create a skill, then discover_skills
    // New skill appears in catalog
}

#[test]
#[ignore]
fn skill_activation_tracked_in_postgres() {
    // Activate a skill during a focus
    // skill_activations table has a row
}

#[test]
#[ignore]
fn skill_provenance_links_to_ledger_entries() {
    // Create skill from ledger findings
    // skill_provenance rows link to work_item_id and ledger_seq
}
```

### Then Implement
- `src/skills/parser.rs`, `src/skills/mod.rs` — parsing, discovery
- `src/tools/skills.rs` — `discover_skills`, `activate_skill`, `create_skill`
- Migration: `skill_activations`, `skill_provenance`
- `src/db/skills.rs`

**Design doc:** [docs/skills.md](docs/skills.md)

---

## Milestone 5d: Engage Hooks

Depends on: M5a.

### Tests First

```rust
#[test]
fn hook_discovery_finds_hooks_in_directory() {
    // hooks/block-bash/HOOK.md with valid frontmatter
    // HookManager discovers it
}

#[test]
fn hook_before_llm_can_block() {
    // Hook returns {"action": "block", "reason": "nope"}
    // Engage loop exits with reason
}

#[test]
fn hook_before_llm_can_patch_system_prompt() {
    // Hook returns {"action": "allow", "patch": {"system_prompt": "new prompt"}}
    // LLM call uses patched prompt
}

#[test]
fn hook_before_tool_can_skip_tool() {
    // Hook returns {"action": "block", "reason": "not allowed"}
    // Tool not executed, error result injected
}

#[test]
fn hook_after_tool_can_modify_result() {
    // Hook returns {"action": "allow", "patch": {"content": "redacted"}}
    // Tool result content replaced
}

#[test]
fn hooks_run_in_priority_order() {
    // Two hooks: priority 10 and priority 50
    // Priority 10 runs first
}

#[test]
fn hook_timeout_treated_as_allow() {
    // Hook that sleeps beyond timeout_ms
    // Treated as allow (not block)
}
```

### Then Implement
- `src/engine/hooks.rs`

**Design doc:** [docs/engage.md](docs/engage.md) (hook sections throughout)

---

## Dependency Graph

```
M3: LLM Client ─────────────┐
                              ├──→ M5a: Minimal Engage Loop ──→ M5b: Parallel Tools
M4: Work Ledger ─────────────┘          │                    ──→ M5c: Bounded Sub-Contexts
                                        │                    ──→ M5d: Engage Hooks
                                        ├──→ M6: Awareness Digest
                                        ├──→ M7: Child Work Items
                                        ├──→ M8: Code Execution Sandbox
                                        └──→ M9a: Skill Discovery ──→ M9b: Autopoiesis
```

M3 and M4 are parallel (no dependency on each other).
M5a is the gate (requires both M3 and M4).
Everything after M5a is parallelizable.

---

## Execution Order

### Phase 1: Kernel (Human + Claude as proxy-animus)

1. **M3 + M4 in parallel** — LLM client + work ledger. The two foundations.
2. **M5a** — minimal engage loop. The kernel is complete when this is green.

The kernel is the smallest thing that can do work: call an LLM, execute tools, maintain a ledger, and iterate until done. Everything we build here, we build using the proxy model — Claude reads the design doc (skill), writes tests first, implements to green, records findings.

### Phase 2: Self-Enhancement (Animus builds itself)

Once the kernel boots, remaining milestones become work items in animus's own queue:

3. **M5b + M5c** — parallel tools and bounded sub-contexts
4. **M6** — awareness digest
5. **M7** — child work items
6. **M9a** — skill discovery
7. **M5d** — engage hooks
8. **M8** — code execution sandbox
9. **M9b** — autopoietic skill creation

Each milestone animus completes makes it more capable of completing the next. The ordering within phase 2 is flexible — animus (with human guidance) picks the highest-value next work item.

### Phase 3: Self-Awareness

When M6 (awareness digest) and M9b (autopoiesis) are both complete, animus can:
- See all its own concurrent operations
- Learn from its implementation experience
- Create skills encoding what it learned
- Apply those skills to future work

The being becomes aware of itself and grows from its own experience.

---

## Principles

1. **Tests are the spec.** Write the test. Watch it fail. Make it pass. The test defines done.
2. **No implementation without a failing test.** If you can't write a test for it, you don't understand it yet.
3. **Unit tests always run.** No Postgres, no Docker, no API keys. Mock what you need.
4. **Integration tests are ignored by default.** `cargo test` is fast. `cargo test -- --ignored` is thorough.
5. **One concern per commit.** Don't mix LLM client with ledger schema.
6. **Design docs are skills, not maintenance targets.** They guide implementation. Once tests are green, the doc becomes historical. If the design was wrong, update the doc, then update the test.
7. **Smallest viable increment.** M5a is a sequential loop with two tools. Get it green, then layer.
8. **Pre-commit hook must pass.** `cargo fmt` + `cargo test` + `cargo clippy -D warnings`. No bypass.
9. **Every friction is a finding.** When something is hard during implementation, record why. It's a signal about the design, the tooling, or the process. These findings feed the system's evolution.
10. **The proxy model validates the real model.** Claude working as proxy-animus is a test of the animus architecture. If the patterns don't work for Claude, they won't work for animus.
