---
name: test
description: Run Phase 4 (Testing) of the privacyclaw feature workflow. PM and Architect decide test types; Test Developer implements them; Stress Tester adds load tests if needed; Test Runner gates completion. Implementation bugs route back through Phase 3. Can be invoked standalone by passing a Development Phase Handoff as the argument.
argument-hint: <develop phase handoff>
context: fork
---

# Phase 4 — Testing

You are the **Phase 4 coordinator**. Your job is to ensure every piece of the
implementation is covered by tests that would catch real bugs in production —
and that all of them pass before the feature branch is declared done.

Input: **$ARGUMENTS**

Extract from the input:
- `feature`: the feature description
- `branch`: the feature branch (`feature/<slug>`)
- `impl_log`: path to the implementation log
- `for_next`: context from Development (risk areas, known complexity, gaps)
- `WORKTREE`: the absolute path to the feature worktree (required;
  if missing, derive as `$(git rev-parse --show-toplevel)/../worktrees/<branch-slug>`)

Derive `WORKTREE_PARENT` = `dirname(<WORKTREE>)`. The test worktree is a
sibling: `<WORKTREE_PARENT>/task-tests-<slug>`.

---

## Working directory

**All operations in this phase must happen inside `<WORKTREE>` (or the test
worktree sibling), never in the main repository working tree.**

Rules that apply to this coordinator and to every agent it invokes:
- File reads/writes: `<WORKTREE>/<relative-path>`
- Git commands: `git -C "<WORKTREE>" <command>`
- Cargo commands: `cd "<WORKTREE>" && cargo <command>` (or the test worktree path)
- Test worktree: created at `<WORKTREE_PARENT>/task-tests-<slug>`
- **Every agent message must include `WORKTREE: <worktree_path>`** with the
  appropriate worktree path.

---

## Agent coordination protocol

Try team-based coordination:

```text
TeamCreate({ name: "test-team",
             agents: ["pm", "architect", "test-developer",
                      "stress-tester", "test-runner"] })
SendMessage({ to: "pm", message: "<task + context>" })
```

Fall back to sequential `Agent` tool calls if `TeamCreate` fails. Do not retry
teams more than once.

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

Test files are written on a dedicated branch that is a **sibling** of the
feature worktree (git cannot nest worktrees):

```bash
git -C "<WORKTREE>" worktree add "<WORKTREE_PARENT>/task-tests-<slug>" \
  -b task/tests-<slug>
```

**Test Developer and Stress Tester** commit to `task/tests-<slug>`.
**Test Runner** reads from it but does not commit.

After Test Runner confirms all tests pass:

```bash
git -C "<WORKTREE>" merge --no-ff task/tests-<slug> \
  -m "test(<slug>): add test coverage"
git -C "<WORKTREE>" worktree remove "<WORKTREE_PARENT>/task-tests-<slug>"
```

---

## Workflow

### Step 1 — PM + Architect: test plan

Invoke **pm** and **architect** jointly (or sequentially if teams unavailable)
with the implementation log and Development handoff context. Task:

> Working directory: `<WORKTREE>` — read files only within this directory.
> Do not access the main repository working tree.
> WORKTREE: `<worktree_path>`
>
> Read the implementation log at `<impl_log>` and the development context.
> Decide which test types are needed for each area of the implementation:
> - Unit tests: individual functions and modules
> - Functional tests: end-to-end behaviour of a feature
> - Integration tests: interaction between components
> - Acceptance tests: user-facing correctness criteria
> - Stress tests: concurrency, load, throughput (flag explicitly if needed)
>
> Pay special attention to areas flagged as risky or complex in the
> development handoff. Produce an Agent Handoff with the test plan —
> a list of test areas, type, and rationale for each.

### Step 2 — Test Developer

Create the test worktree branch. Invoke **test-developer** with the test plan
and branch name. Task:

> Working directory: `<WORKTREE_PARENT>/task-tests-<slug>`
> (branch: `task/tests-<slug>`) — do not read or write any
> files outside this directory.
> WORKTREE: `<WORKTREE_PARENT>/task-tests-<slug>`
>
> Implement the tests specified in the test plan on branch `task/tests-<slug>`.
> Think adversarially. For every item in the plan, cover:
> - Happy path
> - Edge cases (empty input, boundary values, type extremes)
> - Failure modes (errors, timeouts, invalid state)
> - Evasion attempts (inputs that look valid but are not)
>
> Do not write tests that merely confirm the code compiles. Write tests that
> would catch real bugs in production. Use `#[tokio::test]` for async tests.
> Commit your changes. Produce an Agent Handoff.

### Step 3 — Stress Tester (conditional)

If the test plan includes stress tests, invoke **stress-tester** with the
Test Developer handoff and branch name. Task:

> Working directory: `<WORKTREE_PARENT>/task-tests-<slug>`
> (branch: `task/tests-<slug>`) — do not read or write any
> files outside this directory.
> WORKTREE: `<WORKTREE_PARENT>/task-tests-<slug>`
>
> Implement stress, load, and concurrency tests on branch `task/tests-<slug>`.
> Cover: concurrent connections, resource exhaustion, backpressure, byte
> integrity under load, and latency distributions where relevant.
> These tests must be able to run in CI without external dependencies.
> Commit your changes. Produce an Agent Handoff.

If no stress tests were planned, skip this step.

### Step 4 — Test Runner

Invoke **test-runner** with the full test branch and implementation. Task:

> Working directory: `<WORKTREE_PARENT>/task-tests-<slug>`
> (branch: `task/tests-<slug>`) — do not read or write any
> files outside this directory. Run `cd "<WORKTREE_PARENT>/task-tests-<slug>" && cargo test`.
> WORKTREE: `<WORKTREE_PARENT>/task-tests-<slug>`
>
> Run the full test suite (`cargo test`) against the current state of
> `feature/<slug>` merged with `task/tests-<slug>`.
> Interpret every failure precisely:
> - Is this a test code problem (wrong assertion, bad fixture, flaky timing)?
> - Is this an implementation bug (real behaviour does not match spec)?
>
> Produce an Agent Handoff with:
> - Verdict: all pass | test failures | implementation bugs
> - For each failure: exact test name, failure message, diagnosis, and
>   classification (test code | implementation)

### Step 5 — Route Test Runner verdict

**If verdict is "all pass":** proceed to phase completion.

**If verdict is "test failures" (test code problems):**
Return the Test Runner handoff to **Test Developer** (or **Stress Tester** for
stress test failures). They fix the test code and re-commit. Return to Step 4.

**If verdict is "implementation bugs":**
For each implementation bug identified:

1. Extract the affected task id(s) from the implementation log.
2. Trigger a targeted re-run of the affected task(s) through the Phase 3
   per-task cycle. Pass the Test Runner handoff as context so Developer
   understands exactly what is broken.
3. Once the affected tasks are re-approved by Contrarian and merged back to
   the feature branch, return to Step 4 to re-run the full test suite.

This loop continues until Test Runner verdict is "all pass".

**Maximum test-runner iterations:** 8. If the suite has not gone green after
8 Test Runner invocations, surface the remaining failures to the user.

---

## Team cleanup

If `TeamCreate` succeeded earlier, shut down all agents and delete the team
**before** producing the Phase Handoff:

```text
SendMessage({ to: "pm",             message: {type: "shutdown_request"} })
SendMessage({ to: "architect",      message: {type: "shutdown_request"} })
SendMessage({ to: "test-developer", message: {type: "shutdown_request"} })
SendMessage({ to: "stress-tester",  message: {type: "shutdown_request"} })
SendMessage({ to: "test-runner",    message: {type: "shutdown_request"} })
TeamDelete()
```

If `TeamCreate` was never called (sequential fallback path), skip this section.

---

## Phase completion

Phase 4 is complete when:
- Test Runner verdict is "all pass"
- Test branch has been merged into the feature branch
- PM confirms all planned test types have been covered

Produce a **Phase Handoff**:

```text
=== PHASE HANDOFF ===
Phase:     Testing
Status:    complete  (or: blocked — <reason>)
Feature:   <feature description>
Branch:    <branch>
Artifacts:
  task/tests-<slug>  (merged into feature/<slug>)
Decisions: <bullet list of key testing decisions>
For next:  Feature branch is ready to merge to main.
           Tests written: <count>. Areas covered: <list>.
           Any residual known limitations or deferred test areas.
Open:      <unresolved failures or user questions, or "none">
=== END HANDOFF ===
```

When the orchestrator receives this handoff with `Status: complete`, it will
merge `feature/<slug>` into `main`.
