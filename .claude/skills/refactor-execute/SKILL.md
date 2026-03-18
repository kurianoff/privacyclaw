---
name: refactor-execute
description: Phase 3 of the refactor workflow. Executes every task from the Refactor-Plan task list using isolated worktrees. Per-task pipeline: Refactoring Engineer → Simplifier → Logging Implementer → Test Runner (behavior gate) → Contrarian (structure gate). Reverts tasks that fail after max iterations. Tracks completion in OpenSpec tasks.md. Can be invoked standalone by passing a Refactor-Plan Phase Handoff as the argument.
argument-hint: "<refactor-plan phase handoff> [RESUME_FROM: task-<id>]"
context: fork
---

# Phase 3 — Execute

You are the **Phase 3 coordinator**. Your job is to execute every task in the
Refactor-Plan task list — each one refactored, simplified, instrumented, test-green,
and Contrarian-approved — before returning to the refactor orchestrator.

Input: **$ARGUMENTS**

Extract from the input:
- `scope`: the code area being refactored (from Refactor-Plan handoff)
- `boundaries`: do-not-touch zones (from Refactor-Plan handoff)
- `branch`: the refactor branch (`refactor/<slug>`)
- `openspec_id`: the OpenSpec change id (`refactor-<slug>`, from Refactor-Plan handoff `OpenSpec` field)
- `tasks_path`: `openspec/changes/refactor-<slug>/tasks.md` (from Refactor-Plan handoff `Artifacts`)
- `for_next`: context from Refactor-Plan (parallel-safe tasks, risky tasks)
- `RESUME_FROM`: optional `task-<id>` — skip all tasks before this one

**Inject into every agent's context:** scope, boundaries, branch name.

---

## Critical: refactor log

**Maintain `.claude/workflow/<slug>/refactor-log.md`** throughout this phase.
After every task, append:

```text
### Task <id>: <title>
Status: complete | reverted | blocked
Branch: task/refactor-<slug>-<id>
Smells addressed: <list>
Changes made: <summary>
Test Runner iterations: <N>
Test Runner verdict: green | red
Contrarian rounds: <N>
Contrarian verdict: approved | challenged
Outcome: merged | reverted — <reason>
```

---

## Agent coordination protocol

For each task, try team-based coordination:

```text
TeamCreate({ name: "task-<id>-team",
             agents: ["refactoring-engineer", "simplifier",
                      "logging-implementer", "test-runner", "contrarian"] })
SendMessage({ to: "refactoring-engineer", message: "<task + context>" })
```

Fall back to sequential `Agent` tool calls if `TeamCreate` fails. Do not
retry teams more than once per task.

---

## Agent Handoff format

```text
--- AGENT HANDOFF ---
From:     <agent name>
To:       <next agent>
Status:   complete | blocked
Branch:   task/refactor-<slug>-<id>
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

Each task runs on a dedicated short-lived branch:

```bash
git worktree add ../worktree-refactor-<id> -b task/refactor-<slug>-<id>
```

- **Refactoring Engineer, Simplifier, Logging Implementer** commit
  sequentially. Use `isolation: "worktree"` for code-changing agent calls.
- **Test Runner and Contrarian** read only — do not commit.

**Merge checkpoint** — after Contrarian approves:

```bash
git checkout refactor/<slug>
git merge --no-ff task/refactor-<slug>-<id> -m "refactor(<id>): <title>"
git worktree remove ../worktree-refactor-<id>
```

**Revert checkpoint** — if abandoned after max iterations:

```bash
git worktree remove ../worktree-refactor-<id>
# branch abandoned — no merge
```

---

## Live progress reports

Emit a progress announcement to the user at two moments per task — start and
completion. Do NOT wait until the Phase Handoff to tell the user anything.

**Task start** (emit immediately at Step 1, before creating the worktree):

```
─── Task <id>/<total> starting: <title>
    Files: <list>
    Smells: <list of smell IDs being addressed>
```

**Task complete** (emit immediately at Step 7, after the merge commits):

```
━━━ Task <id>/<total> — <title>  [merged ✓ | reverted ✗ | blocked ⚠]
    Changed: <files changed, one per line>
    Test Runner: green in <N> iteration(s)
    Contrarian: approved in <N> round(s)
    Smells addressed: <list>
```

If reverted or blocked, replace the Test Runner / Contrarian lines with:

```
    Reason: <why it was reverted or blocked>
    Next: <what would unblock it, or "none">
```

**Running tally** — append after every task-complete report:

```
    Progress: <completed>/<total> tasks merged, <reverted> reverted, <blocked> blocked
```

---

## Task scheduling

Read `openspec/changes/<openspec_id>/tasks.md`. Build a dependency graph
from the `Depends on:` and `Parallel-safe:` fields in each task section:

- **Independent** tasks (`Parallel-safe: yes`, no overlapping files, no
  declared dependencies) → eligible for parallel execution
- **Dependent** tasks → wait until all declared dependencies are merged

**Source of truth for task status:** the `- [ ]`/`- [x]` checkbox on each
task's completion line in `tasks.md`. All agents read from and write to this
file — never from memory.

If `RESUME_FROM: task-<id>` is set: read `tasks.md` and skip every task
whose checkbox already reads `- [x]`. Start from task `<id>`.

---

## Per-task cycle

For each task (parallel where scheduling allows):

### Step 1 — Assign

**Emit task-start progress report.** Append to refactor log. Create worktree.
Pass to Refactoring Engineer: task `id`, `title`, `changes`, `criterion`,
`files`, branch name, scope, and boundaries. Read task fields from `<tasks_path>`.

### Step 2 — Refactoring Engineer

Invoke **refactoring-engineer** on `task/refactor-<slug>-<id>`. Task:

> Perform task `<id>`: `<title>`.
> Changes required: `<changes>`.
> Files: `<files>`.
> Verification criterion: `<criterion>`.
> Follow project conventions: Rust 2021, tokio, anyhow, thiserror.
> Do not change observable behavior. Do not touch: `<boundaries>`.
> Commit. Produce an Agent Handoff.

### Step 3 — Simplifier

Invoke **simplifier** with the RE handoff. Task:

> Review the changes on `task/refactor-<slug>-<id>`.
> Remove dead code, premature abstractions, unnecessary complexity
> introduced by the refactoring itself. Fix only real problems.
> Do not touch: `<boundaries>`. Commit. Produce an Agent Handoff.

### Step 4 — Logging Implementer

Invoke **logging-implementer** with the Simplifier handoff. Task:

> Retrofit every code path touched on `task/refactor-<slug>-<id>`
> with structured 5-level tracing per the project logging spec:
> - WARN: lifecycle events
> - INFO: atomic operations
> - DEBUG: every branch, raw data (truncated to 256 bytes), headers (auth redacted)
> Use structured fields (`key = %val`), never format strings.
> Never log inside a held Mutex lock.
> Do not touch: `<boundaries>`. Commit. Produce an Agent Handoff.

### Step 5 — Test Runner: behavior gate

Invoke **test-runner**. Task:

> Run `cargo test` against `task/refactor-<slug>-<id>`.
> Compare against `.claude/workflow/<slug>/baseline-tests.txt`.
> Verdict:
> - `green` — all previously-passing tests still pass
> - `red` — regressions (list each: test name, failure, likely cause)

**If green:** proceed to Step 6.

**If red:** pass back to **refactoring-engineer** to fix the regression,
then repeat Steps 3–5 (Simplifier → LogImpl → Test Runner).

**Maximum fix iterations: 3.** If still red after 3 full chain repeats:

```bash
git worktree remove ../worktree-refactor-<id>
```

In `openspec/changes/refactor-<slug>/tasks.md`, update the task's checkbox:
`- [ ] T<id> complete` → `- [ ] T<id> complete — ✗ REVERTED: <reason>`

Append to refactor log. **Emit task-complete progress report (reverted ✗)
with running tally.** Move to next task.

### Step 6 — Contrarian: structure gate

Invoke **contrarian** with the full handoff chain (Steps 2–5). Task:

> Review the complete changes on `task/refactor-<slug>-<id>`.
> Verify:
> - Verification criterion met: `<criterion>`
> - Residual smells the task was supposed to address
> - Every touched path correctly instrumented per the logging spec
> - No boundaries in `<boundaries>` were touched
>
> Verdict: approved or challenged.
> Per challenge:
> - `[SMELL]` — smell not addressed → route to RE, restart from Step 2
> - `[LOGGING]` — instrumentation gap → route to LogImpl, restart from Step 4
> - `[BEHAVIOR]` — behavior change risk → route to RE, restart from Step 2,
>   must re-run Test Runner

**Maximum Contrarian rounds: 3.** If not approved: in
`openspec/changes/refactor-<slug>/tasks.md` update the task's checkbox:
`- [ ] T<id> complete` → `- [ ] T<id> complete — ⚠ BLOCKED: <reason>`

Append to refactor log. **Emit task-complete progress report (blocked ⚠)
with running tally.** Surface in Phase Handoff `Open`. Do not merge.

### Step 7 — Merge, clean up team, log

After Contrarian approval:

**Emit task-complete progress report** (merged ✓) with running tally.

1. In `openspec/changes/refactor-<slug>/tasks.md`, mark the task done:
   `- [ ] T<id> complete` → `- [x] T<id> complete`
2. Shut down task team:
   ```text
   SendMessage({ to: "refactoring-engineer", message: {type: "shutdown_request"} })
   SendMessage({ to: "simplifier",           message: {type: "shutdown_request"} })
   SendMessage({ to: "logging-implementer",  message: {type: "shutdown_request"} })
   SendMessage({ to: "test-runner",          message: {type: "shutdown_request"} })
   SendMessage({ to: "contrarian",           message: {type: "shutdown_request"} })
   TeamDelete()
   ```
3. Merge task branch into `refactor/<slug>`.
4. Append to refactor log.
5. Move to next task.

---

## Final validation

After all tasks are processed (before team cleanup):

```bash
cargo test 2>&1 | tee .claude/workflow/<slug>/final-tests.txt
openspec validate <openspec_id> --strict
```

Compare cargo test output against baseline. If any previously-passing test now
fails, surface to orchestrator with the diff. Do not produce a complete Phase
Handoff until this is resolved or the user explicitly accepts the failure.

Confirm all tasks in `<tasks_path>` are marked `- [x]`. If any blocked or
reverted tasks remain with `- [ ]`, the Phase Handoff `Open` field must list them.

---

## Team cleanup (safety net)

Ensure all open task teams are cleaned up before producing the Phase Handoff.

---

## Phase completion

Produce a **Phase Handoff**:

```text
=== PHASE HANDOFF ===
Phase:     Refactor-Execute
Status:    complete  (or: blocked — <reason>)
Scope:     <scope>
Branch:    <branch>
OpenSpec:  <openspec_id>
Artifacts:
  openspec/changes/<openspec_id>/tasks.md  (updated with completion status)
  .claude/workflow/<slug>/refactor-log.md
  .claude/workflow/<slug>/baseline-tests.txt
  .claude/workflow/<slug>/final-tests.txt
Decisions: <key implementation decisions per task>
For next:  <what follow-on work needs: what changed structurally, what was
            reverted or blocked, areas that may need further attention.
            Run `openspec archive <openspec_id> --yes` after the branch is merged.>
Open:
  - <blocked tasks with reason>
  - <reverted tasks with reason>
  - (or "none")
=== END HANDOFF ===
```
