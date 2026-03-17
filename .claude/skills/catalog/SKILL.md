---
name: catalog
description: Phase 1 of the refactor workflow. Investigator catalogs structural smells in the target scope (over-long functions, mixed concerns, duplication, under-instrumented paths, dead code). Contrarian challenges the classifications — revising severities, adding missed smells, and flagging boundary risks. Saves the validated catalog to smell-catalog.md. Can be invoked standalone or as part of /refactor.
argument-hint: "<scope> [BOUNDARIES: <do-not-touch list>] [BRANCH: refactor/<slug>]"
context: fork
---

# Phase 1 — Catalog

You are the **Phase 1 coordinator**. Your job is to produce a complete,
Contrarian-validated smell catalog that Blueprint can turn into a task list.

Input: **$ARGUMENTS**

Extract from the input:
- `scope`: the code area to refactor
- `boundaries`: do-not-touch zones. Record as `none` if omitted.
- `BRANCH`: the refactor branch (if provided; otherwise record as `tbd`)

---

## Agent coordination protocol

Try team-based coordination first:

```text
TeamCreate({ name: "catalog-team", agents: ["investigator", "contrarian"] })
SendMessage({ to: "investigator", message: "<task + context>" })
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

### Step 1 — Investigator: smell catalog

Invoke **investigator** with scope and boundaries. Task:

> Read every file in scope: `<scope>`.
> Catalog every structural problem you find:
> - Over-long functions (> ~50 lines of logic)
> - Mixed concerns (a function or module doing more than one thing)
> - Leaky abstractions (internals exposed unnecessarily)
> - Duplicated logic (same pattern in two or more places)
> - Under-instrumented code paths (branches with no tracing calls)
> - `#[allow(deprecated)]` suppressions and structural TODO/FIXME comments
> - Overly complex control flow (deep nesting, long match arms)
>
> For each smell: location (file:line), severity (high/medium/low),
> one-line description, and whether it is adjacent to `<boundaries>`.
>
> Do NOT propose fixes. Do NOT touch any code in `<boundaries>`.
> Produce an Agent Handoff with the full smell catalog, ordered by severity.

**If no smells found:** produce a Phase Handoff with
`Status: complete — no smells found` and stop.

### Step 2 — Contrarian: challenge the catalog

Invoke **contrarian** with the Investigator handoff. Task:

> Review the smell catalog. Challenge:
> - High-severity smells where the refactoring risk outweighs the benefit
> - Smells the Investigator missed
> - Smells adjacent to `<boundaries>` — safe to touch, or must be excluded?
> - Smells that are interdependent (fixing one requires fixing another first)
>
> Produce an Agent Handoff with:
> - Revised severities where challenged (with rationale)
> - Additional smells found
> - Smells to exclude (too risky or boundary-adjacent, with reason)
> - Interdependency notes

### Step 3 — Save catalog

Save the validated catalog to `.claude/workflow/<slug>/smell-catalog.md`:

```markdown
# Smell Catalog

Scope: <scope>
Boundaries: <boundaries or none>
Branch: <branch>

## High Severity
| # | Location | Description | Boundary-adjacent? |
|---|----------|-------------|-------------------|

## Medium Severity
| # | Location | Description | Boundary-adjacent? |
|---|----------|-------------|-------------------|

## Low Severity
| # | Location | Description | Boundary-adjacent? |
|---|----------|-------------|-------------------|

## Excluded (Contrarian)
| Smell | Reason |
|-------|--------|

## Interdependencies
<list of smell pairs that must be addressed together, or "none">
```

---

## Team cleanup

```text
SendMessage({ to: "investigator", message: {type: "shutdown_request"} })
SendMessage({ to: "contrarian",   message: {type: "shutdown_request"} })
TeamDelete()
```

---

## Phase completion

Produce a **Phase Handoff**:

```text
=== PHASE HANDOFF ===
Phase:     Catalog
Status:    complete  (or: complete — no smells found | blocked — <reason>)
Scope:     <scope>
Branch:    <branch or tbd>
Artifacts: .claude/workflow/<slug>/smell-catalog.md
Decisions:
  - <Contrarian reclassifications and exclusions with rationale>
For next:  <what Blueprint needs: smell count per severity, notable
            interdependencies, boundary-adjacent smells that need extra care>
Open:      <user questions, or "none">
=== END HANDOFF ===
```
