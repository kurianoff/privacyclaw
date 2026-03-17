---
name: refactor-plan
description: Phase 2 of the refactor workflow. Architect turns the validated smell catalog into an ordered, dependency-aware task list. Contrarian challenges for behavior risk, granularity, sequencing, and boundary safety. Saves the plan as an OpenSpec change (proposal.md + tasks.md). Can be invoked standalone by passing a Refactor-Investigate Phase Handoff as the argument.
argument-hint: "<refactor-investigate phase handoff> [USER_EXCLUSIONS: <additional exclusions>]"
context: fork
---

# Phase 2 — Blueprint

You are the **Phase 2 coordinator**. Your job is to turn the smell catalog
into a concrete, Contrarian-validated refactoring task list that Execute can
run without ambiguity.

Input: **$ARGUMENTS**

Extract from the input:
- `scope`: the code area to refactor (from Refactor-Investigate handoff)
- `boundaries`: do-not-touch zones (from Refactor-Investigate handoff)
- `branch`: the refactor branch (from Refactor-Investigate handoff)
- `smell_catalog`: path to the smell catalog artifact
- `for_next`: context from Refactor-Investigate (smell counts, interdependencies, risks)
- `user_exclusions`: any additional smells the user excluded at the Refactor-Investigate gate

---

## Agent coordination protocol

Try team-based coordination first:

```text
TeamCreate({ name: "refactor-plan-team", agents: ["architect", "contrarian"] })
SendMessage({ to: "architect", message: "<task + context>" })
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

### Step 1 — Architect: refactoring task list

Invoke **architect** with the smell catalog and boundaries. Task:

> Read the smell catalog at `<smell_catalog>`.
> Apply user exclusions: `<user_exclusions>`.
> Produce an ordered, dependency-aware task list. Each task must:
> - Address one or more related smells (group by locality)
> - Be small enough to verify independently (single function, module,
>   or abstraction boundary — touchable in one worktree)
> - Include:
>   - `id`: short identifier (e.g. T1, T2)
>   - `title`: one-line description
>   - `smells`: smell IDs addressed
>   - `files`: files and line ranges touched
>   - `changes`: what to do (not how — leave that to the RE)
>   - `criterion`: exact verification test (e.g. "function X split into Y
>     and Z, each < 30 lines; cargo test green")
>   - `depends_on`: task IDs that must be merged first, or "none"
> - Respect all boundaries — if a smell is near a boundary, document why
>   the specific change is safe
>
> Produce an Agent Handoff with the full task list.

### Step 2 — Contrarian: challenge the task list

Invoke **contrarian** with the Architect handoff and smell catalog. Task:

> Review the task list. Challenge:
> - Tasks that risk behavior change (too aggressive a restructuring)
> - Tasks too large to verify atomically (split them)
> - Sequencing that could leave the codebase non-compilable between tasks
> - Tasks touching a boundary — is the specific change actually safe?
> - Missing or spurious dependency declarations between tasks
> - Missing tasks for smells the Architect overlooked
> - Any two tasks that edit the same file and are marked as independent
>   (they must be sequenced, not parallelized)
>
> Classify each challenge: critical / major / minor.
> Produce an Agent Handoff.

Pass the Contrarian handoff back to **architect**. Task:

> For each challenge: revise the task list to address it, or dismiss with
> a clear rationale. Produce an updated Agent Handoff.

**One round only.** If unresolved critical challenges remain after Architect's
response, surface them to the orchestrator in the `Open` field of the Phase
Handoff — the orchestrator will ask the user for direction.

### Step 3 — Create OpenSpec change

Create `openspec/changes/refactor-<slug>/`.

**`proposal.md`**:

```markdown
# Refactor: <scope>

## Why
Structural smells catalogued by Refactor-Investigate. No behavior change.

## What Changes
<bullet list of task titles>

## Impact
- Affected code: <all files across all tasks>
- Boundaries (do not touch): <boundaries or none>
```

**`tasks.md`** — one section per task, in execution order:

```markdown
# Tasks: Refactor <scope>

Run `cargo test && cargo clippy -- -D warnings` after every merged task.

---

## T<id>: <title>

Smells: <smell IDs>
Files: `<file>:<line range>` (one per line)
Depends on: <task IDs or none>
Parallel-safe: yes | no — <reason if no>

Changes: <what to do>

Criterion: <verification test>

- [ ] T<id> complete

---
```

**Task status convention** (updated by Refactor-Execute):
- Done: `- [x] T<id> complete`
- Reverted: `- [ ] T<id> complete — ✗ REVERTED: <reason>`
- Blocked: `- [ ] T<id> complete — ⚠ BLOCKED: <reason>`

---

## Team cleanup

```text
SendMessage({ to: "architect",  message: {type: "shutdown_request"} })
SendMessage({ to: "contrarian", message: {type: "shutdown_request"} })
TeamDelete()
```

---

## Phase completion

Produce a **Phase Handoff**:

```text
=== PHASE HANDOFF ===
Phase:     Refactor-Plan
Status:    complete  (or: blocked — <reason>)
Scope:     <scope>
Branch:    <branch>
Artifacts: openspec/changes/refactor-<slug>/tasks.md
Decisions:
  - <key task grouping and sequencing decisions>
  - <Contrarian challenges resolved or dismissed>
For next:  <what Execute needs: total task count, which tasks are
            parallel-safe, any tasks that are particularly risky or
            boundary-adjacent and need extra Contrarian scrutiny>
Open:      <unresolved critical Contrarian challenges, or "none">
=== END HANDOFF ===
```
