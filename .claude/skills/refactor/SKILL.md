---
name: refactor
description: Orchestrate the full refactoring workflow (Investigate → Plan → Execute). Investigator + Contrarian investigate structural smells, Architect + Contrarian plan a task list, then a per-task cycle (Refactoring Engineer → Simplifier → Logging Implementer → Test Runner → Contrarian) executes each one with a revert protocol. Standalone skill — not part of the implement flow.
argument-hint: "<scope> [DO NOT TOUCH: <boundaries>] [RESUME_FROM: refactor-investigate|refactor-plan|refactor-execute|task-<id>] [SLUG: <existing-slug>]"
context: fork
---

# Orchestrator — refactor

You are the **refactor orchestrator**. Your job is narrow: set up git, invoke
each phase skill in order, pass compact handoffs between them, maintain the
Phase Log, and surface decisions to the user at the two mandatory gates. You
do not catalog, plan, or implement anything yourself.

Input: **$ARGUMENTS**

Extract from the input:
- `scope`: the code area to refactor
- `boundaries`: "DO NOT TOUCH" zones. Record as `none` if omitted.
- `RESUME_FROM`: optional — `refactor-investigate`, `refactor-plan`, `refactor-execute`, or `task-<id>`
- `SLUG`: required when resuming — the existing run's slug

Derive a slug from the scope (lowercase, hyphens, max 40 chars).
When resuming, use the provided `SLUG` — do not re-derive it.

---

## Design notes

This orchestrator mirrors the `implement` and `modernize` architecture:

- **Context isolation per phase.** Each phase skill runs in a forked subagent
  context. The orchestrator is the only agent that holds the Phase Log.
- **Compact handoffs.** Each phase skill returns a structured Phase Handoff.
  The orchestrator passes only that document to the next phase.
- **Independent phase invocability.** Each sub-skill (`/refactor-investigate`, `/refactor-plan`,
  `/refactor-execute`) can be invoked standalone. This is the resume mechanism — if a
  run is interrupted after a phase completes, re-invoke from that sub-skill
  directly rather than re-running the whole workflow.
- **Two mandatory user gates** — after Investigate and after Plan. The user
  must confirm before the orchestrator proceeds.
- **No auto-merge.** The user reviews and merges `refactor/<slug>` manually.

---

## Phase Handoff format

```text
=== PHASE HANDOFF ===
Phase:     <Refactor-Investigate | Refactor-Plan | Refactor-Execute>
Status:    <complete | blocked — reason>
Scope:     <scope>
Branch:    refactor/<slug>
Artifacts: <newline-separated file paths>
Decisions: <bullet list of key decisions>
For next:  <2–4 sentences for the next phase>
Open:      <user items, or "none">
=== END HANDOFF ===
```

Store the Phase Log at `.claude/workflow/<slug>/phase-log.md`.
Append each handoff as it arrives.

---

## User progress report format

After every phase completes:

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✅ Phase <N> — <Phase Name> complete
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

**What happened**
<2–4 sentences: what agents ran, what they found or built, what Contrarian
challenged and how Architect responded. Be concrete.>

**Key decisions**
<bullet list with one-line rationale each>

**Artifacts**
<bullet list from handoff Artifacts, one-sentence description each>

**What goes to Phase <N+1>**
<verbatim "For next:" from the Phase Handoff>

**Open items** (omit if none)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

## User check-in policy

**After Phase 1 (Refactor-Investigate):** pause and present the smell summary. Ask:

> "Refactor-Investigate complete. Found <N> smells (high: <N>, medium: <N>, low: <N>).
> Excluded by Contrarian: <list or none>.
> Proceed to Refactor-Plan? Or adjust scope?
>
> To resume here later: `/refactor SLUG: <slug> RESUME_FROM: Refactor-Plan`"

Wait for explicit confirmation. Incorporate any user-added exclusions into
the handoff passed to Refactor-Plan.

**After Phase 2 (Refactor-Plan):** pause and present the task plan. Ask:

> "Refactor-Plan complete. <N> tasks planned across <N> files.
> Contrarian challenges resolved: <summary>.
> Unresolved critical items (if any): <list>.
> Proceed to Execute?
>
> To resume here later: `/refactor SLUG: <slug> RESUME_FROM: refactor-execute`"

Wait for explicit confirmation.

**After Phase 3 (Execute):** post the progress report and proceed to the
final report automatically.

---

## Step 1 — Git setup and baseline

Skip if resuming (branch and baseline already exist — verify, don't recreate):

```bash
git checkout main && git pull
git checkout -b refactor/<slug>
mkdir -p .claude/workflow/<slug>
cargo test 2>&1 | tee .claude/workflow/<slug>/baseline-tests.txt
```

If resuming and branch already exists: verify it is clean and ahead of main.
If `.claude/workflow/<slug>/baseline-tests.txt` is missing, re-run `cargo test`.

Tell the user: "Branch `refactor/<slug>` ready. Starting Phase 1 — Refactor-Investigate."

---

## Step 2 — Invoke Phase 1: Refactor-Investigate

Skip if `RESUME_FROM` is `Refactor-Plan`, `Refactor-Execute`, or `task-<id>`.

```text
Skill("refactor-investigate", "<scope>\nBOUNDARIES: <boundaries>\nBRANCH: refactor/<slug>")
```

Wait for Phase Handoff. Append to phase log.

**If `Status: complete — no smells found`:** report to user and stop.

**Post Phase 1 progress report. User check-in.**

Incorporate user adjustments as an amendment note appended to the Refactor-Investigate
handoff before passing to Refactor-Plan.

---

## Step 3 — Invoke Phase 2: Refactor-Plan

Skip if `RESUME_FROM` is `execute` or `task-<id>`. When resuming at
`Refactor-Plan`, load the Refactor-Investigate handoff from `.claude/workflow/<slug>/smell-catalog.md`.

```text
Skill("refactor-plan", "<Refactor-Investigate handoff content>\nUSER_EXCLUSIONS: <any user adjustments>")
```

Wait for Phase Handoff. Append to phase log.

**If `Status: blocked`:** surface unresolved Contrarian challenges to user.
Wait for direction before continuing.

**Post Phase 2 progress report. User check-in.**

---

## Step 4 — Invoke Phase 3: Refactor-Execute

When resuming at `refactor-execute` or `task-<id>`, load the Refactor-Plan handoff from
`.claude/workflow/<slug>/task-list.md`. Append `RESUME_FROM: task-<id>` to
the arguments if resuming mid-execution.

```text
Skill("refactor-execute", "<Refactor-Plan handoff content>")
```

Wait for Phase Handoff. Append to phase log.

**Post Phase 3 progress report.** Note any reverted or blocked tasks clearly.

---

## Step 5 — Final report

```bash
cargo test 2>&1 | tee .claude/workflow/<slug>/final-tests.txt
```

Report to the user:

```text
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✅ Refactor complete — <scope>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Branch: refactor/<slug>
Phase log: .claude/workflow/<slug>/phase-log.md

Tasks merged:   <N>
Tasks reverted: <N> — <list with reason, or "none">
Tasks blocked:  <N> — <list with reason, or "none">

Smells addressed: <count>
Smells remaining: <count and locations>

Behavior guarantee:
  Baseline: <X tests passing>
  Final:    <X tests passing>

To merge:
  git checkout main && git merge --no-ff refactor/<slug>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

## Orchestration rules

- Invoke phases in order. Never start a phase before the previous returns a
  complete handoff.
- Pass only the Phase Handoff to the next phase — not the full phase log.
- If a phase returns a malformed handoff, ask the user whether to re-invoke
  or proceed manually.
