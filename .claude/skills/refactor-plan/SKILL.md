---
name: refactor-plan
description: Phase 2 of the refactor workflow. Architect turns the validated smell catalog into an ordered, dependency-aware task list. Contrarian challenges for behavior risk, granularity, sequencing, and boundary safety. PM completes the OpenSpec change (proposal.md + tasks.md + spec deltas) and runs openspec validate --strict. Can be invoked standalone by passing a Refactor-Investigate Phase Handoff as the argument.
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
- `WORKTREE`: the absolute path to the refactor worktree (required;
  if missing, derive as `$(git rev-parse --show-toplevel)/../worktrees/refactor-<slug>`)

---

## Working directory

**All operations in this phase must happen inside `<WORKTREE>`, never in the
main repository working tree.**

Rules that apply to this coordinator and to every agent it invokes:
- File reads/writes: `<WORKTREE>/<relative-path>`
- openspec commands: `cd "<WORKTREE>" && openspec <command>`
- **Every agent message must include `WORKTREE: <worktree_path>`** so agents
  apply the same rule without ambiguity.

---

## Agent coordination protocol

Try team-based coordination first:

```text
TeamCreate({ name: "refactor-plan-team", agents: ["architect", "contrarian", "pm"] })
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

> Working directory: `<WORKTREE>` — read and write files only within this
> directory. Do not access the main repository working tree.
> WORKTREE: `<worktree_path>`
>
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

> Working directory: `<WORKTREE>` — read files only within this directory.
> Do not access the main repository working tree.
> WORKTREE: `<worktree_path>`
>
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

### Step 3 — Complete OpenSpec change via PM

Invoke **pm** with the finalized task list from the Architect/Contrarian cycle. Task:

> Working directory: `<WORKTREE>` — read and write files only within this
> directory. Do not access the main repository working tree.
> WORKTREE: `<worktree_path>`
>
> Using the validated task list, complete the OpenSpec change `refactor-<slug>`:
> 1. Update `<WORKTREE>/openspec/changes/refactor-<slug>/proposal.md` — fill in "What Changes" with task titles and "Impact" with all affected files.
> 2. Create `<WORKTREE>/openspec/changes/refactor-<slug>/tasks.md` — one section per task in OpenSpec checkbox format:
>    ```markdown
>    # Tasks: Refactor <scope>
>    Run `cargo test && cargo clippy -- -D warnings` after every merged task.
>    ---
>    ## T<id>: <title>
>    Smells: <smell IDs>
>    Files: `<file>:<line range>`
>    Depends on: <task IDs or none>
>    Parallel-safe: yes | no — <reason if no>
>    Changes: <what to do>
>    Criterion: <verification test>
>    - [ ] T<id> complete
>    ---
>    ```
> 3. Run `cd "<WORKTREE>" && openspec list --specs` to identify capabilities whose files are touched.
>    For each affected capability, create `<WORKTREE>/openspec/changes/refactor-<slug>/specs/<capability>/spec.md`
>    with a minimal MODIFIED delta:
>    ```markdown
>    ## MODIFIED Requirements
>    ### Requirement: <existing requirement name>
>    [Full requirement text unchanged — structural refactoring only, no behavior change]
>    **Structural notes**: Internal implementation restructured for maintainability.
>    #### Scenario: <existing scenario name>
>    [All scenarios unchanged]
>    ```
> 4. Run `cd "<WORKTREE>" && openspec validate refactor-<slug> --strict` and fix any issues before returning.
> Produce an Agent Handoff with the change id, task count, and validate result.

**Phase completion requires `openspec validate refactor-<slug> --strict` to pass.** If it does not pass, surface the validation errors in the Phase Handoff `Open` field.

**Task status convention** (updated by Refactor-Execute):
- Done: `- [x] T<id> complete`
- Reverted: `- [ ] T<id> complete — ✗ REVERTED: <reason>`
- Blocked: `- [ ] T<id> complete — ⚠ BLOCKED: <reason>`

---

## Team cleanup

```text
SendMessage({ to: "architect",  message: {type: "shutdown_request"} })
SendMessage({ to: "contrarian", message: {type: "shutdown_request"} })
SendMessage({ to: "pm",         message: {type: "shutdown_request"} })
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
OpenSpec:  refactor-<slug>
Artifacts:
  <worktree_path>/openspec/changes/refactor-<slug>/proposal.md
  <worktree_path>/openspec/changes/refactor-<slug>/tasks.md
Decisions:
  - <key task grouping and sequencing decisions>
  - <Contrarian challenges resolved or dismissed>
  - openspec validate: clean
For next:  <what Execute needs: total task count, which tasks are
            parallel-safe, any tasks that are particularly risky or
            boundary-adjacent and need extra Contrarian scrutiny>
Open:      <unresolved critical Contrarian challenges, or "none">
=== END HANDOFF ===
```
