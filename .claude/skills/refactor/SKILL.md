---
name: refactor
description: Systematically improve code structure in a targeted scope without changing behavior. Investigator catalogs smells → Contrarian challenges the catalog → Architect plans tasks → Contrarian challenges the plan → per-task cycle (Refactoring Engineer → Simplifier → Logging Implementer → Test Runner → Contrarian) with revert protocol if behavior breaks. Standalone skill — not part of the implement flow.
context: fork
argument-hint: <scope description> [DO NOT TOUCH: <comma-separated boundaries>]
---

# Refactor

You are the **refactor coordinator**. Your job is to systematically improve the
structure of a targeted scope of code — without changing observable behavior —
and to leave every touched path fully instrumented with 5-level structured
logging.

Input: **$ARGUMENTS**

Extract from the input:
- `scope`: the code area to refactor (module, file, directory, or feature area)
- `boundaries`: explicit "do not touch" zones (e.g. public API signatures, wire
  protocol, serialization format). Record as `none` if omitted.

Derive a slug from the scope (lowercase, hyphens, max 40 chars).

**Inject into every agent's context throughout this skill:**
- The `scope` being refactored
- The `boundaries` (do not touch list)
- The branch name (`refactor/<slug>`)

---

## Critical: refactor log

**Maintain a running log at `.claude/workflow/<slug>/refactor-log.md`**
throughout this entire skill. After every task cycle, append:

```text
### Task <id>: <task title>
Status: complete | reverted | blocked
Branch: task/refactor-<slug>-<task-id>
Smells addressed:
  - <smell + location>
Changes made:
  - <what changed>
Test Runner iterations: <N>
Test Runner verdict: green | red
Contrarian rounds: <N>
Contrarian verdict: approved | challenged
Outcome: merged | reverted — <reason>
```

---

## Agent coordination protocol

Try team-based coordination first:

```text
TeamCreate({ name: "refactor-team",
             agents: ["investigator", "architect", "contrarian",
                      "refactoring-engineer", "simplifier",
                      "logging-implementer", "test-runner"] })
SendMessage({ to: "investigator", message: "<task + context>" })
```

Use `SendMessage` to pass Agent Handoffs between agents. Fall back to
sequential `Agent` tool calls if `TeamCreate` fails. Do not retry teams
more than once.

---

## Agent Handoff format

```text
--- AGENT HANDOFF ---
From:     <agent name>
To:       <next agent>
Status:   complete | blocked
Branch:   <branch name>
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
git worktree add ../worktree-refactor-<task-id> -b task/refactor-<slug>-<task-id>
```

- **Refactoring Engineer, Simplifier, Logging Implementer** commit to the
  task branch sequentially. Use `isolation: "worktree"` for code-changing
  agent calls.
- **Test Runner and Contrarian** read from the task branch but do not commit.

**Parallel task conflict rule:** before starting parallel tasks, check that
no two parallel tasks list overlapping files. Tasks that touch the same file
must be sequenced (the second depends on the first), not parallelized. Record
this in the dependency graph.

**Merge checkpoint** — after Contrarian approves:

```bash
git checkout refactor/<slug>
git merge --no-ff task/refactor-<slug>-<task-id> -m "refactor(<task-id>): <task title>"
git worktree remove ../worktree-refactor-<task-id>
```

**Revert checkpoint** — if a task is abandoned:

```bash
git worktree remove ../worktree-refactor-<task-id>
# branch abandoned — no merge
```

---

## Task scheduling

After the task list is finalized (Step 6), build a dependency graph:

- **Independent**: no dependency on another incomplete task AND no overlapping
  files with other in-flight tasks → eligible for parallel execution
- **Dependent**: requires a prior task to be merged first, or overlapping
  files with another in-flight task → must wait

A dependent task becomes eligible as soon as all its dependencies are merged
into `refactor/<slug>`.

---

## Workflow

### Step 1 — Git setup and baseline

```bash
git checkout main && git pull
git checkout -b refactor/<slug>
mkdir -p .claude/workflow/<slug>
cargo test 2>&1 | tee .claude/workflow/<slug>/baseline-tests.txt
```

Record the baseline in the refactor log header:

```text
## Baseline
Branch: refactor/<slug>
Scope: <scope>
Boundaries: <boundaries or none>
Baseline tests: <pass/fail summary>
Pre-existing failures: <list, or none>
```

**If the baseline is completely broken** (majority of tests fail), surface
to the user and halt. Do not refactor into a broken baseline.

### Step 2 — Investigator: smell catalog

Invoke **investigator** with scope, boundaries, and baseline. Task:

> Read every file in scope: `<scope>`.
> Catalog every structural problem you find:
> - Over-long functions (> ~50 lines of logic)
> - Mixed concerns (a function or module doing more than one thing)
> - Leaky abstractions (internals exposed unnecessarily)
> - Duplicated logic (same pattern repeated in two or more places)
> - Under-instrumented code paths (branches with no tracing calls)
> - `#[allow(deprecated)]` suppressions and `// TODO`/`// FIXME` comments
>   related to structure or debt
> - Overly complex control flow (deep nesting, long match arms)
>
> For each smell, record: location (file:line), severity (high/medium/low),
> one-line description, and which boundary (if any) it is adjacent to.
>
> Do NOT propose fixes. Do NOT touch any code in `<boundaries>`.
> Produce an Agent Handoff with the full smell catalog, ordered by severity.

**If the smell catalog is empty** (no smells found): produce a Phase Handoff
with `Status: complete`, note that no structural issues were found, and stop.
Do not proceed further.

### Step 3 — Contrarian: challenge the smell catalog

Invoke **contrarian** with the Investigator handoff. Task:

> Review the smell catalog. Challenge:
> - Smells classified as high-severity that may not warrant the risk of
>   refactoring (is the benefit worth the disruption?)
> - Smells the Investigator missed — are there structural problems not listed?
> - Any smell adjacent to a boundary in `<boundaries>` — is it actually safe
>   to touch, or should it be excluded?
> - Smells that are interdependent (fixing one requires fixing another first)
>
> Produce an Agent Handoff with:
> - Revised severity classifications where challenged
> - Additional smells found (if any)
> - Smells that should be excluded (too risky or boundary-adjacent)
> - Interdependency notes between smells

### Step 4 — User gate: smell catalog review

Present the validated smell catalog (Investigator findings + Contrarian
corrections) to the user:

> "Smell catalog ready for `<scope>`. Found <N> smells:
> High severity: <count> — <one-line list>
> Medium severity: <count>
> Low severity: <count>
> Excluded (Contrarian): <list with reason>
>
> Proceed with all high/medium smells? Or adjust scope before I continue?"

Wait for confirmation. The user may exclude additional smells or change
scope. Incorporate their response before proceeding to Step 5.

### Step 5 — Architect: refactoring task list

Invoke **architect** with the validated smell catalog (post-Contrarian,
post-user confirmation) and boundaries. Task:

> Using the validated smell catalog, produce an ordered, dependency-aware
> list of refactoring tasks. Each task must:
> - Address one or more related smells (group by locality)
> - Be small enough to verify independently (single function, module,
>   or abstraction boundary — touchable in one worktree)
> - Include: what to change, smells addressed, files and line ranges touched,
>   verification criterion (e.g. "function X split into Y and Z, each < 30
>   lines"), and dependencies on other tasks
> - Respect all boundaries — if a smell is near a boundary, document why
>   it is either safe to touch or excluded
>
> Produce an Agent Handoff with the full task list (id, title, smells, files,
> verification criterion, dependencies).

### Step 6 — Contrarian: challenge the task list

Invoke **contrarian** with the Architect handoff and the smell catalog. Task:

> Review the refactoring task list. Challenge:
> - Tasks that risk behavior change (restructuring is too aggressive)
> - Tasks too large to verify atomically (should be split)
> - Sequencing that could leave the codebase in a broken intermediate state
> - Tasks touching a boundary — is the specific change actually safe?
> - Tasks with incorrect dependency declarations (missing or spurious deps)
> - Missing tasks for smells the Architect overlooked
>
> Produce an Agent Handoff. Classify each challenge: critical / major / minor.

Pass the Contrarian handoff to **architect**. Task:

> For each challenge: revise the task list to address it, or dismiss it with
> a clear rationale. Produce an updated Agent Handoff with the final task list.

**One round only.** If unresolved critical challenges remain after Architect's
response, collect them and surface to the user:

> "Contrarian raised unresolved critical issues with the refactoring plan:
> <list>. Recommend: [architect's suggested resolution]. How would you
> like to proceed?"

Wait for user direction before continuing.

---

## Per-task cycle

Read the final task list. For each task (run in parallel where scheduling
allows):

### Step 7 — Assign

Record the task as in-progress in the refactor log. Create the worktree
branch. Pass to Refactoring Engineer: task description, branch name, scope,
boundaries, and the verification criterion.

### Step 8 — Refactoring Engineer

Invoke **refactoring-engineer** on `task/refactor-<slug>-<task-id>`. Task:

> Perform the refactoring described in task `<id>`: `<task description>`.
> Work on branch `task/refactor-<slug>-<task-id>`.
> Follow project conventions: Rust 2021, tokio, anyhow, thiserror.
> Do not change observable behavior. Do not touch: `<boundaries>`.
> Verification criterion: `<criterion>`.
> Commit your changes. Produce an Agent Handoff.

### Step 9 — Simplifier

Invoke **simplifier** with the Refactoring Engineer handoff. Task:

> Review the changes on branch `task/refactor-<slug>-<task-id>`.
> Remove: dead code, premature abstractions, over-engineered solutions,
> unnecessary complexity introduced by the refactoring itself.
> Do not nitpick style. Fix only real problems.
> Do not touch: `<boundaries>`. Commit. Produce an Agent Handoff.

### Step 10 — Logging Implementer

Invoke **logging-implementer** with the Simplifier handoff. Task:

> Retrofit every code path touched on branch `task/refactor-<slug>-<task-id>`
> with structured 5-level tracing per the project logging spec:
> - WARN: lifecycle events (proxy/CA bound, mode started/stopped)
> - INFO: atomic operations (connection accepted, request complete)
> - DEBUG: every branch, raw data (truncated to 256 bytes), headers (auth redacted)
> Use structured fields (`key = %val`), never format strings.
> Never log inside a held Mutex lock.
> Do not touch: `<boundaries>`. Commit. Produce an Agent Handoff.

### Step 11 — Test Runner: behavior gate

Invoke **test-runner** on the task branch. Task:

> Run `cargo test` against `task/refactor-<slug>-<task-id>`.
> Compare results against `.claude/workflow/<slug>/baseline-tests.txt`.
> Produce a verdict:
> - `green` — all tests that passed at baseline still pass
> - `red` — one or more previously-passing tests now fail (list each: test
>   name, failure message, likely cause — is this a behavior change or a
>   test fixture issue?)

**If green:** proceed to Step 12.

**If red:** pass the Test Runner handoff back to **refactoring-engineer**:

> Tests regressed on branch `task/refactor-<slug>-<task-id>`.
> Failing tests: `<list>`. Diagnoses: `<list>`.
> Fix the regression without changing behavior. Commit. Produce a handoff.

After the fix, repeat the full chain: Simplifier (Step 9) → Logging
Implementer (Step 10) → Test Runner (Step 11).

**Maximum fix iterations: 3.** If still red after 3 full chain repeats,
revert the task:

```bash
git worktree remove ../worktree-refactor-<task-id>
```

Record in the refactor log: `Status: reverted`. Move to the next task.
Collect all reverts; surface them in the final completion report only
(not individually during the run).

### Step 12 — Contrarian: structure gate

Invoke **contrarian** with the full handoff chain (Steps 8–11) and the
branch name. Task:

> Review the complete changes on branch `task/refactor-<slug>-<task-id>`.
> Verify:
> - Does the refactoring meet its verification criterion: `<criterion>`?
> - Are there residual smells the task was supposed to address?
> - Is every touched code path correctly instrumented per the logging spec?
> - Were any boundaries in `<boundaries>` touched?
>
> Produce a verdict: approved or challenged.
> For each challenge:
> - `[SMELL]` — smell not fully addressed (route to Refactoring Engineer)
> - `[LOGGING]` — instrumentation gap (route to Logging Implementer)
> - `[BEHAVIOR]` — potential behavior change concern (route to Refactoring
>   Engineer, must re-run Test Runner after fix)

**If challenged:** route challenges to the appropriate agent. After fixes,
repeat the full chain from the relevant step:
- `[SMELL]` or `[BEHAVIOR]`: Refactoring Engineer (Step 8) → Simplifier →
  Logging Implementer → Test Runner → Contrarian
- `[LOGGING]` only: Logging Implementer (Step 10) → Test Runner → Contrarian

**Maximum Contrarian rounds: 3.** If not approved after 3 rounds, record
as blocked, surface to user, and do not merge. Move to the next task.

### Step 13 — Merge, clean up team, and log

After Contrarian approval:

1. If a task team was created, shut it down before merging:
   ```text
   SendMessage({ to: "refactoring-engineer", message: {type: "shutdown_request"} })
   SendMessage({ to: "simplifier",           message: {type: "shutdown_request"} })
   SendMessage({ to: "logging-implementer",  message: {type: "shutdown_request"} })
   SendMessage({ to: "test-runner",          message: {type: "shutdown_request"} })
   SendMessage({ to: "contrarian",           message: {type: "shutdown_request"} })
   TeamDelete()
   ```
2. Merge the task branch into the refactor branch.
3. Mark complete in the refactor log.
4. Move to the next task.

---

## Final validation

After all tasks have been processed (before team cleanup):

```bash
cargo test 2>&1 | tee .claude/workflow/<slug>/final-tests.txt
```

Compare against baseline. If any previously-passing test now fails, the
refactor branch is not ready — surface to user with the diff between
baseline and final test output. Do not proceed to completion report until
this is resolved or the user explicitly accepts the failure.

---

## Team cleanup (safety net)

After final validation, ensure all teams are cleaned up:

```text
SendMessage({ to: "investigator",         message: {type: "shutdown_request"} })
SendMessage({ to: "architect",            message: {type: "shutdown_request"} })
SendMessage({ to: "contrarian",           message: {type: "shutdown_request"} })
SendMessage({ to: "refactoring-engineer", message: {type: "shutdown_request"} })
SendMessage({ to: "simplifier",           message: {type: "shutdown_request"} })
SendMessage({ to: "logging-implementer",  message: {type: "shutdown_request"} })
SendMessage({ to: "test-runner",          message: {type: "shutdown_request"} })
TeamDelete()
```

Skip agents already shut down per-task in Step 13.

---

## Completion

Produce a **completion report** to the user:

```text
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✅ Refactor complete — <scope>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Branch: refactor/<slug>
Refactor log: .claude/workflow/<slug>/refactor-log.md

Tasks merged:   <N> — <list with one-line description each>
Tasks reverted: <N> — <list with reason, or "none">
Tasks blocked:  <N> — <list with reason, or "none">

Smells addressed: <count>
Smells remaining: <count and locations — from reverted/blocked tasks>

Behavior guarantee: baseline tests reproduced exactly
  Baseline: <X tests passing>
  Final:    <X tests passing>

Logging coverage: every touched path instrumented per 5-level spec

Next steps (if any):
<blocked tasks, user-excluded smells, or smells near boundaries that
were left intentionally untouched>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

Then produce a **Phase Handoff** (for programmatic invocation):

```text
=== PHASE HANDOFF ===
Phase:     Refactor
Status:    complete  (or: blocked — <reason>)
Scope:     <scope>
Branch:    refactor/<slug>
Artifacts:
  .claude/workflow/<slug>/refactor-log.md
  .claude/workflow/<slug>/baseline-tests.txt
  .claude/workflow/<slug>/final-tests.txt
Decisions: <bullet list of key structural decisions made>
For next:  <what follow-on work needs to know: what changed, what was left,
            areas that may need further attention>
Open:      <reverted or blocked tasks, or "none">
=== END HANDOFF ===
```
