---
name: review
description: Run Phase 5 (Review) of the privacyclaw feature workflow. Investigator + Contrarian perform a code review of the PR against the design spec and agree on VALID comments; Developer + Simplifier + Test-Developer implement them. Cycles until the Review Group approves. Can be invoked standalone by passing a Testing Phase Handoff and PR_URL as arguments.
argument-hint: <testing phase handoff + PR_URL: <url>>
context: fork
---

# Phase 5 — Review

You are the **Phase 5 coordinator**. Your job is to run structured review
cycles until the Review Group approves the PR with zero outstanding findings.

Each cycle has two groups:

- **Review Group** (Investigator + Contrarian): reads the design spec and the
  PR code, produces findings, agrees on which are VALID, and either approves
  the PR or hands the VALID findings to the Implementation Group.
- **Implementation Group** (Developer + Simplifier + Test-Developer): implements
  every VALID finding, simplifies the fixes, updates or adds tests, and pushes
  the updated branch so the Review Group can re-examine.

Input: **$ARGUMENTS**

Extract from the input:
- `feature`: the feature description
- `branch`: the feature branch (`feature/<slug>`)
- `pr_url`: the GitHub PR URL (field `PR_URL:` in the arguments)
- `design_doc`: path to the design document (from the Design phase handoff Artifacts)
- `openspec_id`: the OpenSpec change id (from the Planning phase handoff Decisions)
- `WORKTREE`: the absolute path to the feature worktree (required; if missing,
  derive as `$(git rev-parse --show-toplevel)/../worktrees/<branch-slug>`)

Derive `slug` from the branch name (e.g. `feature/binary-bundling` → `binary-bundling`).
Derive `WORKTREE_PARENT` = `dirname(<WORKTREE>)`. Per-cycle implementation
worktrees are siblings: `<WORKTREE_PARENT>/task-review-<slug>-<N>`.

---

## Working directory

**All operations in this phase must happen inside `<WORKTREE>` (or per-cycle
sibling worktrees), never in the main repository working tree.**

Rules that apply to this coordinator and to every agent it invokes:
- File reads/writes: `<WORKTREE>/<relative-path>`
- Git commands on the feature branch: `git -C "<WORKTREE>" <command>`
- Cargo commands: `cd "<WORKTREE>" && cargo <command>`
- Per-cycle worktrees: created at `<WORKTREE_PARENT>/task-review-<slug>-<N>`
- **Every agent message must include `WORKTREE: <worktree_path>`** with the
  appropriate path so agents never touch the main repository working tree.
- The PR URL is read-only context that gives the Review Group a pointer to
  what is under review. All code work targets the feature branch in `<WORKTREE>`.

---

## Review Add-On format

Each cycle that produces VALID findings records them in an OpenSpec Add-On file:

`<WORKTREE>/openspec/changes/<openspec_id>/review-addon-<N>.md`

```text
# Review Add-On: Cycle <N>

## Valid Findings

### RC<N>-<seq>: <short title>
**Severity:** critical | major | minor
**File:** <relative-file-path>:<line>
**Finding:** <what is wrong or missing>
**Required fix:** <what must change, specific enough for Developer to act on>

### RC<N>-<seq>: ...
```

The Implementation Group reads this file. After implementation it is retained
as a permanent record in the feature branch.

---

## Agent coordination protocol

Attempt team-based coordination:

```text
TeamCreate({ name: "review-team",
             agents: ["investigator", "contrarian", "developer",
                      "simplifier", "test-developer"] })
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
  - <finding id> — <severity> — <one-line description> — VALID | INVALID
Open:
  - <item + owner, or "none">
Pass forward:
  <2–3 sentences of critical context for the next agent>
--- END HANDOFF ---
```

---

## Review cycle

Each review cycle runs the following steps. Start with N=1.

---

### Step 1 — Investigator: code review

Invoke **investigator** with the design doc, branch diff, and OpenSpec tasks.
Task:

> Working directory: `<WORKTREE>` — read files only within this directory.
> Do not access the main repository working tree.
> WORKTREE: `<worktree_path>`
> PR: `<pr_url>`
>
> Read the design document at `<design_doc>`.
> Read all files changed on this feature branch:
>   `git -C "<WORKTREE>" diff main...HEAD`
> Read the OpenSpec tasks at `<WORKTREE>/openspec/changes/<openspec_id>/tasks.md`.
> If prior review add-ons exist, read them to understand what was already addressed.
>
> Perform a thorough code review against the design specification:
> - Does every implemented piece match its design decision exactly?
> - Are there correctness bugs, edge-case gaps, or security issues?
> - Does the implementation cover all OpenSpec tasks completely and correctly?
> - Are there simplicity, maintainability, or logging problems worth raising?
> - Are there any regressions relative to prior review cycles?
>
> Produce an Agent Handoff listing every finding with:
>   - A unique id: RC<N>-<seq>
>   - Severity: critical | major | minor
>   - File and line reference
>   - What is wrong (finding) — do not propose fixes, only findings

---

### Step 2 — Contrarian: challenge and agree

Invoke **contrarian** with the Investigator handoff. Task:

> Working directory: `<WORKTREE>` — read files only within this directory.
> Do not access the main repository working tree.
> WORKTREE: `<worktree_path>`
>
> Review the Investigator's findings for cycle N.
> For each finding, read the actual code at the cited location to independently
> confirm or refute it:
>
> - Mark VALID if the code confirms the finding — the problem genuinely exists.
> - Mark INVALID if the code disproves the finding — provide the evidence
>   (exact file:line) that shows the concern does not apply.
>
> For VALID findings, confirm the required fix is clear and appropriately scoped.
> Do not accept vague findings: if a finding is correct but the required fix is
> underspecified, strengthen the fix description before marking it VALID.
>
> If there are zero VALID findings: declare the PR **approved**.
>
> Produce an Agent Handoff listing:
> - All VALID findings with confirmed fix descriptions
> - All INVALID findings with disproving evidence
> - Your verdict: approved | changes required

---

### Step 3 — Route verdict

**If Contrarian declares the PR approved (zero VALID findings):**
Skip to Phase Completion below.

**If there are VALID findings:**

1. Write the Review Add-On file:
   ```
   <WORKTREE>/openspec/changes/<openspec_id>/review-addon-<N>.md
   ```
   Using the Review Add-On format above. Include only VALID findings.

2. Commit the Add-On to the feature branch:
   ```bash
   git -C "<WORKTREE>" add \
     openspec/changes/<openspec_id>/review-addon-<N>.md
   git -C "<WORKTREE>" commit \
     -m "review(cycle-<N>): record <count> valid findings"
   ```

3. Proceed to Implementation Group (Step 4).

---

### Step 4 — Create implementation worktree

```bash
git -C "<WORKTREE>" worktree add \
  "<WORKTREE_PARENT>/task-review-<slug>-<N>" \
  -b task/review-<slug>-<N>
```

---

### Step 5 — Developer: implement review findings

Invoke **developer** with the Review Add-On and feature context. Task:

> Working directory: `<WORKTREE_PARENT>/task-review-<slug>-<N>`
> (branch: `task/review-<slug>-<N>`) — do not read or write any
> files outside this directory.
> WORKTREE: `<WORKTREE_PARENT>/task-review-<slug>-<N>`
>
> Read the review add-on at:
>   `<WORKTREE>/openspec/changes/<openspec_id>/review-addon-<N>.md`
>
> Implement every VALID finding listed (all RC<N>-* items).
> Follow project conventions: Rust 2021, tokio, anyhow for app code,
> thiserror for library crates, tracing for logging.
> Commit your changes. Produce an Agent Handoff.

---

### Step 6 — Simplifier: remove complexity from the fixes

Invoke **simplifier** with the Developer handoff. Task:

> Working directory: `<WORKTREE_PARENT>/task-review-<slug>-<N>`
> (branch: `task/review-<slug>-<N>`) — do not read or write any
> files outside this directory.
> WORKTREE: `<WORKTREE_PARENT>/task-review-<slug>-<N>`
>
> Review changes introduced in this review cycle on branch `task/review-<slug>-<N>`.
> Remove dead code, premature abstractions, unnecessary complexity introduced
> by the review fixes. Do not change behaviour.
> Commit your changes. Produce an Agent Handoff.

---

### Step 7 — Test Developer: update or add tests

Invoke **test-developer** with the Simplifier handoff. Task:

> Working directory: `<WORKTREE_PARENT>/task-review-<slug>-<N>`
> (branch: `task/review-<slug>-<N>`) — do not read or write any
> files outside this directory.
> WORKTREE: `<WORKTREE_PARENT>/task-review-<slug>-<N>`
>
> Review changes introduced in this review cycle.
> For each fix: does the existing test suite cover the corrected behaviour?
> If not, add or update tests to cover it. Think adversarially — each fix
> addresses a real bug; write a test that would have caught it.
> Run `cd "<WORKTREE_PARENT>/task-review-<slug>-<N>" && cargo test` (or
> `pytest` for Python files) to confirm all tests pass.
> Commit your changes. Produce an Agent Handoff with the test results (pass/fail).

---

### Step 8 — Merge, push, and clean up

**If all tests pass:**

```bash
git -C "<WORKTREE>" merge --no-ff task/review-<slug>-<N> \
  -m "fix(review-cycle-<N>): implement <count> review findings"
git -C "<WORKTREE>" worktree remove \
  "<WORKTREE_PARENT>/task-review-<slug>-<N>"
git -C "<WORKTREE>" push origin <branch>
```

**If tests fail:** route the Test Developer handoff back to Developer (Step 5)
for a targeted fix. This sub-loop runs a maximum of **3 times** per cycle
before surfacing the failure to the user.

---

### Step 9 — Next cycle

Increment N. Return to Step 1 (Investigator re-reviews the updated branch).

**Maximum review cycles: 5.** If the Review Group has not approved after 5
cycles, surface the remaining VALID findings to the user along with a summary
of what was fixed and what remains unresolved. Do not loop further.

---

## Team cleanup

If `TeamCreate` succeeded earlier, shut down all agents and delete the team
**before** producing the Phase Handoff:

```text
SendMessage({ to: "investigator",   message: {type: "shutdown_request"} })
SendMessage({ to: "contrarian",     message: {type: "shutdown_request"} })
SendMessage({ to: "developer",      message: {type: "shutdown_request"} })
SendMessage({ to: "simplifier",     message: {type: "shutdown_request"} })
SendMessage({ to: "test-developer", message: {type: "shutdown_request"} })
TeamDelete()
```

If `TeamCreate` was never called (sequential fallback path), skip this section.

---

## Phase completion and PR merge

Phase 5 is complete when:
- Contrarian declares the PR **approved** (zero VALID findings in the latest cycle)
- All per-cycle implementation worktrees have been merged and removed
- The feature branch has been pushed to origin with all fixes

### Merge the PR

After approval, merge the PR immediately — do not wait for the user:

```bash
gh pr merge <pr_url> --squash --delete-branch \
  --subject "feat(<slug>): <feature description>" \
  --body "Merged after review approval (cycle <N>). See review-addon-*.md for finding history."
```

Use `--squash` to keep main history linear. If the repository enforces merge
commits (branch protection rule: "Require a linear history" not set), use
`--merge` instead. If `gh pr merge` fails due to branch protection requiring
a human approver, report the PR URL to the user and ask them to merge manually —
do not loop or retry.

After a successful merge, clean up the worktree:

```bash
git -C "<WORKTREE>" checkout main
git -C "<WORKTREE>" pull origin main
git worktree remove "<WORKTREE>" --force
git branch -d feature/<slug>
```

Report to the user:
- The PR URL and merge commit SHA
- Review summary: N cycle(s), X VALID findings fixed, Y INVALID discarded
- Worktree cleaned up

### Phase Handoff

Produce a **Phase Handoff**:

```text
=== PHASE HANDOFF ===
Phase:     Review
Status:    complete  (or: blocked — <reason>)
Feature:   <feature description>
Branch:    <branch>
Artifacts:
  <list of review-addon-N.md files created, or "none — approved on first cycle">
Decisions:
  - Review ran <N> cycle(s) before approval
  - Total VALID findings addressed: <count>
  - Total INVALID findings discarded: <count>
  - <notable decisions made during implementation of findings>
For next:  PR merged to main. Worktree removed. Feature complete.
Open:      <unresolved findings or user questions, or "none">
=== END HANDOFF ===
```
