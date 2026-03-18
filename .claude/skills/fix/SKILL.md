---
name: fix
description: Orchestrate the full bug-fix workflow (Fix-Investigate → Planning → Development → Testing). Investigator + Contrarian find and validate the root cause, Plan produces a minimal OpenSpec task list, Develop implements it, Test covers regression. User gates after investigation (root cause confirmed?) and after planning (scope approved?). Standalone skill — not part of the implement flow.
argument-hint: "<symptom description> [DO NOT TOUCH: <boundaries>]"
allowed-tools: Bash, Skill, Read, Write
---

# Orchestrator — fix

You are the **fix orchestrator**. Your job is narrow: set up git, invoke each
phase skill in order, pass compact handoffs between them, maintain the Phase
Log, and surface decisions to the user at the two mandatory gates. You do not
investigate, design, or implement anything yourself.

Input: **$ARGUMENTS**

Extract from the input:
- `symptom`: the bug description
- `boundaries`: "DO NOT TOUCH" zones. Record as `none` if omitted.

Derive a slug from the symptom (lowercase, hyphens, max 40 chars).

---

## Design notes

This orchestrator mirrors the `implement` architecture, with one key
difference: Phase 1 is Fix-Investigate (root cause analysis) instead of
Design (feature design). Phases 2–4 reuse `plan`, `develop`, and `test`
verbatim — the RCA document serves as the "design doc" that `plan` consumes.

- **Context isolation per phase.** Each phase skill runs in a forked subagent
  context. The orchestrator is the only agent that holds the Phase Log.
- **Compact handoffs.** Each phase skill returns a structured Phase Handoff.
  The orchestrator passes only that document to the next phase.
- **Two mandatory user gates.** After Fix-Investigate (was the root cause
  found correctly?) and after Planning (is the fix scope minimal and correct?).
  The user must explicitly approve before the orchestrator continues.
- **Minimal scope principle.** Every phase must be reminded: this is a fix,
  not a feature. The goal is the smallest correct change. Scope creep is a bug.
- **No auto-merge.** The user reviews and merges `fix/<slug>` manually after
  Testing completes, to keep control of what lands on `main`.

---

## Phase Handoff format

```text
=== PHASE HANDOFF ===
Phase:     <Fix-Investigate | Planning | Development | Testing>
Status:    <complete | blocked — reason>
Feature:   fix: <symptom description>
Branch:    fix/<slug>
Artifacts: <newline-separated list of file paths created or modified>
Decisions: <bullet list of key decisions made in this phase>
For next:  <2–4 sentences: what the next phase needs to know>
Open:      <bullet list of items needing user input, or "none">
=== END HANDOFF ===
```

Store the Phase Log at `.claude/workflow/<slug>/phase-log.md`. Append each
handoff as it arrives.

---

## User progress report format

After every phase completes:

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✅ Phase <N> — <Phase Name> complete
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

**What happened**
<2–4 sentences: what agents ran, what they found, where they disagreed and
how it was resolved. Name the agents. Be concrete — e.g. "Investigator traced
the panic to a wrong state assumption at proxy/intercept.rs:142; Contrarian
challenged whether the upstream reset at :89 was the deeper cause and after
re-reading both agreed the fix surface is :142 only.">

**Key decisions**
<bullet list, each with one-line rationale>

**Artifacts created / modified**
<bullet list from handoff Artifacts field, one-sentence description each>

**What goes to Phase <N+1>**
<verbatim "For next:" from the Phase Handoff>

**Open items** (omit if none)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

## User check-in policy

**After Phase 1 (Fix-Investigate):** always pause. This is the most important
gate — if the root cause is wrong, everything downstream is wasted. Present:

> "Fix-Investigate complete.
>
> Root cause: `<file:line>` — `<one-line mechanism>`
> Confidence: <high | medium | low>
> Fix surface: <N> files — `<file list>`
> Contrarian rounds: <N> — <consensus reached | unresolved items>
> RCA: `.claude/workflow/<slug>/rca.md`
>
> Does this match your understanding of the bug? Proceed to Planning?
>
> To resume here later: `/fix SLUG: <slug> RESUME_FROM: Planning`"

Wait for explicit confirmation. If confidence is medium or low, flag it:

> "Note: root cause confidence is <level>. Contrarian and Investigator
> <brief explanation of what they could not fully resolve>. You may want to
> review the RCA before proceeding."

Incorporate any user corrections as an amendment note appended to the
Fix-Investigate handoff before passing to Planning.

**After Phase 2 (Planning):** pause and present the task plan:

> "Planning complete. OpenSpec change `<id>` has `<N>` tasks.
> `openspec validate <id> --strict`: clean.
> Fix scope: `<bullet list of task titles>`.
>
> Does this scope look minimal and correct? Proceed to Development?
>
> To resume here later: `/fix SLUG: <slug> RESUME_FROM: Development`"

Wait for explicit confirmation. If you notice that Planning has expanded scope
beyond the fix surface (tasks unrelated to the root cause), flag it:

> "Warning: the task list includes `<task>` which was not part of the fix
> surface identified in the RCA. This may be scope creep. Confirm if intended."

**After Phase 3 (Development):** post the progress report and proceed
automatically to Testing.

**After Phase 4 (Testing):** post the progress report and proceed to the
final report automatically.

---

## Step 1 — Git setup and baseline

```bash
git checkout main && git pull
git checkout -b fix/<slug>
mkdir -p .claude/workflow/<slug>
cargo test 2>&1 | tee .claude/workflow/<slug>/baseline-tests.txt
```

If the baseline is broken (pre-existing test failures), record them. Do not
block — fixes may be made on a broken baseline — but note the pre-existing
failures clearly so Phase 3 (Development) and Phase 4 (Testing) can
distinguish them from regressions introduced by the fix.

Tell the user: "Branch `fix/<slug>` created. Starting Phase 1 — Fix-Investigate."

---

## Step 2 — Invoke Phase 1: Fix-Investigate

```text
Skill("fix-investigate", "<symptom>\nBOUNDARIES: <boundaries>\nBRANCH: fix/<slug>")
```

Wait for Phase Handoff. Append to phase log.

**If `Status: complete — no root cause found`:** report to user and stop.
The symptom may require more context or a different investigation approach.

**Post Phase 1 progress report. User check-in (mandatory).**

Incorporate any user corrections as an amendment note appended to the
Fix-Investigate handoff before passing to Planning.

---

## Step 3 — Invoke Phase 2: Planning

Tell the user: "Starting Phase 2 — Planning."

Pass the Fix-Investigate handoff as input, with explicit framing:

```text
Skill("plan", "<fix-investigate handoff content>
FRAMING: This is a bug fix, not a feature. The RCA at .claude/workflow/<slug>/rca.md
is the design document. Planning must scope tasks to the minimal fix surface
identified in the RCA — do not expand scope beyond what the root cause requires.
Prefer a single task unless the fix surface genuinely requires sequenced changes.
USER_AMENDMENTS: <any user corrections from the Phase 1 gate>")
```

Wait for Phase Handoff. Append to phase log.

**If `Status: blocked`:** surface unresolved Planning issues to user. Wait for
direction before continuing.

**Post Phase 2 progress report. User check-in (mandatory).**

---

## Step 4 — Invoke Phase 3: Development

Tell the user: "Starting Phase 3 — Development. This phase implements the fix — it may take a while."

Pass the Planning handoff as input, with framing:

```text
Skill("develop", "<planning handoff content>
FRAMING: This is a bug fix. The fix must be minimal — implement exactly what
the RCA identified, nothing more. Do not refactor surrounding code unless it
directly interferes with the fix. Branch is fix/<slug>.")
```

Wait for Phase Handoff. Append to phase log.

**Post Phase 3 progress report.** Proceed automatically to Testing.

If Development reports a blocked task, surface it to the user before continuing.

---

## Step 5 — Invoke Phase 4: Testing

Tell the user: "Starting Phase 4 — Testing."

Pass the Development handoff as input, with framing:

```text
Skill("test", "<development handoff content>
FRAMING: This is a bug fix. Testing must include:
1. A regression test that would have caught this specific bug before the fix —
   test the exact scenario described in the symptom.
2. Verification that the fix does not break adjacent behavior.
Stress tests are NOT needed unless the root cause was a concurrency issue.
Branch is fix/<slug>.")
```

Wait for Phase Handoff. Append to phase log.

**Post Phase 4 progress report.**

---

## Step 6 — Final report

```bash
cargo test 2>&1 | tee .claude/workflow/<slug>/final-tests.txt
```

Compare against baseline. Report to the user:

```text
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✅ Fix complete — <symptom description>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Branch: fix/<slug>
Phase log: .claude/workflow/<slug>/phase-log.md

Root cause: <file:line> — <one-line mechanism>
Fix surface: <N files changed>

Behavior guarantee:
  Baseline: <X tests passing, Y pre-existing failures>
  Final:    <X tests passing>
  Regression test: added ✓

OpenSpec change: <id>
  Tasks:    openspec/changes/<id>/tasks.md
  Validate: <clean | issues remaining>

To merge:
  git checkout main && git merge --no-ff fix/<slug>

After merge:
  openspec archive <id> --yes
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

## Resuming an interrupted run

If `RESUME_FROM: Planning | Development | Testing` is passed as an argument:
- Load the Fix-Investigate handoff from `.claude/workflow/<slug>/rca.md` and
  the phase log from `.claude/workflow/<slug>/phase-log.md`
- Skip phases that are already complete per the phase log
- Verify the fix branch exists: `git branch --list fix/<slug>`
- Pick up from the named phase

---

## Orchestration rules

- Invoke phases strictly in order. Never start a phase before the previous
  returns a complete handoff.
- Pass only the Phase Handoff to the next phase — not the full phase log.
- Inject the `FRAMING` note into every `plan`, `develop`, and `test` call
  to enforce minimal fix scope.
- If a phase skill returns a malformed or missing handoff, ask the user
  whether to re-invoke the phase or proceed manually.
- If the fix surface expands during Planning or Development beyond what the
  RCA identified, flag it to the user immediately. Scope creep in a fix branch
  is a risk to stability.
