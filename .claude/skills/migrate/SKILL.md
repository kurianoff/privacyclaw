---
name: migrate
description: Phase 3 of the modernize workflow. Executes the migration plan from Research. Batch-bumps Tier 1 deps, removes unused deps, then runs a full Developer → Investigator → Simplifier → Logging Implementer → Test Runner → Contrarian cycle per Tier 2/3 task. Contrarian also reviews any Batch A Developer fixes. Reverts tasks that cannot be migrated cleanly. Can be invoked standalone by passing a Research Phase Handoff as the argument.
argument-hint: <research phase handoff>
context: fork
---

# Phase 3 — Migrate

You are the **Phase 3 coordinator**. Your job is to execute the migration plan
from Research — updating every dependency in the correct order, adapting code
to new APIs, and ensuring the test suite and linter stay clean at every merge
point.

Input: **$ARGUMENTS**

Extract from the input:
- `scope`: the modernization scope
- `branch`: the modernize branch (`modernize/<slug>`)
- `migration_plan`: path to the migration plan (from Research handoff `Artifacts`)
- `for_next`: context from Research (task count per batch, complex tasks,
  low-confidence flags, grouped migration task IDs)

If the fast-path applies (Research returned only patch/minor updates with no
migration plan file), read the Audit catalog instead and proceed with
Batch A only.

---

## Critical: migration log

**Maintain a running log at `.claude/workflow/<slug>/migration-log.md`**
throughout this phase. After every task, append:

```text
### <Dep or Task ID>: <dep name(s)> <current> → <target>
Batch: A | B | C | unused-removal
Status: complete | reverted | blocked
Branch: task/migrate-<slug>-<task-id>   (or: direct commit for Batch A)
Changes:
  - <Cargo.toml changes>
  - <code changes summary>
Investigator: <findings or "clean">
Test Runner: green | red (fix iterations: N)
Contrarian: approved | challenged (rounds: N)
Outcome: merged | reverted — <reason>
```

---

## Agent coordination protocol

For Batch B and C tasks, try team-based coordination:

```text
TeamCreate({ name: "migrate-<task-id>-team",
             agents: ["developer", "investigator", "simplifier",
                      "logging-implementer", "test-runner", "contrarian"] })
SendMessage({ to: "developer", message: "<task + context>" })
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

Each Batch B/C task runs on a dedicated branch. Sanitize dep names for
branch use: replace `/`, `.`, `+` with `-` and lowercase.

```bash
git worktree add ../worktree-migrate-<task-id> -b task/migrate-<slug>-<task-id>
```

**Cargo.toml serialization rule:** only one worktree may have a Cargo.toml
edit in flight at a time. Apply the version bump for a task to the
`modernize/<slug>` branch directly before creating the task worktree, then
create the worktree from that state. This prevents Cargo.toml merge conflicts
when parallel worktrees are merged back.

- **Developer, Simplifier, Logging Implementer** commit to the task branch.
  Use `isolation: "worktree"` for code-changing agent calls.
- **Investigator, Test Runner, Contrarian** read from the task branch but
  do not commit.

**Merge checkpoint** — after Contrarian approves:

```bash
git checkout modernize/<slug>
git merge --no-ff task/migrate-<slug>-<task-id> \
  -m "migrate(<task-id>): <dep> <current> → <target>"
git worktree remove ../worktree-migrate-<task-id>
```

**Revert checkpoint** — if a task is abandoned:

```bash
git worktree remove ../worktree-migrate-<task-id>
# Revert the Cargo.toml version bump on modernize/<slug>:
git revert HEAD --no-edit  # if the bump was the last commit
# or manually restore the version in Cargo.toml and commit
```

---

## Live progress reports

Emit a progress announcement to the user at key moments. Do NOT wait until
the Phase Handoff to tell the user what happened.

**Batch A complete** (after Step 3 passes, or after Developer fixes land):

```
━━━ Batch A complete
    Deps bumped: <count> (<names>)
    Status: clean | required <N> fix iteration(s)
    Test Runner: green | Contrarian rounds: N
```

**Task start** (emit immediately at Step 4a, before the worktree is created):

```
─── Task <task-id>/<total> starting: migrate <dep> <current> → <target>
    Batch: B | C
    Files: <list from migration plan>
    Confidence: high | medium | low
```

**Task complete** (emit immediately at Step 4h, after the merge commits):

```
━━━ Task <task-id>/<total> — <dep> <current> → <target>  [merged ✓ | reverted ✗ | blocked ⚠]
    Investigator: clean | N remaining usages fixed
    Test Runner: green in <N> iteration(s)
    Contrarian: approved in <N> round(s)
```

If reverted or blocked, replace the last two lines with:

```
    Reason: <why>
    Next: <what would unblock it, or "none">
```

**Running tally** — append after every task-complete report:

```
    Progress: <completed>/<total> tasks merged, <reverted> reverted, <blocked> blocked
```

---

## Task scheduling

Build a dependency graph from the migration plan before starting Batch B/C.
Independent tasks within the same batch (no overlapping files, no task
dependency) may run in parallel — with their Cargo.toml bumps applied
serially to `modernize/<slug>` before worktrees are created.

---

## Workflow

### Step 1 — Baseline

```bash
cargo test 2>&1 | tee .claude/workflow/<slug>/migrate-baseline.txt
cargo clippy -- -D warnings 2>&1 | tee -a .claude/workflow/<slug>/migrate-baseline.txt
```

Record the baseline in the migration log header. If baseline is broken,
surface to user and halt.

### Step 2 — Unused dep removal

For every dep confirmed unused in the migration plan (Audit catalog, Batch
"unused-removal"):

Edit `Cargo.toml` directly on `modernize/<slug>` to remove the dep entry.

```bash
cargo build  # verify removal compiles
cargo test   # verify no regressions
```

If compilation or tests fail: a dep marked unused is actually used. Restore
it, record as `reverted — false unused positive`, and continue.

Record each removal in the migration log.

### Step 3 — Batch A: Tier 1 patch/minor updates

Collect all Tier 1 deps from the migration plan into a single batch. Apply
all version bumps to `Cargo.toml` on `modernize/<slug>` at once:

```bash
cargo update   # resolve new Cargo.lock
cargo test 2>&1
cargo clippy -- -D warnings 2>&1
```

**If both pass:** record all Tier 1 deps as complete in the migration log.
Proceed to Batch B.

**If either fails:** identify which dep caused the regression by reverting
version bumps one at a time and re-running until the failure disappears. Then:

Invoke **developer** directly on `modernize/<slug>` (no worktree needed):

> These Tier 1 dep bumps caused regressions: `<dep list>`.
> Failures: `<test names / clippy errors>`.
> Fix the minimum code needed to restore green. Do not change behavior
> beyond what the patch/minor API change requires. Commit directly to
> `modernize/<slug>`. Produce an Agent Handoff.

After Developer fixes, invoke **contrarian** to review the fix:

> Review the Batch A fixes committed directly to `modernize/<slug>`.
> Verify: the fix is minimal, no behavior was changed beyond the dep update,
> no new tech debt was introduced. Approved or challenged?

If challenged, pass back to Developer. Maximum 2 Contrarian rounds for
Batch A fixes. If still challenged, surface to user.

Re-run `cargo test` and `cargo clippy` after all fixes. If still failing
after Developer + 2 fix attempts: revert the failing deps' version bumps,
record as reverted, and continue with Batch B.

### Step 4 — Batch B and C: per-task migration cycle

Read the migration plan. Execute tasks in dependency order. Independent
tasks within the same batch may run in parallel (apply their Cargo.toml
bumps serially to `modernize/<slug>` before creating parallel worktrees).

For each task:

#### Step 4a — Apply Cargo.toml bump and create worktree

**Emit task-start progress report** before creating the worktree.

On `modernize/<slug>`:
```bash
# Edit Cargo.toml: update dep version(s) for this task
cargo build 2>&1  # record what breaks — this is the migration target
```

```bash
git worktree add ../worktree-migrate-<task-id> -b task/migrate-<slug>-<task-id>
```

Inject into every agent's context: task ID, dep name(s), version bump,
migration notes (from migration plan), file list with specific old→new API
changes, verification criterion, confidence level, and whether this is a
grouped migration.

#### Step 4b — Developer: implement migration

Invoke **developer** on the task branch. Task:

> Migrate `<dep>` from `<current>` to `<target>` on branch
> `task/migrate-<slug>-<task-id>`.
> The Cargo.toml has already been updated. Fix all compilation errors and
> deprecation warnings by applying these changes per file:
> `<file:line>`: `<old API>` → `<new API>` (repeat per entry in plan)
> Migration guide: `<URL>`
> Confidence: `<level>` — `<low-confidence note if applicable>`
> Verification criterion: `<criterion>`
> Follow project conventions: Rust 2021, tokio, anyhow, thiserror.
> Do not change behavior beyond what the API migration requires.
> Commit. Produce an Agent Handoff.

#### Step 4c — Investigator: verify migration completeness

Invoke **investigator** with the Developer handoff and task context. Task:

> Review the changes on branch `task/migrate-<slug>-<task-id>`.
> Check for remaining uses of the old API that Developer did not address:
> - Search for every old API name listed in the migration plan
> - Search for any remaining deprecation warnings (`#[deprecated]` usages)
> - Search for any `#[allow(deprecated)]` suppressions that should have been
>   removed as part of this migration
> - Verify that grouped-migration deps were all addressed together
>
> Produce an Agent Handoff: list of remaining old API usages (if any) with
> file:line, or confirm the migration is complete.

If Investigator finds remaining usages, pass back to **developer** to fix
them. Repeat Steps 4b–4c until Investigator confirms clean. Maximum 2
Investigator rounds before escalating to Contrarian as a known gap.

#### Step 4d — Simplifier

Invoke **simplifier** with the last handoff. Task:

> Review the migration changes on branch `task/migrate-<slug>-<task-id>`.
> Remove: compatibility shims that the new API makes unnecessary, adapter
> types that can be replaced by native new-API equivalents, redundant
> `#[allow(...)]` suppressions now cleared. Do not touch code unrelated to
> this migration. Commit. Produce an Agent Handoff.

#### Step 4e — Logging Implementer

Invoke **logging-implementer** with the Simplifier handoff. Task:

> Retrofit every code path touched by the `<dep>` migration on branch
> `task/migrate-<slug>-<task-id>` with structured 5-level tracing:
> - WARN: lifecycle events (proxy/CA bound, mode started/stopped)
> - INFO: atomic operations (connection accepted, request complete)
> - DEBUG: every branch, raw data (truncated to 256 bytes), headers (auth redacted)
> Use structured fields (`key = %val`), never format strings.
> Never log inside a held Mutex lock. Commit. Produce an Agent Handoff.

#### Step 4f — Test Runner: behavior gate

Invoke **test-runner**. Task:

> Run `cargo test` and `cargo clippy -- -D warnings` on branch
> `task/migrate-<slug>-<task-id>`.
> Compare against `.claude/workflow/<slug>/migrate-baseline.txt`.
> Produce a verdict:
> - `green` — all previously-passing tests pass, clippy clean
> - `red` — regressions or new clippy errors (list each: test name, error,
>   diagnosis — behavior change vs test fixture issue)

**If green:** proceed to Step 4g.

**If red:** pass to **developer** to fix. After each fix, repeat the full
chain: Simplifier (4d) → Logging Implementer (4e) → Test Runner (4f).

**Maximum fix iterations: 3.** If still red after 3 full chain repeats,
revert the task (see worktree protocol). Record `Status: reverted` in the
migration log. Move to the next task. Collect reverts for the final report.

#### Step 4g — Contrarian: migration quality gate

Invoke **contrarian** with the full handoff chain (Steps 4b–4f). Task:

> Review the complete migration of `<dep>` on branch `task/migrate-<slug>-<task-id>`.
> Verify:
> - All breaking API changes in the migration plan were addressed
>   (cross-reference the Investigator's clean confirmation from Step 4c)
> - No compatibility shims remain that Simplifier should have removed
> - Logging coverage is complete for every touched path
> - No behavior was changed beyond what the migration required
> - Verification criterion is met: `<criterion>`
>
> Produce a verdict: approved or challenged.
> For each challenge:
> - `[MIGRATION]` — missed API change (route to Developer, restart from 4b)
> - `[LOGGING]` — instrumentation gap (route to Logging Implementer, restart from 4e)
> - `[BEHAVIOR]` — unexpected behavior change (route to Developer, restart from 4b,
>   must re-run Test Runner)

**If challenged:** route to the appropriate agent and repeat from the
indicated step. After fixes, re-run Test Runner and Contrarian.

**Maximum Contrarian rounds: 3.** If not approved after 3 rounds, record as
blocked, do not merge, surface to user in the final report.

#### Step 4h — Merge, clean up team, log

After Contrarian approval:

**Emit task-complete progress report** (merged ✓) with running tally.

1. Shut down task team if one was created:
   ```text
   SendMessage({ to: "developer",           message: {type: "shutdown_request"} })
   SendMessage({ to: "investigator",        message: {type: "shutdown_request"} })
   SendMessage({ to: "simplifier",          message: {type: "shutdown_request"} })
   SendMessage({ to: "logging-implementer", message: {type: "shutdown_request"} })
   SendMessage({ to: "test-runner",         message: {type: "shutdown_request"} })
   SendMessage({ to: "contrarian",          message: {type: "shutdown_request"} })
   TeamDelete()
   ```
2. Merge task branch into `modernize/<slug>`.
3. Mark complete in migration log. Move to next task.

---

### Step 5 — Final audit

After all tasks are processed:

```bash
cargo audit 2>&1          # verify no new advisories introduced
cargo outdated --depth 1 2>&1   # show what remains (excluded/reverted)
cargo clippy -- -D warnings 2>&1
cargo test 2>&1
```

Record results in `.claude/workflow/<slug>/migrate-final.txt`.

---

## Team cleanup (safety net)

Before producing the Phase Handoff, ensure all open task teams are cleaned up.

---

## Phase completion

Phase 3 is complete when:
- All Batch A, B, and C tasks are marked complete, reverted, or blocked
- All task worktrees are merged or discarded
- Final `cargo test`, `cargo clippy`, `cargo audit` all recorded
- Migration log reflects reality

Produce a **Phase Handoff**:

```text
=== PHASE HANDOFF ===
Phase:     Migrate
Status:    complete  (or: blocked — <reason>)
Scope:     <scope>
Branch:    <branch>
Artifacts:
  .claude/workflow/<slug>/migration-log.md
  .claude/workflow/<slug>/migrate-baseline.txt
  .claude/workflow/<slug>/migrate-final.txt
Decisions:
  - <key migration decisions, reverts, and their reasons>
For next:  <what Upgrade needs: current Rust edition, codebase health
            post-migration, any known complexity that could affect edition
            upgrade, blockers if any>
Open:
  - <reverted tasks: dep, reason, what would unblock>
  - <blocked tasks: dep, reason, user decision needed>
  - (or "none")
=== END HANDOFF ===
```
