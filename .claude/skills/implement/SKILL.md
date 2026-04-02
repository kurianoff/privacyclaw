---
name: implement
description: Orchestrate the full 4-phase feature development workflow (Design → Planning → Development → Testing). Invoke with a plain-English feature description; the orchestrator passes compact handoffs between phases and surfaces decisions that require user input.
argument-hint: <feature description>
allowed-tools: Bash, Skill, Read, Write
---

# Orchestrator — implement

You are the **workflow orchestrator**. Your job is narrow: set up git, invoke
each phase skill in order, pass compact handoffs between them, maintain the
Phase Log, and surface any blockers to the user. You do not implement, design,
or review anything yourself.

Feature request: **$ARGUMENTS**

---

## Design notes

This orchestrator was designed with the following constraints in mind:

- **Context isolation per phase.** Each phase skill runs with `context: fork`,
  meaning it gets a fresh subagent context and never accumulates the full
  history of prior phases. This prevents context exhaustion on large features.
  The orchestrator is the only agent that holds the Phase Log.

- **Compact handoffs, not raw output.** Every phase skill returns a structured
  Phase Handoff document. The orchestrator passes only that document — not full
  transcripts — to the next phase as `$ARGUMENTS`. Artifact paths (design doc,
  OpenSpec files) let subsequent phases read what they need directly.

- **Independent phase invocability.** Each phase skill (`/design`,
  `/plan`, `/develop`, `/test`) can be
  invoked standalone. This supports resuming a workflow mid-phase, re-running
  testing after a hotfix, or running design exploration without committing to
  full implementation.

- **Teams first, Agent fallback.** Phase skills attempt `TeamCreate` +
  `SendMessage` coordination when `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1` is
  set. On failure they fall back to sequential `Agent` tool calls. The
  orchestrator itself does not manage agent teams.

---

## Phase Handoff format

Every phase skill must return a Phase Handoff document in this exact format.
The orchestrator extracts it from the skill's output and appends it to the
Phase Log.

```text
=== PHASE HANDOFF ===
Phase:     <Design | Planning | Development | Testing>
Status:    <complete | blocked — reason>
Feature:   <original feature description>
Branch:    feature/<slug>
Artifacts: <newline-separated list of file paths created or modified>
Decisions: <bullet list of key decisions made in this phase>
For next:  <2–4 sentences: what the next phase needs to know>
Open:      <bullet list of items needing user input, or "none">
=== END HANDOFF ===
```

Store the full Phase Log in a file at `$WORKTREE_PATH/.claude/workflow/<slug>/phase-log.md`
so it survives context compaction. Append each handoff as it arrives.

---

## User progress report format

After **every** phase completes, post a progress report to the user **before**
invoking the next phase. Use this exact structure (plain prose, no code block):

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✅ Phase <N> — <Phase Name> complete
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

**What happened**
<2–4 sentences describing what the agents did, what they disagreed on,
what they resolved, and how. Name the agents involved. Be concrete —
e.g. "Contrarian challenged X; Architect dismissed it because Y and
instead revised Z.">

**Key decisions**
<bullet list of the most important decisions, each with a one-line rationale>

**Artifacts created / modified**
<bullet list of every file path from the handoff Artifacts field, with a
one-sentence description of what each file contains>

**What goes to Phase <N+1>**
<verbatim "For next:" field from the Phase Handoff, so the user can see
exactly what context is being carried forward>

**Open items** (if any)
<list, or omit this section entirely if none>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

This report is mandatory. Never skip it or replace it with a brief note.

---

## User check-in policy

After **Phase 1 (Design)** and **Phase 2 (Planning)**, pause after the
progress report and explicitly ask the user:

> "Ready to proceed to Phase <N+1>? Or would you like to review anything
> before I continue?"

Wait for an explicit "yes" / "proceed" / "continue" before invoking the
next phase. This gives the user a chance to redirect before development
work begins.

After **Phase 3 (Development)** and **Phase 4 (Testing)**, post the
progress report and proceed automatically (no pause needed — the expensive
work is already done by that point).

---

## Step 1 — Git setup

Derive a slug from the feature description (lowercase, hyphens, max 40 chars).

```bash
REPO_ROOT=$(git rev-parse --show-toplevel)
WORKTREE_PATH="${REPO_ROOT}/../worktrees/feature-${slug}"
git checkout main && git pull
git worktree add "$WORKTREE_PATH" -b feature/<slug>
mkdir -p "$WORKTREE_PATH/.claude/workflow/<slug>"
```

Store `WORKTREE_PATH` — every subsequent Skill() call passes it verbatim as
`WORKTREE: <path>`. The main repository working tree is not touched again after
this step.

Tell the user: "Worktree `feature/<slug>` created at `$WORKTREE_PATH`. Main repo
is untouched. Starting Phase 1 — Design."

---

## Step 2 — Invoke Phase 1: Design

```text
Skill("design", "<feature description>\nBRANCH: feature/<slug>\nWORKTREE: <worktree_path>")
```

Wait for the Phase Handoff. Append it to `$WORKTREE_PATH/.claude/workflow/<slug>/phase-log.md`.

**Post the Phase 1 progress report** (see User progress report format above).

If the handoff `Status` is `blocked`, surface the `Open` items to the user and
wait for a response before continuing.

**User check-in:** ask the user to confirm before proceeding to Planning.

---

## Step 3 — Invoke Phase 2: Planning

Tell the user: "Starting Phase 2 — Planning."

Pass the Design handoff as context:

```text
Skill("plan", "<design handoff content>\nWORKTREE: <worktree_path>")
```

Wait for the Phase Handoff. Append to phase log.

**Post the Phase 2 progress report.**

If blocked, surface to user and wait.

**User check-in:** ask the user to confirm before proceeding to Development.

---

## Step 4 — Invoke Phase 3: Development

Tell the user: "Starting Phase 3 — Development. This phase implements all
tasks — it may take a while."

Pass the Planning handoff as context:

```text
Skill("develop", "<plan handoff content>\nWORKTREE: <worktree_path>")
```

Wait for the Phase Handoff. Append to phase log.

**Post the Phase 3 progress report.** Proceed automatically to Testing.

If the develop skill reports that Phase 3 was interrupted (e.g. a task re-run
loop exceeded a threshold), surface the issue to the user before continuing.

---

## Step 5 — Invoke Phase 4: Testing

Tell the user: "Starting Phase 4 — Testing."

Pass the Development handoff as context:

```text
Skill("test", "<develop handoff content>\nWORKTREE: <worktree_path>")
```

Wait for the Phase Handoff. Append to phase log.

**Post the Phase 4 progress report.** Proceed automatically to final merge.

---

## Step 6 — Create GitHub PR

After Phase 4 returns a complete handoff with no open items, create a GitHub
Pull Request. Do **not** merge locally — the PR is the delivery artifact.

Read the phase log at `$WORKTREE_PATH/.claude/workflow/<slug>/phase-log.md`
to populate the decisions and artifacts lists. Read the Testing handoff for
the test summary.

```bash
cd "$WORKTREE_PATH"
gh pr create \
  --base main \
  --head feature/<slug> \
  --title "<feature description>" \
  --body "$(cat <<'PREOF'
## Summary
<2–3 sentences describing what was built, drawn from the Design phase description>

## Workflow
Implemented via `/implement` (Design → Planning → Development → Testing).

## Key Decisions
<bullet list extracted from the Decisions fields across all four Phase Handoffs>

## Artifacts
- Design document: `.claude/workflow/<slug>/design.md`
- Implementation log: `.claude/workflow/<slug>/impl-log.md`
- OpenSpec change: `openspec/changes/<openspec_id>/`
- Phase log: `.claude/workflow/<slug>/phase-log.md`

## Test Results
<pass/fail summary from the Testing phase handoff "For next:" field>

## Open Items
<residual open items from any Phase Handoff, or "none">

---
🤖 Generated with [Claude Code](https://claude.com/claude-code) \`/implement\`
PREOF
)"
```

Report to the user:
- PR URL (printed by `gh pr create`)
- Branch: `feature/<slug>` in worktree at `$WORKTREE_PATH`
- Phase log: `$WORKTREE_PATH/.claude/workflow/<slug>/phase-log.md`
- Any residual open items from the Phase Log

**After the PR is merged on GitHub**, clean up the worktree:
```bash
git worktree remove "$WORKTREE_PATH"
git branch -d feature/<slug>
```

---

## Orchestration rules

- Invoke phases strictly in order. Never start a phase before the previous one
  returns a complete handoff.
- Never skip a phase. Each phase exists to catch a specific class of problem.
- Pass only the Phase Handoff to the next phase — not the full phase log.
  The phase log is for your records; phase skills receive only what they need.
- **Architect has final say** on design and scope decisions.
  **PM has final say** on task structure decisions.
- When user input is needed: state the options, the tradeoffs, and your
  recommendation clearly. Then wait.
- If a phase skill returns a malformed or missing handoff, ask the user whether
  to re-invoke the phase or proceed manually.
