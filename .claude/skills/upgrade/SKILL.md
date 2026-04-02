---
name: upgrade
description: Phase 4 of the modernize workflow. Upgrades the Rust edition (e.g. 2021 → 2024) using cargo fix --edition, then a Developer cycle for anything cargo fix cannot automate. Contrarian reviews the research before migration starts. Investigator audits the cargo fix output before Developer fixes begin. Test Runner and Contrarian gate the result. Only runs with explicit user opt-in. Can be invoked standalone by passing a Migrate Phase Handoff as the argument.
argument-hint: <migrate phase handoff>
context: fork
---

# Phase 4 — Upgrade

You are the **Phase 4 coordinator**. Your job is to upgrade the Rust edition
of the project — the most disruptive modernization step — safely and
completely. This phase only runs with explicit user opt-in.

Input: **$ARGUMENTS**

Extract from the input:
- `scope`: the modernization scope
- `branch`: the modernize branch (`modernize/<slug>`)
- `openspec_id`: the OpenSpec change id (from Migrate handoff `OpenSpec` field)
- `for_next`: context from Migrate (codebase health, known blockers)
- `WORKTREE`: the absolute path to the modernize worktree (required when
  invoked from orchestrator; if missing, derive as
  `$(git rev-parse --show-toplevel)/../worktrees/modernize-<slug>`)

Derive `WORKTREE_PARENT` = `dirname(<WORKTREE>)`. The upgrade worktree is
created as a sibling: `<WORKTREE_PARENT>/task-upgrade-<slug>-edition`.

---

## Working directory

**All operations in this phase must happen inside `<WORKTREE>` or its sibling
upgrade worktree, never in the main repository working tree.**

Rules that apply to this coordinator and to every agent it invokes:
- File reads/writes on the modernize branch: `<WORKTREE>/<relative-path>`
- Git commands on the modernize branch: `git -C "<WORKTREE>" <command>`
- Cargo commands: `cd "<WORKTREE>" && cargo <command>` (or the upgrade worktree path)
- openspec commands: `cd "<WORKTREE>" && openspec <command>`
- Upgrade worktree: `<WORKTREE_PARENT>/task-upgrade-<slug>-edition`
- **Every agent message must include `WORKTREE: <upgrade_worktree_path>`** so
  agents know exactly where to work.

---

## Agent coordination protocol

Try team-based coordination first:

```text
TeamCreate({ name: "upgrade-team",
             agents: ["general-purpose", "investigator", "developer",
                      "simplifier", "logging-implementer",
                      "test-runner", "contrarian"] })
SendMessage({ to: "general-purpose", message: "<task + context>" })
```

Fall back to sequential `Agent` tool calls if `TeamCreate` fails. Do not
retry teams more than once.

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

## Live progress reports

Emit progress announcements at key milestones — do NOT wait until the Phase
Handoff to tell the user anything.

**Upgrade starting** (emit at Step 3, after baseline passes):

```
─── Edition upgrade <current> → <target> starting
    Branch: task/upgrade-<slug>-edition
    cargo fix handles automatically: <summary>
    Manual fixes expected: <summary>
```

**cargo fix complete** (emit at Step 4, after commit):

```
━━━ cargo fix complete
    Files changed: <count>
    Remaining errors: <count> — <brief list>
    Remaining warnings: <count>
```

**Tests green** (emit at Step 9, after Test Runner returns green):

```
━━━ Tests green — submitting to Contrarian
    cargo test: green
    cargo clippy: clean
```

**Upgrade merged** (emit at Step 11, after merge):

```
━━━ Edition upgrade merged ✓
    Edition: <current> → <target>
    Files changed by cargo fix: <count>
    Manual fixes: <count>
    Contrarian: approved in <N> round(s)
```

---

## Workflow

### Step 1 — General-purpose agent: edition research

Invoke **general-purpose** (with web search). Task:

> **All file reads must use `<WORKTREE>/<path>`. Never read files outside the worktree.**
>
> 1. Read `<WORKTREE>/Cargo.toml` and record the current `edition` field.
> 2. Web search: "rust stable edition latest <current year>" to identify
>    the latest stable edition.
> 3. If current edition equals latest: produce a handoff with
>    `Status: blocked — already on latest edition`. Stop.
> 4. Web search: "rust edition guide <current> to <target> migration" and
>    read the official Rust Edition Guide for this transition.
> 5. Web search: "cargo fix edition <target> <current year> known issues
>    limitations" to surface any reported problems with the automated tool.
> 6. Web search: "rust <target> edition breaking changes production experience"
>    for community reports of pain points.
> 7. Also read `<WORKTREE>/Cargo.toml` for the `rust-version` field (MSRV). After upgrade,
>    the edition must be compatible with the declared MSRV, or MSRV must be bumped.
>
> Produce an Agent Handoff with:
> - Current edition, target edition
> - Summary: what `cargo fix` handles automatically vs what requires manual work
> - Known issues with `cargo fix --edition` for this transition
> - MSRV compatibility note
> - Migration guide URL

**If handoff Status is blocked (already on latest edition):** produce a
Phase Handoff immediately with `Status: blocked — already on latest edition`
and stop.

### Step 2 — Contrarian: challenge the research

Invoke **contrarian** with the Step 1 handoff. Task:

> Review the edition upgrade research. Challenge:
> - Is the target edition truly production-ready, or are there known
>   compiler issues affecting projects like this one? Web search:
>   "<target> edition rustc regression <current year>"
> - Are the "known issues" with `cargo fix` actually relevant to this
>   codebase, given the Migrate phase context: `<for_next>`?
> - Is the MSRV impact acceptable, or would bumping MSRV break anything
>   we care about?
> - Are there patterns common in async Rust codebases (this project uses
>   tokio extensively) that the edition change affects in non-obvious ways?
>   Web search: "rust <target> edition async tokio breaking changes"
>
> Produce an Agent Handoff with: confirmed risks, dismissed concerns with
> rationale, any blockers that would prevent proceeding.

If Contrarian identifies a genuine blocker (e.g. a known compiler regression
affecting async code in the target edition): surface it to the user immediately
and halt with a blocked Phase Handoff.

### Step 3 — Baseline

```bash
cd "<WORKTREE>"
cargo test 2>&1 | tee "<WORKTREE>/.claude/workflow/<slug>/upgrade-baseline.txt"
cargo clippy -- -D warnings 2>&1 | tee -a "<WORKTREE>/.claude/workflow/<slug>/upgrade-baseline.txt"
```

If baseline is broken, surface to user and halt. The edition upgrade cannot
proceed on a broken baseline.

### Step 4 — Create worktree and run cargo fix

```bash
git -C "<WORKTREE>" worktree add \
  "<WORKTREE_PARENT>/task-upgrade-<slug>-edition" \
  -b task/upgrade-<slug>-edition
```

Invoke **general-purpose** on this branch. Task:

> Working directory: `<WORKTREE_PARENT>/task-upgrade-<slug>-edition`
> (branch: `task/upgrade-<slug>-edition`) — run all commands and edit all
> files within this directory only.
>
> 1. Run `cargo fix --edition` exactly once — do not run it twice, it is
>    not idempotent:
>    ```bash
>    cd "<WORKTREE_PARENT>/task-upgrade-<slug>-edition" && \
>      cargo fix --edition 2>&1 | tee "<WORKTREE>/.claude/workflow/<slug>/cargo-fix-output.txt"
>    ```
> 2. Edit `<WORKTREE_PARENT>/task-upgrade-<slug>-edition/Cargo.toml`:
>    change the `edition` field to `"<target>"`.
> 3. Run `cd "<WORKTREE_PARENT>/task-upgrade-<slug>-edition" && cargo build 2>&1`
>    and record every remaining error and warning that `cargo fix` did not
>    resolve. These are the manual fixes needed.
> 4. Run `cd "<WORKTREE_PARENT>/task-upgrade-<slug>-edition" && cargo test 2>&1`
>    and record which tests pass and which fail after `cargo fix` alone
>    (before any Developer manual fixes). Save as
>    `<WORKTREE>/.claude/workflow/<slug>/post-fix-baseline.txt`.
>    This separates "cargo fix broke this" from "manual fix broke this".
> 5. Commit the `cargo fix` output and `Cargo.toml` edition change.
>
> Produce an Agent Handoff with:
> - Files changed by `cargo fix` (list)
> - Remaining compilation errors (file:line, error message)
> - Remaining warnings (file:line, warning)
> - Post-fix test results summary

### Step 5 — Investigator: catalog remaining manual work

Invoke **investigator** with the Step 4 handoff. Task:

> Working directory: `<WORKTREE_PARENT>/task-upgrade-<slug>-edition`
> (branch: `task/upgrade-<slug>-edition`) — search and read files only
> within this directory.
>
> Review the `cargo fix` output on branch `task/upgrade-<slug>-edition`.
> Catalog everything that still needs manual attention:
> - Each remaining compilation error: file:line, error text, likely cause
>   in terms of the edition change
> - Each remaining warning: file:line, warning text, whether it is a
>   must-fix (e.g. `unused_import`) or a style warning
> - Any `#[allow(...)]` suppressions that `cargo fix` added automatically
>   that should be properly fixed instead of suppressed
> - Any patterns that `cargo fix` handled technically but that are
>   non-idiomatic for the `<target>` edition (e.g. it added a workaround
>   where the idiomatic new API exists)
> - Verify: does the `rust-version` field in
>   `<WORKTREE_PARENT>/task-upgrade-<slug>-edition/Cargo.toml` need bumping
>   to match the new edition's MSRV requirements?
>
> Do NOT propose fixes. Produce an Agent Handoff with a prioritized list
> of manual fixes: errors first, then must-fix warnings, then idiom improvements.

### Step 6 — Developer: manual fixes

If Step 5 found remaining errors or warnings, invoke **developer** on the
upgrade branch. Task:

> Working directory: `<WORKTREE_PARENT>/task-upgrade-<slug>-edition`
> (branch: `task/upgrade-<slug>-edition`) — modify files only within
> this directory.
>
> `cargo fix --edition` has run and the edition is now `"<target>"` in
> `<WORKTREE_PARENT>/task-upgrade-<slug>-edition/Cargo.toml`.
> The following issues remain and need manual fixes:
>
> Errors (must fix): `<list from Investigator>`
> Must-fix warnings: `<list from Investigator>`
> Idiom improvements: `<list from Investigator>`
> MSRV adjustment needed: `<yes/no — and what to change>`
>
> Fix every error and must-fix warning. For idiom improvements, use the
> idiomatic `<target>` edition approach — consult the migration guide at
> `<URL from Step 1>`. Do not change behavior beyond what the edition
> migration requires. Commit. Produce an Agent Handoff.

**Maximum Developer iterations: 3** (Test Runner gate may send fixes back).
If still failing after 3 iterations within the test loop (Steps 7–8),
record the upgrade as blocked.

If Step 5 found no remaining issues, skip this step.

### Step 7 — Simplifier

Invoke **simplifier** with the last handoff. Task:

> Working directory: `<WORKTREE_PARENT>/task-upgrade-<slug>-edition`
> (branch: `task/upgrade-<slug>-edition`) — modify files only within
> this directory.
>
> Review all changes on branch `task/upgrade-<slug>-edition` — both the
> `cargo fix` output and any Developer manual fixes.
> Remove: `#[allow(...)]` suppressions that `cargo fix` added automatically
> but that can be properly fixed, compatibility workarounds the new edition
> handles natively, dead code that only existed to support the old edition.
> Do not touch code unrelated to the edition upgrade.
> Commit. Produce an Agent Handoff.

### Step 8 — Logging Implementer

Invoke **logging-implementer** with the Simplifier handoff. Task:

> Working directory: `<WORKTREE_PARENT>/task-upgrade-<slug>-edition`
> (branch: `task/upgrade-<slug>-edition`) — modify files only within
> this directory.
>
> Retrofit every code path touched by the edition upgrade on branch
> `task/upgrade-<slug>-edition` with structured 5-level tracing:
> - WARN: lifecycle events
> - INFO: atomic operations
> - DEBUG: every branch, raw data (truncated to 256 bytes), headers (auth redacted)
> Use structured fields (`key = %val`), never format strings.
> Never log inside a held Mutex lock. Commit. Produce an Agent Handoff.

### Step 9 — Test Runner: behavior gate

Invoke **test-runner**. Task:

> Working directory: `<WORKTREE_PARENT>/task-upgrade-<slug>-edition`
> (branch: `task/upgrade-<slug>-edition`).
> Run `cd "<WORKTREE_PARENT>/task-upgrade-<slug>-edition" && cargo test`
> and `cargo clippy -- -D warnings` there.
> Compare against `<WORKTREE>/.claude/workflow/<slug>/upgrade-baseline.txt`.
> Also compare against `<WORKTREE>/.claude/workflow/<slug>/post-fix-baseline.txt`
> to distinguish: did a manual fix or simplification cause a new regression
> vs was it already broken after `cargo fix`?
> Produce a verdict:
> - `green` — all previously-passing tests still pass, clippy clean
> - `red` — regressions (list each: test name, error, which baseline it
>   broke relative to — cargo-fix baseline or original baseline)

**If green:** proceed to Step 10.

**If red:** pass to **developer** to fix the regression. After each fix,
repeat: Simplifier (Step 7) → Logging Implementer (Step 8) → Test Runner
(Step 9).

**Maximum fix iterations: 3.** If still red after 3 full chain repeats,
revert without merging:

```bash
git -C "<WORKTREE>" worktree remove \
  "<WORKTREE_PARENT>/task-upgrade-<slug>-edition"
```

Produce a Phase Handoff with `Status: blocked — edition upgrade could not
be completed cleanly. Manual intervention required. Remaining failures:
<list>.`

### Step 10 — Contrarian: edition quality gate

Invoke **contrarian** with the full handoff chain (Steps 1–9). Task:

> Working directory: `<WORKTREE_PARENT>/task-upgrade-<slug>-edition`
> (branch: `task/upgrade-<slug>-edition`) — read files only within
> this directory.
>
> Review the complete edition upgrade on branch `task/upgrade-<slug>-edition`.
> Verify:
> - `cargo fix` output was applied correctly (check
>   `<WORKTREE>/.claude/workflow/<slug>/cargo-fix-output.txt`)
> - Manual fixes in Step 6 use idiomatic `<target>` edition patterns, not
>   minimal workarounds
> - No `<current>` edition idioms remain where `<target>` has a proper
>   replacement (cross-reference Investigator's idiom improvement list)
> - No `#[allow(...)]` suppressions were left as lazy fixes
> - Logging coverage is complete for all touched paths
> - The `rust-version` field in `Cargo.toml` is consistent with the new edition
> - No behavior was changed beyond what the edition migration requires
>
> Produce a verdict: approved or challenged.
> For each challenge:
> - `[IDIOM]` — non-idiomatic fix that should use edition-native approach
>   (route to Developer, restart from Step 7)
> - `[LOGGING]` — instrumentation gap (route to Logging Implementer,
>   restart from Step 8, then re-run Test Runner)
> - `[BEHAVIOR]` — unexpected behavior change (route to Developer, restart
>   from Step 7, must re-run Test Runner)

**Maximum Contrarian rounds: 2.** If not approved after 2 rounds, surface
the remaining challenges to the user with a recommendation. The user may
override and proceed to merge, or defer the upgrade.

### Step 11 — Merge

After Contrarian approval (or user override):

```bash
git -C "<WORKTREE>" merge --no-ff task/upgrade-<slug>-edition \
  -m "upgrade: Rust edition <current> → <target>"
git -C "<WORKTREE>" worktree remove \
  "<WORKTREE_PARENT>/task-upgrade-<slug>-edition"
```

---

## Team cleanup

```text
SendMessage({ to: "general-purpose",     message: {type: "shutdown_request"} })
SendMessage({ to: "investigator",        message: {type: "shutdown_request"} })
SendMessage({ to: "developer",           message: {type: "shutdown_request"} })
SendMessage({ to: "simplifier",          message: {type: "shutdown_request"} })
SendMessage({ to: "logging-implementer", message: {type: "shutdown_request"} })
SendMessage({ to: "test-runner",         message: {type: "shutdown_request"} })
SendMessage({ to: "contrarian",          message: {type: "shutdown_request"} })
TeamDelete()
```

---

## Phase completion

Phase 4 is complete when:
- Edition field in `Cargo.toml` is updated to the target edition
- `cargo test` and `cargo clippy -- -D warnings` are green
- Upgrade branch is merged into `modernize/<slug>`
- Contrarian has approved (or user has overridden with documented rationale)

Produce a **Phase Handoff**:

```text
=== PHASE HANDOFF ===
Phase:     Upgrade
Status:    complete  (or: blocked — <reason>)
Scope:     <scope>
Branch:    <branch>
OpenSpec:  <openspec_id>
Artifacts:
  <WORKTREE>/.claude/workflow/<slug>/upgrade-baseline.txt
  <WORKTREE>/.claude/workflow/<slug>/post-fix-baseline.txt
  <WORKTREE>/.claude/workflow/<slug>/cargo-fix-output.txt
Decisions:
  - Edition upgraded: <current> → <target>
  - Files changed by cargo fix: <count>
  - Manual fixes applied: <count>
  - MSRV adjusted: <yes — from X to Y | no>
For next:  Modernize branch is ready for user review and merge to main.
           Edition: <target>. All tests green. All touched paths instrumented.
           Run `openspec archive <openspec_id> --yes` after the branch is merged.
Open:      <Contrarian challenges overridden by user, or "none">
=== END HANDOFF ===
```
