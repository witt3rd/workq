# Implementation Plan

*From design to working substrate — milestone-based execution.*

## What We Accomplished Today (2026-03-01)

### Research
- Deep analysis of MicroClaw's agent loop — tool system, hooks, permissions, context management, LLM integration, sub-agents, streaming, session persistence (`docs/research/microclaw/agent.md`)

### Design Decisions
1. **Engage phase is a built-in loop**, not an external process. Orient/consolidate/recover remain external hooks. Escape hatch available for faculties that need a custom loop.
2. **Work ledger in Postgres**, not filesystem. Append-only typed entries (plan/finding/decision/step/error/note). Agent-maintained via `ledger_append`/`ledger_read` tools. Engine reads it for compaction. Cross-faculty findings feed the awareness digest.
3. **Bounded sub-contexts** replace token-counting compaction. `ledger_append(step)` closes a context block. Closed blocks are replaced with ledger stubs. Open block preserved verbatim.
4. **Parallel tool execution** via `tokio::JoinSet`, not sequential.
5. **Child work items** via the work queue, not in-process sub-agents. `spawn_child_work`/`await_child_work`/`check_child_work`.
6. **Awareness digest** — engine-level cross-faculty coherence, assembled from `work_items` + `work_ledger`, injected at orient. Default-on.
7. **Code execution sandbox** — Docker-based Python sandbox with tool SDK. Agent writes code that calls tools; return value (not raw output) enters context.
8. **Skills** — progressive discovery, runtime activation, autopoietic creation from ledger findings, filesystem storage with Postgres activation index.
9. **Drop rig-core for LLM calls** — thin `LlmClient` trait with Anthropic + OpenAI implementations via raw `reqwest` + SSE. Keep `rig-postgres` for embeddings.
10. **DESIGN.md restructured** as high-level overview pointing to subsystem docs.

### Design Documents Produced
| Document | Lines | Covers |
|---|---|---|
| `docs/research/microclaw/agent.md` | 906 | MicroClaw agent loop deep research |
| `docs/ledger.md` | 473 | Work ledger schema, tools, context management, observability |
| `docs/engage.md` | 1082 | Engage loop: sub-contexts, parallel tools, child work, awareness, sandbox |
| `docs/skills.md` | 701 | Skill discovery, activation, autopoiesis, sandbox integration |
| `docs/llm.md` | 619 | LlmClient trait, Anthropic/OpenAI providers, migration from rig-core |
| `DESIGN.md` | 261 | Restructured overview with subsystem pointers |

**Total: ~4,000 lines of design, 0 lines of design debt.**

---

## What's Already Implemented

| Module | Status |
|---|---|
| `src/config/` | Typed env var loading, secrecy |
| `src/db/mod.rs` | PgPool, SQLx migrations |
| `src/db/pgmq.rs` | Queue operations (create, send, read, archive, delete) |
| `src/db/work.rs` | Work item submit with structural dedup |
| `src/memory/store.rs` | pgvector storage, vector search, hybrid BM25+vector |
| `src/llm/mod.rs` | rig-core Anthropic client factory (to be replaced) |
| `src/model/work.rs` | WorkItem, State, Provenance, Outcome |
| `src/model/memory.rs` | MemoryEntry, NewMemory, MemoryFilters |
| `src/telemetry/` | Three-signal OTel (traces, metrics, logs), GenAI spans |
| `src/faculty/mod.rs` | Faculty config (TOML), hook definitions, registry |
| `src/engine/control.rs` | ControlPlane loop: PgListener, route to faculty, spawn focus |
| `src/engine/focus.rs` | Focus lifecycle: dir creation, hook pipeline, outcome reading |
| `src/bin/animus.rs` | Control plane daemon |
| `Dockerfile` | Multi-stage Rust build |
| `docker-compose.yml` | Full appliance (animus + Postgres + observability) |

Migrations: extensions, work_items, memories, unique_dedup_index.

---

## Implementation Milestones

The milestones are ordered by dependency. Each builds on the previous. Each is independently shippable and testable.

### Milestone 3: LLM Client

**Replace rig-core with thin provider clients.**

This unblocks everything else — the engage loop needs `LlmClient` to make LLM calls.

| Task | Files | Estimate |
|---|---|---|
| Define `LlmClient` trait, request/response types | `src/llm/types.rs`, `src/llm/mod.rs` | Small |
| Implement SSE stream parser | `src/llm/sse.rs` | Small |
| Implement Anthropic Messages API client | `src/llm/anthropic.rs` | Medium |
| Implement OpenAI Chat Completions client | `src/llm/openai.rs` | Medium |
| Provider factory (`create_client`) | `src/llm/mod.rs` | Small |
| Add `reqwest` to dependencies, update `Cargo.toml` | `Cargo.toml` | Small |
| Integration test: Anthropic round-trip | `tests/llm_test.rs` | Small |
| Remove direct rig-core imports for LLM | `src/llm/mod.rs` (old) | Small |

**Design doc:** [docs/llm.md](docs/llm.md)
**Tests:** Unit tests for SSE parser, type serialization. Integration test (ignored, needs API key) for Anthropic round-trip.
**Done when:** `cargo test` passes, `LlmClient::complete` returns a `CompletionResponse` from Anthropic.

---

### Milestone 4: Work Ledger

**Add the `work_ledger` table and Db API.**

This unblocks the engage loop's context management (bounded sub-contexts depend on ledger entries).

| Task | Files | Estimate |
|---|---|---|
| SQLx migration: `work_ledger` table | `migrations/..._work_ledger.sql` | Small |
| `LedgerEntryType` enum, `LedgerEntry` struct | `src/model/ledger.rs` | Small |
| `Db::ledger_append`, `Db::ledger_read`, `Db::ledger_read_formatted` | `src/db/ledger.rs` | Medium |
| Unit tests for ledger CRUD | `tests/ledger_test.rs` | Small |
| Integrate ledger into `Db` public API | `src/db/mod.rs`, `src/lib.rs` | Small |

**Design doc:** [docs/ledger.md](docs/ledger.md)
**Tests:** Integration tests (ignored) for append, read, read_formatted, filtering by type, `last_n` support.
**Done when:** `Db::ledger_append` writes entries, `Db::ledger_read_formatted` returns grouped output.

---

### Milestone 5: Core Engage Loop

**The agentic tool-use loop — the heart of the system.**

Depends on: Milestone 3 (LLM client), Milestone 4 (ledger). This is the biggest milestone. Break into phases.

#### Phase 5a: Minimal Loop

The simplest viable engage loop: call LLM, check stop_reason, execute tools sequentially, loop until end_turn or max_turns.

| Task | Files | Estimate |
|---|---|---|
| `EngageLoop` struct with configuration | `src/engine/engage.rs` | Medium |
| Tool trait and registry (engine tools: ledger_append, ledger_read) | `src/tools/mod.rs`, `src/tools/ledger.rs` | Medium |
| Basic iteration: LLM call → tool execution → loop | `src/engine/engage.rs` | Large |
| System prompt template (working memory, focus context) | `src/engine/prompt.rs` | Medium |
| Wire into focus lifecycle (replace placeholder engage) | `src/engine/focus.rs` | Medium |
| Integration test: focus runs engage loop with mock tools | `tests/engage_test.rs` | Medium |

**Done when:** A focus can run a multi-turn engage loop with ledger tools, produce ledger entries, and terminate on end_turn.

#### Phase 5b: Parallel Tool Execution

| Task | Files | Estimate |
|---|---|---|
| `tokio::JoinSet` parallel tool dispatch | `src/engine/engage.rs` | Medium |
| Concurrent hook invocation (before/after tool) | `src/engine/engage.rs` | Small |
| Configuration: `parallel_tool_execution`, `max_parallel_tools` | `src/faculty/mod.rs` | Small |

**Done when:** Multiple tool_use blocks in one LLM response execute concurrently.

#### Phase 5c: Bounded Sub-Contexts

| Task | Files | Estimate |
|---|---|---|
| `ContextBlocks` tracking (step boundaries) | `src/engine/context.rs` | Medium |
| Closed block truncation (replace with ledger stubs) | `src/engine/context.rs` | Medium |
| Fallback: ledger-based compaction (threshold-triggered) | `src/engine/context.rs` | Medium |
| Fallback: emergency LLM summarization | `src/engine/context.rs` | Small |
| Engine nudge (iterations since last ledger write) | `src/engine/engage.rs` | Small |

**Done when:** Closed context blocks are replaced with ledger stubs. Context pressure stays bounded during long loops.

#### Phase 5d: Engage Hooks

| Task | Files | Estimate |
|---|---|---|
| `BeforeLLMCall` hook event | `src/engine/hooks.rs` | Medium |
| `BeforeToolCall` / `AfterToolCall` hook events | `src/engine/hooks.rs` | Medium |
| Hook outcome: allow/block/modify with patches | `src/engine/hooks.rs` | Medium |
| Hook discovery from filesystem | `src/engine/hooks.rs` | Small |

**Done when:** External hook scripts can intercept LLM calls and tool executions.

**Design doc:** [docs/engage.md](docs/engage.md) §§ 1-2
**Tests at each phase.** The engage loop is critical infrastructure — every phase must be tested before the next begins.

---

### Milestone 6: Awareness Digest

**Cross-faculty coherence.**

Depends on: Milestone 4 (ledger — findings are the raw material), Milestone 5a (engage loop — must exist to inject into).

| Task | Files | Estimate |
|---|---|---|
| Digest assembly queries (running, completed, findings) | `src/engine/awareness.rs` | Medium |
| Digest formatting for system prompt injection | `src/engine/awareness.rs` | Small |
| Inject during orient phase | `src/engine/focus.rs` | Small |
| Configuration: `[faculty.awareness]` section | `src/faculty/mod.rs` | Small |
| Integration test: digest includes sibling work | `tests/awareness_test.rs` | Medium |

**Design doc:** [docs/engage.md](docs/engage.md) § 4
**Done when:** A focus's orient context includes awareness of running siblings and recent findings.

---

### Milestone 7: Child Work Items

**Async delegation via the work queue.**

Depends on: Milestone 5a (engage loop), existing work queue infrastructure.

| Task | Files | Estimate |
|---|---|---|
| `spawn_child_work` tool (creates child work item with parent_id) | `src/tools/child_work.rs` | Medium |
| `await_child_work` tool (pg_notify-based blocking wait) | `src/tools/child_work.rs` | Medium |
| `check_child_work` tool (non-blocking poll) | `src/tools/child_work.rs` | Small |
| `NOTIFY` emission on work item terminal state transition | `src/db/work.rs` | Small |
| Integration test: parent spawns child, awaits, reads outcome | `tests/child_work_test.rs` | Medium |
| Depth limit enforcement (prevent unbounded recursion) | `src/tools/child_work.rs` | Small |

**Design doc:** [docs/engage.md](docs/engage.md) § 3
**Done when:** A focus can spawn child work items and await their outcomes.

---

### Milestone 8: Code Execution Sandbox

**Docker-based Python sandbox for programmatic tool composition.**

Depends on: Milestone 5a (engage loop), Docker infrastructure (already present).

| Task | Files | Estimate |
|---|---|---|
| Sandbox container image (Python + tool SDK) | `docker/sandbox/Dockerfile`, `docker/sandbox/sdk/` | Medium |
| Tool SDK: Python functions that call engine over HTTP | `docker/sandbox/sdk/tools.py` | Medium |
| Engine-side HTTP endpoint for sandbox tool calls | `src/engine/sandbox.rs` | Medium |
| `execute_code` tool implementation | `src/tools/sandbox.rs` | Large |
| Container lifecycle (start, execute, capture output, destroy) | `src/engine/sandbox.rs` | Medium |
| Resource limits (CPU, memory, timeout) | `src/engine/sandbox.rs` | Small |
| OTel: nested spans for SDK calls | `src/engine/sandbox.rs` | Small |
| Configuration: `[faculty.engage]` sandbox settings | `src/faculty/mod.rs` | Small |
| Integration test: code execution with tool SDK calls | `tests/sandbox_test.rs` | Medium |

**Design doc:** [docs/engage.md](docs/engage.md) § 5
**Done when:** A focus can call `execute_code` and the agent's Python code calls tools via the SDK.

---

### Milestone 9: Skills System

**Progressive discovery, runtime activation, autopoietic creation.**

Depends on: Milestone 5a (engage loop), Milestone 4 (ledger — findings feed skill creation).

#### Phase 9a: Skill Discovery and Activation

| Task | Files | Estimate |
|---|---|---|
| `SKILL.md` frontmatter parser (YAML extraction) | `src/skills/parser.rs` | Small |
| `SkillManager`: filesystem scan, catalog building | `src/skills/mod.rs` | Medium |
| `discover_skills` tool | `src/tools/skills.rs` | Small |
| `activate_skill` tool (inject prompt context) | `src/tools/skills.rs` | Medium |
| Auto-activation during orient (trigger matching) | `src/engine/focus.rs` | Medium |
| System prompt: skills catalog section | `src/engine/prompt.rs` | Small |

**Done when:** The engage loop can discover and activate skills from the filesystem.

#### Phase 9b: Skill Creation (Autopoiesis)

| Task | Files | Estimate |
|---|---|---|
| `create_skill` tool (writes SKILL.md to filesystem) | `src/tools/skills.rs` | Medium |
| SQLx migration: `skill_activations`, `skill_provenance` tables | `migrations/..._skills.sql` | Small |
| Activation tracking (Postgres) | `src/db/skills.rs` | Small |
| Provenance tracking (ledger entry → skill content) | `src/db/skills.rs` | Medium |
| Awareness digest: "skills updated" section | `src/engine/awareness.rs` | Small |

**Done when:** A focus can create skills, and future foci can discover them. Provenance is tracked.

**Design doc:** [docs/skills.md](docs/skills.md)

---

### Milestone 10: Polish and Integration

| Task | Estimate |
|---|---|
| OTel span hierarchy for full focus lifecycle (engage iterations, tool calls, sandbox, child work) | Medium |
| Metrics: all counters/histograms from docs/engage.md and docs/ledger.md | Medium |
| Faculty TOML: complete configuration surface from docs/engage.md | Small |
| CLI: `animus status`, `animus work list`, `animus work show`, `animus ledger show` | Medium |
| End-to-end test: full focus lifecycle with real LLM, ledger, tools, skills | Large |
| Documentation: update CLAUDE.md with new modules | Small |

---

## Dependency Graph

```
M3: LLM Client ─────────────┐
                              ├──→ M5a: Minimal Engage Loop ──→ M5b: Parallel Tools
M4: Work Ledger ─────────────┘          │                          │
                                        ├──→ M5c: Bounded Sub-Contexts
                                        ├──→ M5d: Engage Hooks
                                        ├──→ M6: Awareness Digest
                                        ├──→ M7: Child Work Items
                                        ├──→ M8: Code Execution Sandbox
                                        └──→ M9a: Skill Discovery ──→ M9b: Autopoiesis
                                                                           │
                                        M10: Polish ←─────────────────────┘
```

M3 and M4 can proceed in parallel — they have no dependency on each other.
M5a requires both M3 and M4.
M5b, M5c, M5d, M6, M7, M8, M9a can proceed in parallel after M5a (they're independent).
M9b requires M9a.
M10 requires everything.

---

## Principles for Execution

1. **Test at every phase.** The engage loop is the heart of the system. No phase ships without tests.
2. **Integration tests are ignored by default** (need Postgres / Docker / API keys). Unit tests always run.
3. **One concern per PR.** Don't mix LLM client changes with ledger schema changes.
4. **Design docs are authoritative.** Implementation follows the docs. If the implementation needs to diverge, update the doc first.
5. **Smallest viable increment.** M5a is a minimal loop — sequential tools, no compaction, no sandbox. Get it working, then layer on complexity.
6. **Pre-commit hook must pass.** `cargo fmt`, `cargo test`, `cargo clippy -D warnings`. No bypass.
