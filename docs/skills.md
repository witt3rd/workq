---
status: active
milestone: M9
spec: PLAN.md § Milestone 9
code: null
---

# Skills Architecture

*Progressive discovery, runtime activation, and autopoietic evolution of agent capabilities.*

## Context

The engage loop design (`docs/engage.md`) defines five mechanisms for how a focus executes work: bounded sub-contexts, parallel tool execution, child work items, the awareness digest, and the code execution sandbox. The ledger design (`docs/ledger.md`) defines durable working memory. The faculty TOML defines a static tool set.

But real agents need more than intrinsic tools. A Social faculty checking in with Kelly needs relationship context, communication preferences, draft templates. A Computer Use faculty analyzing a codebase needs language-specific patterns, framework conventions, review checklists. These aren't tools — they're *knowledge packaged as capabilities*.

Skills are how faculties acquire new capabilities — both at design time (humans author skills) and at runtime (the agent discovers, activates, and creates skills). They bridge the gap between the fixed tool set in faculty TOML and the open-ended nature of real work.

## Prior Art

### Claude Code Skills

YAML frontmatter + markdown body. Progressive disclosure: the agent sees a one-liner in the catalog, decides to read more, and the full content provides detailed instructions, scripts, and resources. The agent can create and modify its own skills. This model prioritizes runtime knowledge acquisition and autopoiesis.

### MicroClaw Skills

Similar to Claude Code. Skills are directories discovered from a skills path, activated via a tool call. Activation injects prompt context and optionally loads `.env` files for tool configuration. A `SkillManager` handles discovery and activation.

### Nanoclaw/Nanorepo Architecture

Skills as composable code modifications. A skill literally modifies the system — adds files, changes source code, adds dependencies. Three-way git merge with structured conflict resolution. Intent is first-class: every modified file has a `.intent.md` with machine-readable headings (what the skill adds, invariants, must-keep sections). Tests run after every operation. State tracked for deterministic replay.

Key nanoclaw principles relevant to animus-rs:
- **Intent is first-class and structured** — not just documentation, but a contract
- **One skill, one happy path** — the reasonable default for 80% of cases
- **Skills layer via dependencies** — extension skills build on base skills
- **Always tested** — tests after apply, update, uninstall, replay
- **Customization via patching** — users apply the default, then customize

### Synthesis

Claude Code and MicroClaw show that runtime skill discovery and activation work well — agents are good at deciding which skills are relevant and incorporating their guidance. Nanoclaw shows that skills can also modify the system itself, with composability guaranteed by git-based merging and intent documentation.

animus-rs needs both: runtime skills that augment a focus's capabilities during the engage loop, and system skills that evolve the agent's infrastructure. And because animus-rs is a substrate for relational beings, it needs a third thing neither prior system fully addresses: **autopoietic skill creation** — the agent learning from its own experience and encoding that learning as skills for future use.

---

## Three Levels of Skills

Skills in animus-rs operate at three levels, each building on the one below:

### Level 1: Runtime Skills (Discovery and Activation)

Skills that augment a focus's capabilities during the engage loop. The skill provides prompt context, instructions, reference data, and optionally scripts. The agent discovers relevant skills, activates them, and their content becomes part of the engage context.

This is the primary skill interaction for most work. A faculty doesn't need to know every skill at configuration time — it discovers what it needs based on the work at hand.

### Level 2: Autopoietic Skills (Creation and Evolution)

Skills that the agent creates from its own experience. Recurring patterns, relationship knowledge, domain expertise, debugging strategies — anything the agent learns that would be useful in future work. The consolidate hook is the natural place for skill creation, using ledger entries as raw material.

This is how the being learns and grows. A finding recorded in one focus becomes a skill available to all future foci.

### Level 3: System Skills (Infrastructure Modification)

Skills that modify the animus-rs system itself — adding tools, faculties, hooks, channel adapters. These use nanoclaw-style composable code modifications with git three-way merge, intent documentation, and mandatory testing.

This is the most ambitious level and the longest path to implementation. The architecture accommodates it, but Levels 1 and 2 come first.

---

## Skill Package Structure

```
skills/
  {skill-name}/
    SKILL.md              # YAML frontmatter + progressive disclosure body
    prompt.md             # optional: detailed prompt context injected on activation
    scripts/              # optional: callable from code execution sandbox
      analyze.py
      draft.py
    resources/            # optional: reference data, templates, examples
      preferences.md
      template.md
    tests/                # optional: validation tests
      test_skill.py
    manifest.yaml         # optional: for system-level skills (Level 3)
    add/                  # optional: new files for system skills
    modify/               # optional: modified files + intent docs for system skills
```

Only `SKILL.md` is required. Everything else is progressive — added as the skill grows in sophistication.

---

## SKILL.md Format

The skill's primary file uses YAML frontmatter for machine-readable metadata and a markdown body for progressive disclosure.

### Frontmatter

```yaml
---
name: check-in-with-person
description: >
  Guides relational check-ins — timing, tone, context gathering,
  memory integration, and follow-up scheduling.
triggers:
  work_types: ["engage", "check-in", "respond"]
  keywords: ["check in", "catch up", "reach out", "follow up"]
  params:                           # match against work item params
    person: "*"                     # any value for "person" param triggers
faculties: ["social", "heartbeat"]  # which faculties can use this skill
tools_needed: ["memory-search", "send-message", "calendar"]
auto_activate: true                 # activate when triggers match (vs. manual)
created_by: animus                  # "animus" (autopoietic) or "human"
created_from: work_item:abc123      # optional: source work item for autopoietic skills
version: 3
updated_at: 2026-02-28
depends: []                         # other skills this one builds on
conflicts: []                       # skills that can't be active simultaneously
---
```

### Body (Progressive Disclosure)

The body is structured for three levels of reading depth:

```markdown
# Check-In With Person

Brief description for the skills catalog. The agent reads this far during
discovery — one or two sentences that help it decide whether to activate.

## When to Use

Activate this skill when the work involves checking in with a specific person
you have a relationship with. Not for cold outreach or transactional messages.

## Instructions

### Context Gathering

Before reaching out, use `memory-search` to find:
- Recent interactions with this person (last 2 weeks)
- Known preferences (timing, topics, communication style)
- Any pending items, commitments, or follow-ups
- Recent findings from other faculties involving this person

Check the awareness digest — another faculty may have recently interacted
with this person or learned something relevant.

### Composing the Check-In

[Detailed guidance on tone, structure, personalization...]

### Follow-Up

After sending, use `ledger_append(finding)` to record:
- What you learned from the interaction
- Any commitments made
- Suggested timing for next check-in

Consider using `schedule_task` to queue the next check-in.

## Scripts

`scripts/draft-check-in.py` — generates a draft message given relationship
context. Callable from the code execution sandbox:

```python
from skills.check_in_with_person.scripts.draft import draft_check_in
draft = draft_check_in(person="Kelly", context=context, tone="warm")
```

## Resources

`resources/communication-styles.md` — reference on adapting tone and timing
to different relationship types.
```

**Level 1** (catalog): The agent reads the frontmatter `description` field. One line in the catalog.

**Level 2** (activation decision): The agent reads through "When to Use". Enough to decide whether to activate.

**Level 3** (full incorporation): The agent reads the full body. Instructions, scripts, resources become part of its working context.

---

## Engine Integration

### Skill Discovery and Activation Tools

Three engine tools, always available (like ledger tools):

#### `discover_skills`

```json
{
  "name": "discover_skills",
  "description": "Search available skills. Returns frontmatter only (name, description, triggers). Use this to find skills relevant to your current work.",
  "input_schema": {
    "type": "object",
    "properties": {
      "query": {
        "type": "string",
        "description": "Natural language query or keywords to match against skill descriptions and triggers."
      },
      "work_type": {
        "type": "string",
        "description": "Optional: filter to skills that trigger on this work type."
      },
      "faculty": {
        "type": "string",
        "description": "Optional: filter to skills available to this faculty."
      }
    }
  }
}
```

Returns a concise listing:

```
Available skills matching "check in":
  [1] check-in-with-person — Guides relational check-ins (auto-activate: on)
  [2] follow-up-scheduler — Manages follow-up timing and reminders
  [3] kelly-relationship — Context for interactions with Kelly (created by: animus)
```

#### `activate_skill`

```json
{
  "name": "activate_skill",
  "description": "Activate a skill for the current focus. Loads its full prompt context, makes its scripts available in the code execution sandbox, and registers its resources. Use after discovering a relevant skill.",
  "input_schema": {
    "type": "object",
    "properties": {
      "skill_name": {
        "type": "string",
        "description": "Name of the skill to activate."
      }
    },
    "required": ["skill_name"]
  }
}
```

On activation:
1. Read the full `SKILL.md` body and `prompt.md` (if present)
2. Inject the skill's prompt context into the engage loop (appended to the system prompt or as a context message)
3. Register the skill's `scripts/` directory as importable in the code execution sandbox
4. Make the skill's `resources/` files accessible via `read_file`
5. Record activation in the focus's engine state (for OTel tracing)

#### `create_skill`

```json
{
  "name": "create_skill",
  "description": "Create a new skill from what you've learned. Encodes patterns, knowledge, or procedures as a reusable skill for future foci. The skill will be discoverable by all faculties listed in the frontmatter.",
  "input_schema": {
    "type": "object",
    "properties": {
      "name": {
        "type": "string",
        "description": "Skill name (kebab-case). Used as the directory name."
      },
      "description": {
        "type": "string",
        "description": "Brief description for the skills catalog."
      },
      "faculties": {
        "type": "array",
        "items": { "type": "string" },
        "description": "Which faculties can use this skill."
      },
      "triggers": {
        "type": "object",
        "description": "Optional: work_types, keywords, and params that auto-activate this skill."
      },
      "content": {
        "type": "string",
        "description": "The skill body (markdown). Instructions, context, guidance."
      }
    },
    "required": ["name", "description", "faculties", "content"]
  }
}
```

The engine writes the `SKILL.md` file with generated frontmatter and the provided content. The skill is immediately discoverable by future foci.

### Auto-Activation During Orient

During the orient phase, the engine can automatically discover and pre-activate skills:

1. Read `work_type` and `params` from the work item
2. Scan all `SKILL.md` frontmatter for matching `triggers`:
   - `work_types` match against the work item's `work_type`
   - `keywords` match against the work item's description/params
   - `params` match against specific work item parameter keys/values
3. Filter by `faculties` — only skills available to the current faculty
4. For skills with `auto_activate: true`, activate them (inject prompt context)
5. For all remaining skills, include them in the catalog section of the system prompt

The agent can still manually discover and activate additional skills during the engage loop. Auto-activation is a convenience for common patterns, not a constraint.

```toml
[faculty.orient]
command = "scripts/social-orient"
auto_activate_skills = true       # default: true
max_auto_activated = 5            # prevent prompt bloat from too many skills
```

---

## Autopoietic Skill Lifecycle

### Creation: From Findings to Skills

The consolidate hook is the natural place for skill creation. After a focus completes, the consolidate hook queries the ledger for findings that might be worth encoding:

```sql
-- Findings from this focus
SELECT content FROM work_ledger
WHERE work_item_id = $1 AND entry_type = 'finding'
ORDER BY seq;

-- Similar findings from recent foci (pattern detection)
SELECT w.work_type, wl.content, count(*) as occurrences
FROM work_ledger wl
JOIN work_items w ON w.id = wl.work_item_id
WHERE wl.entry_type = 'finding'
    AND wl.created_at > now() - interval '7 days'
    AND wl.content ILIKE '%' || $pattern || '%'
GROUP BY w.work_type, wl.content
HAVING count(*) >= 3
ORDER BY occurrences DESC;
```

When a pattern appears across multiple foci (e.g., "Kelly prefers morning messages" found in 3 separate check-ins), the consolidate hook creates or updates a skill:

```
Consolidate detects: 3 findings about Kelly's preferences
  → Checks: does skills/kelly-relationship/ exist?
  → No: creates it with the accumulated findings
  → Yes: updates resources/preferences.md with new findings
```

### Evolution: Skills Improve Over Time

Autopoietic skills have a `version` field and `updated_at` timestamp. Each consolidate pass that updates a skill bumps the version. The skill's content accumulates knowledge:

```
v1: "Kelly prefers morning messages"
v2: "Kelly prefers morning messages, always asks about the cat"
v3: "Kelly prefers morning messages (before 10am), always asks about the cat,
     interested in Rust, suggested a writing project"
```

The skill doesn't just grow — it can also be refined. If the agent discovers that a previous finding was wrong ("Kelly actually prefers evening messages now"), the consolidate hook updates the skill accordingly.

### Provenance Tracking

Autopoietic skills track their origin:

```yaml
created_by: animus
created_from: work_item:abc123
creation_findings:
  - "Kelly prefers morning messages" (work_item:abc123, seq:7)
  - "Kelly always asks about the cat" (work_item:def456, seq:12)
  - "Kelly interested in Rust" (work_item:ghi789, seq:4)
```

This provenance chain — from ledger finding to skill content — is auditable. You can trace exactly which work produced which knowledge.

### Retirement

Skills can become stale. A relationship skill for someone the agent hasn't interacted with in months. A debugging skill for a framework version that's been upgraded. The engine can track skill activation frequency:

```sql
-- Skills that haven't been activated in 30 days
SELECT skill_name, last_activated_at
FROM skill_activations
WHERE last_activated_at < now() - interval '30 days';
```

Stale skills aren't deleted — they're flagged. The agent (or a human) can archive them. A Heartbeat faculty reflection could review stale skills as part of periodic self-maintenance.

---

## Skills in the Engage Loop

### System Prompt Integration

The engage loop's system prompt includes a skills section (after the existing Working Memory and Execution Modes sections):

```
## Skills

You have access to skills — packaged knowledge and capabilities that extend
your tools. Some skills have been auto-activated based on your current work
and are available below. You can discover additional skills with `discover_skills`
and activate them with `activate_skill`.

### Active Skills

[Auto-activated skill prompt contexts are injected here]

### Available Skills (not yet activated)

- follow-up-scheduler — Manages follow-up timing and reminders
- conversation-starters — Context-appropriate opening lines
- mood-detection — Guidance on reading emotional tone in messages
```

### Skill Context and Compaction

Activated skill prompt context is injected into the system prompt, not into the message history. This means:

- Skill context survives bounded sub-context compaction (system prompt is never truncated)
- Skill context doesn't grow with iterations — it's fixed-size
- Multiple skills compose by concatenation in the system prompt

If skill context grows too large (too many activated skills), the engine enforces `max_auto_activated` and warns the agent that it should deactivate skills it no longer needs:

```
[engine] You have 7 active skills consuming significant prompt context.
Consider deactivating skills that are no longer relevant to your current step.
```

### Skills in the Code Execution Sandbox

When `code_execution = true` and a skill has a `scripts/` directory, those scripts are importable in the sandbox:

```python
# Inside execute_code — activated skill scripts are available
from skills.check_in_with_person.scripts.draft import draft_check_in
from skills.kelly_relationship.resources import preferences

context = memory_search("Kelly recent interactions")
prefs = read_file(preferences.path)
draft = draft_check_in(person="Kelly", context=context, preferences=prefs)
return f"Draft check-in:\n{draft}"
```

Skill scripts run inside the same sandbox container with the same security model. They can call the tool SDK functions. They are subject to the same resource limits and timeout.

---

## Skills and the Awareness Digest

The awareness digest (`docs/engage.md` § 4) surfaces recent findings across all foci. Skills add another dimension: the digest can also surface recently created or updated skills.

```
== AWARENESS ==

Currently active:
- [Social/engage] Checking in with Kelly
  ...

Recently completed:
- ...

Recent findings:
- Kelly mentioned interest in Rust (Heartbeat/reflect, 5h ago)
- ...

Skills updated:
- kelly-relationship v3 (updated by Social/consolidate, 2h ago)
  Added: "suggested a writing project"
- codebase-review-checklist v1 (created by Computer Use/consolidate, 6h ago)
  New skill for code review patterns
```

This means the awareness digest not only shows what happened (work items and findings) but also what was *learned* (skills created and evolved). The being's growth is visible to all its faculties.

---

## Skill Storage

### Filesystem (Levels 1 and 2)

Runtime and autopoietic skills live in the filesystem under the configured skills directory:

```
{data_dir}/skills/                  # root skills directory
  check-in-with-person/             # human-authored skill
    SKILL.md
    prompt.md
    scripts/
    resources/
  kelly-relationship/               # autopoietic skill
    SKILL.md
    resources/preferences.md
  analyze-codebase/                 # human-authored skill
    SKILL.md
    prompt.md
    scripts/analyze.py
    tests/test_analyze.py
```

The filesystem is the right home for skills because:
- Skills are text files — markdown, YAML, Python scripts — that benefit from version control
- Progressive discovery is natural — read frontmatter first, read body later
- The code execution sandbox can mount the skills directory
- Git tracks skill evolution (autopoietic skills have commit history)

### Skill Activation Index (Postgres)

While skills live on the filesystem, activation metadata lives in Postgres for queryability:

```sql
CREATE TABLE skill_activations (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    skill_name      TEXT NOT NULL,
    work_item_id    UUID REFERENCES work_items(id),
    faculty         TEXT NOT NULL,
    activated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    activation_type TEXT NOT NULL     -- 'auto' or 'manual'
);

CREATE INDEX idx_skill_activations_skill ON skill_activations(skill_name, activated_at DESC);
CREATE INDEX idx_skill_activations_work_item ON skill_activations(work_item_id);
```

This enables:
- Tracking which skills are most/least used
- Identifying stale skills (no recent activations)
- Understanding which work types trigger which skills
- The awareness digest's "skills updated" section

### Skill Provenance Table (Postgres)

For autopoietic skills, track the link between ledger findings and skill content:

```sql
CREATE TABLE skill_provenance (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    skill_name      TEXT NOT NULL,
    skill_version   INTEGER NOT NULL,
    source_type     TEXT NOT NULL,     -- 'finding', 'pattern', 'manual'
    work_item_id    UUID REFERENCES work_items(id),
    ledger_seq      INTEGER,           -- which ledger entry
    content_snippet TEXT NOT NULL,      -- what was incorporated
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_skill_provenance_skill ON skill_provenance(skill_name, skill_version);
```

This is the audit trail for autopoietic learning: which work produced which knowledge, and how that knowledge became a skill.

---

## Configuration

### Global Skills Configuration

```toml
[skills]
dir = "skills"                      # relative to data_dir, or absolute path
auto_discovery = true               # scan for skills at startup
hot_reload = true                   # watch for skill changes during runtime
max_skill_prompt_tokens = 4000      # max tokens from all active skill prompts combined
```

### Per-Faculty Configuration

```toml
[faculty.engage]
# ... existing fields ...
auto_activate_skills = true         # orient auto-activates matching skills
max_auto_activated = 5              # limit auto-activated skills per focus

[faculty.consolidate]
# ... existing fields ...
skill_creation = true               # consolidate hook can create/update skills
skill_creation_threshold = 3        # minimum finding occurrences to trigger skill creation
```

---

## System Prompt Addition

Added to the engine's prompt template (after the Execution Modes section from `docs/engage.md`):

```
## Skills

You have access to skills — packaged knowledge and capabilities that extend
your core tools. Skills provide domain expertise, relationship context,
procedural guidance, and callable scripts.

Some skills have been auto-activated based on your current work — their
guidance appears below. You can discover more with `discover_skills` and
activate them with `activate_skill`.

When you learn something that would be useful in future work — a pattern,
a preference, a procedure — consider using `create_skill` to encode it.
Future foci will be able to discover and use what you learned.

### Active Skills

{auto_activated_skill_contexts}

### Available Skills

{skill_catalog_one_liners}
```

---

## OTel Integration

### Spans

```
work.execute
  ├── work.orient
  │     ├── work.awareness.digest
  │     └── work.skills.auto_activate          (auto-activation during orient)
  │           ├── work.skills.discover          (scan frontmatter for triggers)
  │           ├── work.skills.activate[skill-a] (load and inject)
  │           └── work.skills.activate[skill-b]
  ├── work.engage
  │     ├── work.engage.iteration[N]
  │     │     ├── work.tool.execute[discover_skills]   (manual discovery)
  │     │     ├── work.tool.execute[activate_skill]    (manual activation)
  │     │     └── work.tool.execute[create_skill]      (autopoietic creation)
  │     └── ...
  └── work.consolidate
        └── work.skills.consolidate_create      (consolidate-triggered skill creation)
```

### Metrics

| Metric | Type | Labels | Description |
|---|---|---|---|
| `work.skills.activated` | Counter | faculty, skill, type (auto/manual) | Skill activations |
| `work.skills.discovered` | Counter | faculty | Discovery queries |
| `work.skills.created` | Counter | faculty, created_by (animus/human) | Skills created |
| `work.skills.updated` | Counter | faculty, skill | Autopoietic skill updates |
| `work.skills.prompt_tokens` | Histogram | faculty | Total tokens from active skill prompts |
| `work.skills.stale` | Gauge | — | Skills not activated in 30 days |

---

## Interaction with Other Systems

### Skills and the Ledger

The ledger feeds skill creation:
- `finding` entries across multiple foci are the raw material for autopoietic skills
- The consolidate hook queries the ledger for recurring patterns
- Skill provenance links back to specific ledger entries

Skills improve ledger quality:
- An activated skill's instructions guide the agent to record better findings
- A "check-in-with-person" skill that says "record preferences in your ledger" produces structured findings that are easier to turn into future skills

### Skills and the Awareness Digest

The awareness digest surfaces skill evolution:
- Recently created/updated skills appear in the digest
- Every faculty sees what the system has learned, not just what it has done

Skills consume the awareness digest:
- A skill's instructions can reference the digest: "Check the awareness digest for recent interactions with this person from other faculties"

### Skills and the Code Execution Sandbox

Activated skills with `scripts/` directories are importable in the sandbox:
- Skill scripts extend the sandbox's capabilities beyond the core tool SDK
- Scripts can encode complex procedures (drafting messages, analyzing code, processing data)
- Scripts run with the same security model and resource limits as other sandbox code

### Skills and Child Work Items

Child work items can activate different skills than their parent:
- Parent (Social) activates `check-in-with-person` and `kelly-relationship`
- Parent spawns child (Computer Use) with `work_type: "analyze"`
- Child's orient auto-activates `analyze-codebase` — different skill set for different work
- This is natural: different work types trigger different skills, and child work has its own work type

---

## Open Questions

- **Skill size limits.** How large can a skill's prompt context be before it crowds out other context? The `max_skill_prompt_tokens` config caps total active skill prompt size, but individual skills could still be large. Should there be a per-skill limit? Or should the engine just warn when a skill is unusually large?

- **Skill versioning and rollback.** Autopoietic skills evolve as the agent learns. What if the agent learns something wrong and updates a skill with bad information? Git versioning of the skills directory provides rollback, but should the engine have explicit rollback support (e.g., `rollback_skill(name, version)`)? Or is this a human-intervention concern?

- **Skill testing for autopoietic skills.** Human-authored skills can include tests. But autopoietic skills are created by the agent — should the agent also write tests? This might be too ambitious for Level 2. Alternatively, the engine could validate autopoietic skills structurally (valid frontmatter, non-empty content, reasonable size) without requiring functional tests.

- **Skill sharing across animi.** In a fleet deployment, should skills be shareable? If one animus learns something valuable, can it propagate to others? This could use the shared observer infrastructure or a skill registry (like MicroClaw's ClawHub). But it raises questions about identity — should two animi have the same learned behaviors?

- **Skill deactivation.** Should the agent be able to deactivate a skill mid-focus? If it activated a skill that turned out to be irrelevant, the prompt context is wasted. A `deactivate_skill` tool would remove the skill's context from subsequent LLM calls. But modifying the system prompt mid-loop adds complexity. Simpler alternative: the agent just ignores the irrelevant skill, and the wasted tokens are acceptable.

- **Conflict between auto-activated skills.** If two skills declare `conflicts: ["each-other"]` but both match the work item's triggers, which one wins? Options: higher version, more specific trigger match, alphabetical, or ask the agent to choose. Probably: don't auto-activate conflicting skills — include both in the catalog and let the agent decide.

- **Skill dependency resolution.** If skill A `depends` on skill B, activating A should automatically activate B. This is straightforward for a flat dependency chain but could get complex with diamonds or version requirements. Keep it simple: flat dependencies only, no version constraints, error on circular dependencies.

- **Hybrid skills (Level 1.5).** Some skills are primarily runtime (prompt context) but also include a small system modification (register a custom tool). This is between Level 1 and Level 3. Should the engine support a `tools` section in the skill manifest that registers lightweight tools (e.g., a Python function exposed via the sandbox SDK) without full nanoclaw-style code modification?
