---
name: refactor
description: Orchestrate the full refactoring workflow (Investigate → Plan → Execute). Investigator + Contrarian investigate structural smells, Architect + Contrarian + PM plan and validate an OpenSpec change, then a per-task cycle (Refactoring Engineer → Simplifier → Logging Implementer → Test Runner → Contrarian) executes each one with a revert protocol. Standalone skill — not part of the implement flow.
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
OpenSpec:  refactor-<slug>
Artifacts: <newline-separated file paths>
Decisions: <bullet list of key decisions>
For next:  <2–4 sentences for the next phase>
Open:      <user items, or "none">
=== END HANDOFF ===
```

Store the Phase Log at `$WORKTREE_PATH/.claude/workflow/<slug>/phase-log.md`.
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

**OpenSpec change**: `refactor-<slug>`

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
> OpenSpec change `refactor-<slug>` scaffolded at `openspec/changes/refactor-<slug>/`.
> Proceed to Refactor-Plan? Or adjust scope?
>
> To resume here later: `/refactor SLUG: <slug> RESUME_FROM: Refactor-Plan`"

Wait for explicit confirmation. Incorporate any user-added exclusions into
the handoff passed to Refactor-Plan.

**After Phase 2 (Refactor-Plan):** pause and present the task plan. Ask:

> "Refactor-Plan complete. <N> tasks in `openspec/changes/refactor-<slug>/tasks.md`.
> `openspec validate refactor-<slug> --strict`: clean.
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

Skip if resuming (worktree and baseline already exist — verify, don't recreate):

```bash
REPO_ROOT=$(git rev-parse --show-toplevel)
WORKTREE_PATH="${REPO_ROOT}/../worktrees/refactor-${slug}"
git checkout main && git pull
git worktree add "$WORKTREE_PATH" -b refactor/<slug>
mkdir -p "$WORKTREE_PATH/.claude/workflow/<slug>"
cd "$WORKTREE_PATH" && cargo test 2>&1 | tee "$WORKTREE_PATH/.claude/workflow/<slug>/baseline-tests.txt"
```

Store `WORKTREE_PATH` — every subsequent Skill() call passes it verbatim as
`WORKTREE: <path>`. The main repository working tree is not touched again.

If resuming and worktree already exists: reconstruct `WORKTREE_PATH` from
`${REPO_ROOT}/../worktrees/refactor-${slug}` and verify it is clean and
ahead of main. If `baseline-tests.txt` is missing, re-run `cargo test`.

Tell the user: "Worktree `refactor/<slug>` ready at `$WORKTREE_PATH`. Main repo untouched. Starting Phase 1 — Refactor-Investigate."

---

## Step 2 — Invoke Phase 1: Refactor-Investigate

Skip if `RESUME_FROM` is `Refactor-Plan`, `Refactor-Execute`, or `task-<id>`.

```text
Skill("refactor-investigate", "<scope>\nBOUNDARIES: <boundaries>\nBRANCH: refactor/<slug>\nWORKTREE: <worktree_path>")
```

Wait for Phase Handoff. Append to phase log.

**If `Status: complete — no smells found`:** report to user and stop.

**Post Phase 1 progress report. User check-in.**

Incorporate user adjustments as an amendment note appended to the Refactor-Investigate
handoff before passing to Refactor-Plan.

---

## Step 3 — Invoke Phase 2: Refactor-Plan

Skip if `RESUME_FROM` is `execute` or `task-<id>`. When resuming at
`Refactor-Plan`, load the Refactor-Investigate handoff from `$WORKTREE_PATH/.claude/workflow/<slug>/smell-catalog.md`
and the OpenSpec change-id from `$WORKTREE_PATH/openspec/changes/refactor-<slug>/`.

```text
Skill("refactor-plan", "<Refactor-Investigate handoff content>\nUSER_EXCLUSIONS: <any user adjustments>\nWORKTREE: <worktree_path>")
```

Wait for Phase Handoff. Append to phase log.

**If `Status: blocked`:** surface unresolved Contrarian challenges to user.
Wait for direction before continuing.

**Post Phase 2 progress report. User check-in.**

---

## Step 4 — Invoke Phase 3: Refactor-Execute

When resuming at `refactor-execute` or `task-<id>`, load the Refactor-Plan handoff from
`$WORKTREE_PATH/openspec/changes/refactor-<slug>/tasks.md` (the OpenSpec tasks file is the source of truth).
Append `RESUME_FROM: task-<id>` to the arguments if resuming mid-execution.

```text
Skill("refactor-execute", "<Refactor-Plan handoff content>\nWORKTREE: <worktree_path>")
```

Wait for Phase Handoff. Append to phase log.

**Post Phase 3 progress report.** Note any reverted or blocked tasks clearly.

---

## Step 5 — Final report and GitHub PR

```bash
cd "$WORKTREE_PATH" && cargo test 2>&1 | tee "$WORKTREE_PATH/.claude/workflow/<slug>/final-tests.txt"
cd "$WORKTREE_PATH" && openspec validate refactor-<slug> --strict
```

Then create the GitHub Pull Request:

```bash
cd "$WORKTREE_PATH"
gh pr create \
  --base main \
  --head refactor/<slug> \
  --title "chore: refactor <scope>" \
  --body "$(cat <<'PREOF'
## Refactor: <scope>

No behavior change. Structural improvements only.

## Smells Addressed
<bullet list from smell catalog: location, severity, description>

## Tasks
- Merged: <N>
- Reverted: <N> — <list with reason, or "none">
- Blocked: <N> — <list with reason, or "none">

## Behavior Guarantee
- Baseline: <X tests passing>
- Final: <X tests passing>

## Artifacts
- Smell catalog: `.claude/workflow/<slug>/smell-catalog.md`
- Refactor log: `.claude/workflow/<slug>/refactor-log.md`
- OpenSpec change: `openspec/changes/refactor-<slug>/`
- Phase log: `.claude/workflow/<slug>/phase-log.md`

## Open Items
<blocked/reverted tasks, or "none">

---
🤖 Generated with [Claude Code](https://claude.com/claude-code) \`/refactor\`
PREOF
)"
```

Report to the user:

```text
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✅ Refactor complete — <scope>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

PR: <URL from gh pr create>
Branch: refactor/<slug>  (worktree: $WORKTREE_PATH)
Phase log: $WORKTREE_PATH/.claude/workflow/<slug>/phase-log.md

Tasks merged:   <N>
Tasks reverted: <N> — <list with reason, or "none">
Tasks blocked:  <N> — <list with reason, or "none">

Smells addressed: <count>
Smells remaining: <count and locations>

Behavior guarantee:
  Baseline: <X tests passing>
  Final:    <X tests passing>

OpenSpec change: refactor-<slug>
  Validate: <clean | issues remaining>

After the PR is merged on GitHub:
  git worktree remove "$WORKTREE_PATH"
  git branch -d refactor/<slug>
  openspec archive refactor-<slug> --yes
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

## Orchestration rules

- Invoke phases in order. Never start a phase before the previous returns a
  complete handoff.
- Pass only the Phase Handoff to the next phase — not the full phase log.
- If a phase returns a malformed handoff, ask the user whether to re-invoke
  or proceed manually.
