---
name: plan
description: Run Phase 2 (Planning) of the privacyclaw feature workflow. Coordinates PM, Architect, Investigator, and Contrarian to produce a validated OpenSpec task list from a Design Document. Can be invoked standalone by passing a Design Phase Handoff as the argument.
argument-hint: <design phase handoff>
context: fork
---

# Phase 2 — Planning

You are the **Phase 2 coordinator**. Your job is to turn the Design Document
into a complete, validated OpenSpec task list that the Development phase can
execute without ambiguity.

Input: **$ARGUMENTS**

Extract from the input:
- `feature`: the feature description
- `branch`: the feature branch (`feature/<slug>`)
- `design_doc`: path to the Design Document (from the Design handoff `Artifacts`)
- `decisions`: key design decisions from the Design handoff
- `for_next`: context the Planning phase needs from Design
- `WORKTREE`: the absolute path to the isolated workflow worktree (required;
  if missing, derive as `$(git rev-parse --show-toplevel)/../worktrees/<branch-slug>`)

---

## Working directory

**All operations in this phase must happen inside `<WORKTREE>`, never in the
main repository working tree.**

Rules that apply to this coordinator and to every agent it invokes:
- File reads/writes: use the absolute path `<WORKTREE>/<relative-path>`
- openspec commands: `cd "<WORKTREE>" && openspec <command>`
- **Every agent message must include `WORKTREE: <worktree_path>`** so agents
  apply the same rule without ambiguity.

---

## Agent coordination protocol

Try team-based coordination first:

```text
TeamCreate({ name: "plan-team", agents: ["pm", "architect", "investigator", "contrarian"] })
SendMessage({ to: "pm", message: "<task + context>" })
```

Use `SendMessage` to pass Agent Handoffs between agents. Fall back to
sequential `Agent` tool calls if `TeamCreate` fails. Do not retry teams more
than once.

---

## Agent Handoff format

Same format used in all phases:

```text
--- AGENT HANDOFF ---
From:     <agent name>
To:       <next agent>
Status:   complete | blocked
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

## Workflow

### Step 1 — PM + Architect: joint planning kickoff

Invoke **pm** and **architect** together to study the Design Document and agree
on the implementation approach before any task list is drafted. Task:

> PM: read the Design Document at `<design_doc>` and the design decisions from
> the handoff. Identify the major implementation areas and likely task groupings.
>
> Architect: review the same document. For each area PM identifies, assess:
> what existing code is touched, what the natural implementation order is, and
> whether any areas carry hidden complexity that affects scope.
>
> Together, agree on: the implementation areas, their order, and any scope
> boundaries. Produce a joint Agent Handoff that documents the agreed approach.
> This becomes the blueprint PM uses to draft tasks.

### Step 2 — PM: create OpenSpec proposal

Invoke **pm** with the joint kickoff handoff and Design Document. Task:

> Working directory: `<WORKTREE>` — read and write files only within this
> directory. Do not access the main repository working tree.
> WORKTREE: `<worktree_path>`
>
> Using the agreed approach from the planning kickoff, run
> `cd "<WORKTREE>" && openspec proposal` to scaffold a new change. Create:
> - `<WORKTREE>/openspec/changes/<id>/proposal.md`
> - `<WORKTREE>/openspec/changes/<id>/design.md` (if the design warrants it)
> - `<WORKTREE>/openspec/changes/<id>/tasks.md` — first draft task list
>
> Tasks must be small, ordered, and independently verifiable.
> Each task must include: what to implement, how to verify it is done.
> Produce an Agent Handoff with the OpenSpec change id and task count.

### Step 2 — Architect: review task list

Invoke **architect** with the PM handoff and task list path. Task:

> Working directory: `<WORKTREE>` — read and write files only within this
> directory. Do not access the main repository working tree.
> WORKTREE: `<worktree_path>`
>
> Review `<WORKTREE>/openspec/changes/<id>/tasks.md`. Flag:
> - Missing tasks (things the Design requires that have no task)
> - Wrong sequencing (dependencies not respected)
> - Under-specified tasks (not enough detail to implement without guessing)
> - Tasks that are too large (should be split)
>
> Produce an Agent Handoff listing every issue found.

### Step 3 — PM ↔ Architect cycle

Pass the Architect handoff back to **pm**. PM updates the task list to address
every Architect finding. Then invoke Architect again to re-review.

Repeat until the Architect's handoff contains **no open items**.

**PM has final say on task structure.** If PM and Architect disagree on how to
structure a task, PM's decision stands unless Architect raises a dependency or
correctness concern (not a preference).

### Step 4 — Investigator and Contrarian: independent review

Invoke **investigator** and **contrarian** independently (in parallel if teams
are available) against the current task list. Each produces an Agent Handoff.

**Investigator** task:

> Working directory: `<WORKTREE>` — search and read files only within this
> directory. Do not access the main repository working tree.
> WORKTREE: `<worktree_path>`
>
> Read `<WORKTREE>/openspec/changes/<id>/tasks.md`. Identify tasks that:
> - Touch code paths with hidden complexity not reflected in the task scope
> - Have implicit dependencies on other tasks not marked
> - Make assumptions about current behaviour that may be wrong
> Produce an Agent Handoff with findings. Do not propose fixes.

**Contrarian** task:

> Working directory: `<WORKTREE>` — read files only within this directory.
> Do not access the main repository working tree.
> WORKTREE: `<worktree_path>`
>
> Read `<WORKTREE>/openspec/changes/<id>/tasks.md` and the Design Document.
> Challenge:
> - Tasks that are too optimistic about implementation effort
> - Missing tasks for error handling, edge cases, or rollback
> - The overall sequencing — is there a better order?
> - Anything that would cause Phase 3 to stall mid-way
> Produce an Agent Handoff.

### Step 5 — Architect: filter and route feedback

Pass both the Investigator and Contrarian handoffs to **architect**. Task:

> For each finding from Investigator and Contrarian: decide whether it requires
> a task change. Route actionable findings to PM. Dismiss non-actionable items
> with a rationale. Produce an Agent Handoff with routing decisions.

Pass Architect's routing decisions to **pm**. PM creates, updates, or deletes
tasks accordingly and produces a final Agent Handoff.

### Step 6 — User escalation

If scope, priority, or edge-case decisions arise that require user input:

1. Collect all pending questions.
2. Return a Phase Handoff with `Status: blocked` and questions in `Open`.

### Step 7 — Validate

Run:

```bash
cd "<WORKTREE>" && openspec validate <id> --strict
```

Resolve every issue reported before proceeding. If validation cannot be made
to pass, return a blocked Phase Handoff with the validation errors in `Open`.

---

## Team cleanup

If `TeamCreate` succeeded earlier, shut down all agents and delete the team
**before** producing the Phase Handoff:

```text
SendMessage({ to: "pm",           message: {type: "shutdown_request"} })
SendMessage({ to: "architect",    message: {type: "shutdown_request"} })
SendMessage({ to: "investigator", message: {type: "shutdown_request"} })
SendMessage({ to: "contrarian",   message: {type: "shutdown_request"} })
TeamDelete()
```

If `TeamCreate` was never called (sequential fallback path), skip this section.

---

## Phase completion

Phase 2 is complete when:
- `openspec validate <id> --strict` passes with no issues
- Architect has approved the final task list (no open items in last Architect handoff)
- All user questions have been answered (or none were raised)

Produce a **Phase Handoff**:

```text
=== PHASE HANDOFF ===
Phase:     Planning
Status:    complete  (or: blocked — <reason>)
Feature:   <feature description>
Branch:    <branch>
Artifacts:
  <WORKTREE>/openspec/changes/<id>/proposal.md
  <WORKTREE>/openspec/changes/<id>/tasks.md
  <WORKTREE>/openspec/changes/<id>/design.md  (if created)
Decisions: <bullet list of key planning decisions>
For next:  <what Development needs: OpenSpec change id, task count,
            any constraints or known risks Development must plan around>
Open:      <user questions still pending, or "none">
=== END HANDOFF ===
```
