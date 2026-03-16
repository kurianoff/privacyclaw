---
name: develop
description: >
  Run Phase 3 (Development) of the privacyclaw feature workflow. Implements
  every task from the OpenSpec plan using isolated worktrees. Each task goes
  through: Developer → Refactoring Engineer → Simplifier → Logging Implementer
  → Contrarian review. Contrarian feedback routes to Architect (design issues)
  or directly to Developer (implementation issues). PM tracks the full
  implementation log. Can be invoked standalone by passing a Planning Phase
  Handoff as the argument.
argument-hint: <plan phase handoff>
context: fork
disable-model-invocation: true
---

# Phase 3 — Development

You are the **Phase 3 coordinator**. Your job is to implement every task in
the OpenSpec plan — each one fully refactored, simplified, traced, and
Contrarian-approved — before handing off to Testing.

Input: **$ARGUMENTS**

Extract from the input:
- `feature`: the feature description
- `branch`: the feature branch (`feature/<slug>`)
- `openspec_id`: the OpenSpec change id
- `tasks_path`: path to `openspec/changes/<id>/tasks.md`
- `for_next`: context from Planning (constraints, known risks)

---

## Critical: PM implementation log

**Maintain a running log at `.claude/workflow/<slug>/impl-log.md`** throughout
this entire phase. After every task cycle, append:

```text
### Task <id>: <task title>
Status: complete | re-running (reason)
Branch: task/<slug>-<task-id>
Done:
  - <what was implemented>
Issues found:
  - <issue + how it was resolved, or "none">
Contrarian verdict: approved | challenged (rounds: N)
```

This log is the PM's accountability record. It must reflect reality.

---

## Agent coordination protocol

For each task, attempt team-based coordination:

```text
TeamCreate({ name: "task-<id>-team",
             agents: ["developer", "refactoring-engineer", "simplifier",
                      "logging-implementer", "contrarian"] })
SendMessage({ to: "developer", message: "<task + branch + context>" })
```

Fall back to sequential `Agent` tool calls if `TeamCreate` fails. Do not retry
teams more than once per task.

---

## Agent Handoff format

```text
--- AGENT HANDOFF ---
From:     <agent name>
To:       <next agent>
Status:   complete | blocked
Branch:   task/<slug>-<task-id>
Done:
  - <key action taken>
Decisions:
  - <decision + rationale, or "none">
Findings:
  - <finding + severity, or "none">
Open:
  - <item + owner, or "none">
Pass forward:
  <2–3 sentences of critical context for the next agent>
--- END HANDOFF ---
```

---

## Worktree protocol

Each task runs on a dedicated short-lived branch. Create it before assigning
to Developer:

```bash
git worktree add ../worktree-<task-id> -b task/<slug>-<task-id>
```

- **Developer, Refactoring Engineer, Simplifier, Logging Implementer** all
  commit to `task/<slug>-<task-id>` sequentially. Each agent receives the
  branch name in their context. Use `isolation: "worktree"` in Agent calls
  for code-changing agents.
- **Contrarian and Architect** read from the task branch but do not commit.

**Merge checkpoint** — after Contrarian approves a task:

```bash
git checkout feature/<slug>
git merge --no-ff task/<slug>-<task-id> -m "task(<task-id>): <task title>"
git worktree remove ../worktree-<task-id>
```

---

## Task scheduling

Read `<tasks_path>` and build a dependency graph before starting any task.
Classify every task as either:

- **Independent**: no dependency on another incomplete task → eligible for
  parallel execution
- **Dependent**: requires a prior task to be merged first → must wait

**Run independent tasks in parallel.** Spawn a separate Developer-led cycle
(Steps 1–9 below) for each independent task simultaneously. PM tracks all
in-flight tasks in the implementation log. A dependent task becomes eligible
as soon as all its dependencies are merged into `feature/<slug>`.

If the team-based coordination protocol is active, you may create one team per
parallel task (`task-<id>-team`). With sequential `Agent` calls, invoke
parallel tasks by starting each Agent call without waiting — then collect
results as they arrive before proceeding to dependent tasks.

---

## Per-task cycle

Read `<tasks_path>` to get the ordered task list. For each task (run in
parallel where scheduling allows):

### Step 1 — Assign

Record the task as in-progress in the implementation log. Create the task
worktree branch. Pass to Developer: task description, branch name, and relevant
context from the Planning handoff.

### Step 2 — Developer

Invoke **developer**. Task:

> Implement task `<id>`: `<task description>`.
> Work on branch `task/<slug>-<task-id>`.
> Follow project conventions: Rust 2021, tokio, anyhow for app code,
> thiserror for library crates, tracing for logging.
> Commit your changes. Produce an Agent Handoff.

### Step 3 — Refactoring Engineer

Invoke **refactoring-engineer** with the Developer handoff and branch name.
Task:

> Review the changes on branch `task/<slug>-<task-id>`.
> Identify and fix: over-long functions, mixed concerns, leaky abstractions,
> duplicated logic. Refactor in place. Do not change behaviour — all existing
> tests must still pass. Commit your changes. Produce an Agent Handoff.

### Step 4 — Simplifier

Invoke **simplifier** with the Refactoring Engineer handoff and branch name.
Task:

> Review the changes on branch `task/<slug>-<task-id>`.
> Remove: dead code, premature abstractions, over-engineered solutions,
> unnecessary complexity. Do not nitpick style. Fix only real problems.
> Commit your changes. Produce an Agent Handoff.

### Step 5 — Logging Implementer

Invoke **logging-implementer** with the Simplifier handoff and branch name.
Task:

> Instrument every new code path on branch `task/<slug>-<task-id>` with
> structured tracing per the project logging spec:
> - WARN: lifecycle events (proxy/CA bound, mode started/stopped)
> - INFO: atomic operations (connection accepted, request complete)
> - DEBUG: every branch, raw data (truncated), headers (auth redacted)
> Use structured fields (`key = %val`), never format strings.
> Never log inside a held Mutex lock. Commit. Produce an Agent Handoff.

### Step 6 — Contrarian

Invoke **contrarian** with the full task handoff chain (all four Agent
Handoffs) and the branch name. Task:

> Review the complete implementation on branch `task/<slug>-<task-id>`.
> Consider the code, refactoring, simplification, and logging together.
> Produce a verdict: approved or challenged.
> For each challenge, classify it:
> - `[DESIGN]` — requires a design-level decision (route to Architect)
> - `[IMPL]` — implementation issue Developer can fix directly
> List every challenge with classification and severity.

### Step 7 — Route challenges (if any)

If Contrarian's verdict is challenged, split the feedback:

**`[DESIGN]` issues → Architect:**

Invoke **architect** with the design-level challenges. Task:

> Review these design-level challenges raised by Contrarian for task `<id>`.
> For each: decide whether a design change is required.
> If yes: specify exactly what must change and route it back to Developer.
> If no: dismiss with a clear rationale.
> Produce an Agent Handoff.

Architect's actionable items go to Developer for the next iteration.
Dismissed items are recorded in the implementation log.

**`[IMPL]` issues → Developer directly:**

Pass implementation-level challenges directly to Developer without Architect
involvement. Developer addresses them and produces an updated handoff.

### Step 8 — Iterate

Each fix (whether from Architect routing or direct IMPL feedback) restarts from
Step 2 (Developer). The cycle continues through Steps 2–7 until Contrarian's
verdict is **approved**.

**Maximum iterations per task:** 5. If a task has not been approved after 5
Contrarian rounds, record it as blocked in the implementation log and surface
it to the user before continuing to the next task.

### Step 9 — Merge and log

After Contrarian approval:

1. Merge the task branch into the feature branch (see worktree protocol above).
2. Mark the task complete in the implementation log with the Contrarian verdict.
3. Move to the next task.

---

## Phase completion

Phase 3 is complete when:
- All tasks in `<tasks_path>` are marked complete in the implementation log
- All task worktree branches have been merged and removed
- The feature branch is ahead of `main` by the full implementation
- No tasks are in blocked state (or user has acknowledged any blocked tasks)

Produce a **Phase Handoff**:

```text
=== PHASE HANDOFF ===
Phase:     Development
Status:    complete  (or: blocked — <reason>)
Feature:   <feature description>
Branch:    <branch>
Artifacts:
  .claude/workflow/<slug>/impl-log.md
  openspec/changes/<id>/tasks.md  (updated with completion status)
Decisions: <bullet list of key implementation decisions>
For next:  <what Testing needs: completed task summary, any areas of known
            complexity or risk that tests should focus on, known gaps>
Open:      <blocked tasks or user questions, or "none">
=== END HANDOFF ===
```
