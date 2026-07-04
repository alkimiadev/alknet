# Spec-Driven Development Process

## Overview

This document defines the SDD process for the @alkdev/storage package. It
leverages:

- **OpenCode CLI** as the agent execution environment
- **Open-coordinator plugin** for worktree management and parallel session
  orchestration
- **Structured task graphs** with dependency analysis and safe exit protocols

## Core Principles

1. **Specification First**: Invest in architecture before implementation
2. **Roles as Modes**: Same agent adopts different behavioral modes
3. **Flexible Self**: Agents can implement, self-review, and fix objectively
4. **Task-Driven**: Structured task graphs with dependency analysis
5. **Safe Exit**: Always have a way to unblock progress when stuck
6. **Categorical Estimates**: Use risk/scope/impact categories, not time
   estimates. These are structurally important — upstream failures multiply
   downstream damage regardless of developer type (human or LLM). See the
   cost-benefit framework in taskgraph's framework docs.

## Workflow Phases

### Phase 0: Exploration (Conditional)

**When**: Requirements unclear, multiple approaches to evaluate, or hard
problems need investigation.

**Process**:

1. Capture vision and guiding principles
2. Research Specialist investigates options (`docs/research/` or external)
3. POC Specialist validates promising approaches (`.worktrees/research/`)
4. Document learnings
5. Converge on recommended approach

**Output**: Clear understanding of WHAT to build and WHY, with validated
approaches

### Phase 1: Architecture

**Objective**: Produce comprehensive, committed architecture specification.

**Process**:

1. Architect creates the architecture documentation structure:
   - `docs/architecture/README.md` — index with doc table, ADR table, lifecycle
   - `docs/architecture/overview.md` — package purpose, exports, dependencies
   - `docs/architecture/<component>.md` — one focused doc per component/area
   - `docs/architecture/decisions/` — numbered ADR files (ADR-001, ADR-002, ...)
   - `docs/architecture/open-questions.md` — centralized tracker with OQ-IDs
2. All docs start in `Draft` status. Spec docs reference ADRs by number (not
   inline rationale) and OQs by number (not inline questions).
3. Architecture Review validates for ambiguities, risks, and structural issues
   (inline decisions not extracted, missing ADRs, no README index)
4. Iterate until zero critical issues
5. Transition to `Reviewed` status when all open questions for a doc are resolved

**Output**: Reviewed architecture documents ready for decomposition

**Key pattern**: Decisions go in `decisions/` ADRs, not inline in specs.
Open questions go in `open-questions.md`, not scattered per-doc. Specs describe
WHAT, ADRs explain WHY, open questions track what's unresolved.

### Phase 2: Decomposition

**Objective**: Break architecture into atomic, dependency-ordered tasks.

**Process**:

1. Decomposer analyzes architecture
2. Creates tasks (markdown files in `tasks/`)
3. Establishes dependencies between tasks
4. Validates structure (no cycles, logical ordering)
5. Identifies review injection points

**Output**: Well-structured task graph in `tasks/` directory

### Phase 3: Implementation

**Objective**: Execute tasks in dependency order with verification.

**Process**:

1. Coordinator identifies parallelizable work
2. Coordinator spawns worktrees + sessions (via
   `worktree({action: "spawn", ...})` or hub `coord.spawn` when available)
   - Feature work: `.worktrees/feat/<task-id>/` → Implementation Specialist
   - Research POCs: `.worktrees/research/<task-id>/` → POC Specialist
3. Coordinator injects task context into each session
4. Agents execute tasks with self-verification
5. On completion: agent notifies coordinator, updates task status, commits to
   worktree branch
6. On blocker: Safe Exit protocol, agent notifies coordinator, create blocker
   task
7. Merge worktrees back to main when complete

**Output**: Completed, verified implementation

### Phase 4: Review & Finalization

**Objective**: Validate quality and readiness.

**Process**:

1. Code review at injected checkpoints
2. Final integration testing
3. Architecture sync check
4. Deployment preparation

**Output**: Production-ready codebase

## Roles

### Primary Roles

#### 1. Architect

**Responsibility**: Create and maintain architecture specifications.

**Mode**: Primary (interactive with user)

**Tools**:

- Read, Write, Edit, Glob, Grep
- webSearch (research patterns, best practices)

**Key Behaviors**:

- Focus on WHAT and WHY, never HOW
- Extract decisions into numbered ADRs in `decisions/` directory
- Centralize open questions in `open-questions.md` with OQ-IDs
- Maintain `README.md` as the architecture index
- Keep spec documents focused (~500 lines) — reference ADRs and OQs, don't inline them
- Redirect exploration work to Research Specialist
- Iterate based on review feedback

**Deliverables**:

- `docs/architecture/README.md` — index with doc table, ADR table, lifecycle
- `docs/architecture/<component>.md` — focused spec documents
- `docs/architecture/decisions/` — numbered ADR files
- `docs/architecture/open-questions.md` — centralized OQ tracker

---

#### 2. Decomposer

**Responsibility**: Transform architecture into atomic task graph.

**Mode**: Primary (interactive with user for approval)

**Tools**:

- Read, Glob, Grep

**Key Behaviors**:

- Decompose to atomic tasks (single objective, clear acceptance criteria)
- Establish logical dependencies
- Validate structure (no cycles, logical ordering)
- Inject review tasks at critical points

**Deliverables**:

- Task files in `tasks/` directory
- Dependency graph validated

---

#### 3. Coordinator

**Responsibility**: Orchestrate parallel task execution across worktrees and
sessions.

**Mode**: Primary (manages worktrees and agent sessions)

**Uses**: The `worktree` tool from the **open-coordinator** opencode plugin.
Single tool with `{action, args}` dispatch. Role is auto-detected — coordinator
sessions get the full operation set, spawned implementation sessions get a
limited set (current, notify, status). No mode toggle required.

**Tools**:

- `worktree({action, args})` — spawn, sessions, dashboard, message, abort,
  cleanup
- Bash (opencode CLI for session interaction)
- Read (monitor task files)
- `memory` / `memory_compact` — context management and session history (via
  @alkdev/open-memory, when available)

**Key Behaviors**:

- Identify parallelizable task groups
- Spawn worktrees + sessions via `worktree({action: "spawn", ...})`
- Inject task context into sessions
- Monitor progress via `worktree({action: "sessions"})` and dashboard
- Handle blocked tasks (escalate or reassign)
- Merge completed worktrees

**Deliverables**:

- Coordinated parallel execution
- Blocked task escalation
- Merged branches

---

#### 4. Implementation Specialist

**Responsibility**: Execute atomic tasks with self-verification.

**Mode**: Primary (works on assigned task in worktree)

**Tools**:

- Read, Write, Edit, Glob, Grep, Bash
- `worktree({action: "notify", ...})` — report progress/blockers to coordinator
- `worktree({action: "current"})` — verify worktree assignment
- webSearch (documentation lookup)
- `memory` / `memory_compact` — context management (via @alkdev/open-memory,
  when available)

**Key Behaviors**:

- Load task context (architecture, dependencies)
- Propose plan before implementing
- Implement following architecture constraints
- Self-verify against acceptance criteria
- Use Safe Exit when blocked
- Notify coordinator via worktree tool
- Commit to worktree branch

**Deliverables**:

- Completed task implementation
- Tests passing
- Committed changes in worktree

---

### Reviewer Roles

#### 5. Architecture Reviewer

**Responsibility**: Validate architecture for ambiguities, risks, and structural issues.

**Mode**: Subagent (invoked by Architect)

**Tools**:

- Read, Grep

**Key Behaviors**:

- Check for inline decision rationale that should be in ADRs
- Check for inline open questions that should be in `open-questions.md`
- Check for missing ADR references for visible design choices
- Identify undefined terms or concepts
- Identify missing trade-off documentation
- Validate quality attribute coverage
- Flag ambiguities that could cause implementation issues
- Check document size (recommend split if >700 lines)
- Verify README has complete doc table and ADR table
- Verify cross-references between docs, ADRs, and OQs are correct

---

#### 6. Code Reviewer

**Responsibility**: Review code quality at checkpoints.

**Mode**: Subagent (invoked by Coordinator or as task)

**Tools**:

- Read, Grep, Bash (lint, test)

**Key Behaviors**:

- Check adherence to architecture
- Validate patterns and conventions
- Run linters and tests
- Identify security/performance concerns

---

#### 7. Research Specialist

**Responsibility**: Research documentation, libraries, best practices.

**Mode**: Subagent (invoked by any role)

**Tools**:

- Read, Write, Glob
- webSearch (primary research tool)

**Key Behaviors**:

- Find and summarize documentation
- Evaluate library alternatives
- Document findings

---

#### 8. POC Specialist

**Responsibility**: Create proof-of-concepts to validate technical approaches
before production implementation.

**Mode**: Primary (works in isolated research worktree)

**Worktree Location**: `.worktrees/research/<task-id>/`

**Tools**:

- Read, Write, Edit, Glob, Grep, Bash
- webSearch (implementation references)

**Key Behaviors**:

- Create minimal POCs to validate hypotheses
- Work in isolated research worktrees
- Document findings and recommendations
- Timebox strictly - abandon if taking too long
- Be honest about limitations and blockers

**When Invoked**:

- After Research Specialist completes initial research
- When a technical approach needs validation before commitment
- When integration complexity or performance is uncertain

**Deliverables**:

- Working POC code
- Findings document with recommendation (proceed/pivot/block)
- Updated research task with results

---

## Task File Format

Tasks are markdown files stored in `tasks/`. Since they're in the repo, they're
automatically available in worktrees.

```markdown
---
id: auth-setup
name: Setup Authentication
status: pending
depends_on: []
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement OAuth2 authentication with provider abstraction.

## Acceptance Criteria

- [ ] OAuth2 flow works with Google provider
- [ ] Tokens stored securely
- [ ] Session management implemented

## References

- docs/architecture/auth.md

## Notes

> Agent fills this during implementation. Document any decisions, deviations
> from architecture, or relevant context discovered.

## Summary

> Agent fills this on completion. Brief description of what was implemented,
> files changed, and any follow-up needed.
```

### Categorical Estimates

These fields are structurally important, not optional metadata. They power
`taskgraph decompose`, `risk-path`, `critical`, and `bottleneck` — commands that
reveal structural problems in the task graph. A task missing `scope`, `risk`,
`impact`, or `level` is a red flag indicating incomplete decomposition. See the
cost-benefit framework in taskgraph's framework docs for the reasoning.

| Scope    | Description                  | Example                   |
| -------- | ---------------------------- | ------------------------- |
| single   | One function, one file       | Add validation helper     |
| narrow   | One component, few files     | Implement auth middleware |
| moderate | Feature, multiple components | Build user API endpoints  |
| broad    | Multi-component feature      | Implement OAuth flow      |
| system   | Cross-cutting changes        | Database migration        |

| Risk     | Failure Likelihood        |
| -------- | ------------------------- |
| trivial  | Nearly impossible to fail |
| low      | Standard implementation   |
| medium   | Some uncertainty          |
| high     | Significant unknowns      |
| critical | High chance of failure    |

### Task Lifecycle

**Status values**: `pending` → `in-progress` → `completed` | `blocked` |
`failed`

**On completion**, the agent:

1. Updates `status: completed`
2. Fills in `## Summary` section
3. Commits changes to worktree branch

## Safe Exit Protocol

When a task becomes untendable:

### Criteria

**Hard Criteria** (automatic):

- Same task fails verification 3+ times
- Task attempts exceed 5+ total

**Soft Criteria** (agent judgment):

- Ambiguous architecture
- Missing dependencies
- External library incompatibility
- Scope creep detected

### Process

1. Create blocker task
2. Update original task: `status: blocked`, add blocker to `depends_on`
3. Document in task notes
4. Notify coordinator

## Review Injection

Use graph analysis to determine where reviews should happen:

| Analysis         | Injection Point              |
| ---------------- | ---------------------------- |
| Parallel groups  | Review before groups merge   |
| Bottleneck tasks | Review before critical path  |
| High-risk tasks  | Review before proceeding     |
| Critical path    | Review before critical tasks |

## Coordinator Implementation

### Current (open-coordinator plugin)

The Coordinator uses the `worktree` tool from the open-coordinator opencode
plugin. It's a single tool with `{action, args}` dispatch — no separate
enable/toggle steps. Role is auto-detected from session state.

```
1. Identify parallel work
   Read task files → groups of independent tasks

2. Spawn worktrees + sessions
   worktree({action: "spawn", args: {
     tasks: ["auth-setup", "db-schema", "api-routes"],
     prefix: "feat/",
     agent: "implementation-specialist",
     prompt: "Your task: {{task}}. Read tasks/{{task}}.md for details."
   }})

3. Monitor progress
   worktree({action: "sessions"})     → status of all spawned sessions
   worktree({action: "dashboard"})    → worktree + session overview

4. Handle issues
   - Recovery message: worktree({action: "message", args: {sessionID: "ses_...", message: "..."}})
   - Abort if unrecoverable: worktree({action: "abort", args: {sessionID: "ses_..."}})

5. Handle completion
   - Agent commits to worktree branch
   - Agent notifies via worktree({action: "notify", ...})
   - Coordinator merges back to main

6. Cleanup
   worktree({action: "cleanup", args: {action: "remove", pathOrBranch: "feat/auth-setup"}})
```

The plugin also provides SSE-based anomaly detection (model degradation, high
error count, session stall) with automatic notifications to the coordinator.

### Implementation Agent Operations

Spawned sessions (implementation specialists, code reviewers, POC specialists)
get a limited worktree interface:

```text
worktree({action: "current"})                              → Show worktree mapping
worktree({action: "notify", args: {message: "...", level: "info|blocking"}})  → Report to coordinator
worktree({action: "status"})                                 → Show worktree git status
```

The plugin auto-injects `workdir` for bash commands when a session is mapped to
a worktree.

### Context & Memory (with @alkdev/open-memory)

When the open-memory plugin is available alongside open-coordinator, the
coordinator gains:

- `memory({tool: "children", args: {sessionId: "..."}})` — view sub-agent
  sessions spawned from the coordinator
- `memory({tool: "messages", args: {sessionId: "..."}})` — read a spawned
  session's conversation for debugging
- `memory({tool: "context"})` — check context window usage before long
  monitoring sessions
- `memory_compact()` — proactively compact at natural breakpoints

Implementation agents can also use `memory({tool: "context"})` and
`memory_compact()` to manage their context during long tasks.

### Future (Hub Operations)

Once the hub is operational, coordination uses native operations:

```
1. Identify parallel work
   hub.call("coord.spawn", { task, branch, ... })

2. Monitor progress
   hub.call("coord.status", { parentSessionId })

3. Message sessions
   hub.call("coord.message", { sessionId, message })

4. Handle aborts
   hub.call("coord.abort", { sessionId })
```

State moves from in-process tracking to Postgres `mappings` table. The
open-coordinator plugin becomes unnecessary — the hub provides the same
capabilities as server-side operations accessible from any environment.

## Document Structure

```
.opencode/
├── agents/
│   ├── architect.md
│   ├── decomposer.md
│   ├── coordinator.md
│   ├── implementation-specialist.md
│   ├── poc-specialist.md
│   ├── code-reviewer.md
│   ├── architecture-reviewer.md
│   └── research-specialist.md

docs/
├── architecture/
│   ├── README.md              # Index: doc table, ADR table, lifecycle definitions
│   ├── overview.md            # Package purpose, exports, dependencies
│   ├── <component>.md         # One focused doc per component/area
│   ├── open-questions.md      # Centralized OQ tracker with IDs, priorities, status
│   └── decisions/             # Numbered ADRs
│       ├── 001-<slug>.md
│       ├── 002-<slug>.md
│       └── ...
├── sdd_process.md             # This document

tasks/
├── architecture/
│   └── ...
├── implementation/
│   └── ...
└── (taskgraph validates & analyzes dependency graph)

.worktrees/                    # Created by coordinator
├── feat/
│   ├── api-auth/
│   └── api-users/
└── research/
    └── storage-abstraction/
```

### ADR Format

ADRs follow this template:

```markdown
# ADR-NNN: Descriptive Title

## Status
Accepted | Proposed | Deprecated | Superseded

## Context
(Why this decision is needed)

## Decision
(What was decided)

## Consequences
(Positive and negative outcomes)

## References
(Links to related specs and ADRs)
```

Numbering starts at 001 within each project. ADRs are stable — once Accepted,
they don't revert. If superseded, mark the old one and create a new one.

### Open Questions Format

`open-questions.md` organizes questions by theme:

```markdown
## Theme 1: <Theme Name>

### OQ-NN: <Question>

- **Origin**: [spec-doc.md]
- **Status**: open | resolved | deferred(scope) | partially resolved
- **Priority**: high | medium | low
- **Resolution**: (when resolved)
- **Blocked on**: (when deferred — concrete dependency that would unblock)
- **Cross-references**: OQ-NN, ADR-NNN
```

### Spec Document Format

Spec documents reference ADRs and OQs by number, not by inlining rationale or
questions. The Design Decisions section is a reference table:

```markdown
## Design Decisions

All design decisions are documented as ADRs in [decisions/](decisions/).

| ADR | Decision | Summary |
|-----|----------|---------|
| [001](decisions/001-slug.md) | Decision title | One-line summary |
```

The Open Questions section is also a reference:

```markdown
## Open Questions

Open questions are tracked in [open-questions.md](open-questions.md). Key
questions affecting this document:

- **OQ-01**: Question summary (status)
- **OQ-03**: Question summary (status)
```

## Agent Role Specs

Agent definitions are in `.opencode/agents/`:

- **architect.md** - Creates architecture specifications
- **decomposer.md** - Transforms architecture to task graph
- **coordinator.md** - Orchestrates parallel execution
- **implementation-specialist.md** - Executes tasks with self-verification
- **poc-specialist.md** - Creates proof-of-concepts for validation
- **code-reviewer.md** - Reviews code quality at checkpoints
- **architecture-reviewer.md** - Validates architecture specs
- **research-specialist.md** - Researches and documents findings

Use with opencode CLI:

```bash
# Spawn coordinator in interactive mode
opencode --agent coordinator

# Send task to implementation specialist
opencode run -s <session-id> --agent implementation-specialist "Your task: auth-setup"
```

## Evolution

This document should evolve with the project:

1. Refine roles based on actual usage
2. Adjust task templates based on what works
3. Document coordinator patterns as they emerge
4. Capture learnings in after-action reviews
