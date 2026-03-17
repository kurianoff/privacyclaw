---
name: design
description: Run Phase 1 (Design) of the privacyclaw feature workflow. Coordinates Architect, Investigator, and Contrarian to produce a stable Design Document. Can be invoked standalone or as part of /implement.
argument-hint: <feature description> [BRANCH: feature/<slug>]
context: fork
---

# Phase 1 — Design

You are the **Phase 1 coordinator**. Your job is to produce a stable, fully
challenged Design Document that the Planning phase can build a task list from.

Input: **$ARGUMENTS**

Extract from the input:
- `feature`: the feature description
- `BRANCH`: the feature branch name (if provided; otherwise record as `tbd`)

---

## Agent coordination protocol

For each agent invocation in this phase, try team-based coordination first:

```text
TeamCreate({ name: "design-team", agents: ["architect", "investigator", "contrarian"] })
SendMessage({ to: "architect", message: "<task + context>" })
```

Use `SendMessage` to pass the Agent Handoff document between agents within the
team. If `TeamCreate` fails or is unavailable, fall back immediately to
sequential `Agent` tool invocations — do not retry teams more than once.

---

## Agent Handoff format

Every agent in this phase produces an Agent Handoff before exiting. Pass this
handoff — not raw output — to the next agent.

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

### Step 1 — Architect: initial Design Document

Invoke **architect** with the feature description. Task:

> Create a Design Document for: `<feature>`. The document must cover:
> purpose and user-facing behaviour; internal architecture; components and
> data model changes; integration points with existing code; open questions
> that need resolution before implementation.
>
> Save the document to `.claude/workflow/<slug>/design.md`.
> Produce an Agent Handoff.

### Step 2 — Investigator: codebase fit analysis

Invoke **investigator** with the Architect's handoff and the design doc path.
Task:

> Read the Design Document at `<path>`. Trace every integration point named
> in it through the existing codebase. Identify: code paths that must change,
> conflicts with current architecture, risks the design does not address,
> and any assumptions the Architect made that the code contradicts.
>
> Do NOT propose fixes. Produce an Agent Handoff with your findings.

### Step 3 — Architect: incorporate findings

Pass the Investigator handoff back to **architect**. Task:

> Read the Investigator's findings. Update the Design Document to address
> every finding. Where a finding reveals a genuine design flaw, revise the
> design. Where a finding is a non-issue, document why.
> Produce an Agent Handoff describing what changed.

### Step 4 — Contrarian: challenge the design

Invoke **contrarian** with the updated design doc and the Architect + Investigator
handoffs. Task:

> Read the Design Document and the prior agent handoffs. Your job is to find
> what is wrong, under-specified, or risky. Challenge every assumption.
> Identify unaddressed failure modes. Flag design decisions that appear
> convenient but fragile.
>
> Produce an Agent Handoff listing each challenge with severity (critical /
> major / minor). Do not approve prematurely.

### Step 5 — Architect: respond to challenges

Pass the Contrarian handoff to **architect**. Task:

> For each challenge in the Contrarian's handoff: either revise the Design
> Document to address it, or explicitly dismiss it with a clear rationale.
> Produce an Agent Handoff documenting each response.

### Step 6 — Loop until resolved

Repeat Steps 4–5 until the Contrarian's handoff contains **no open critical or
major items**. Minor items may remain if Architect has documented a rationale.

**Maximum iterations:** 4. If the loop has not converged after 4 rounds,
surface the remaining open items to the user and wait for a decision before
continuing.

### Step 7 — User escalation

At any point during Steps 1–6, if Architect or Contrarian raises a question
that requires a product or scope decision:

1. Collect all pending user questions into a single list.
2. Pause the loop.
3. Return a Phase Handoff with `Status: blocked` and the questions in `Open`.
   The orchestrator will surface these to the user.

---

## Team cleanup

If `TeamCreate` succeeded earlier, shut down all agents and delete the team
**before** producing the Phase Handoff:

```text
SendMessage({ to: "architect",    message: {type: "shutdown_request"} })
SendMessage({ to: "investigator", message: {type: "shutdown_request"} })
SendMessage({ to: "contrarian",   message: {type: "shutdown_request"} })
TeamDelete()
```

If `TeamCreate` was never called (sequential fallback path), skip this section.

---

## Phase completion

Phase 1 is complete when:
- Design Document exists at `.claude/workflow/<slug>/design.md`
- Contrarian's last handoff has no open critical or major items
- All user questions have been answered (or none were raised)

Produce a **Phase Handoff** in the format required by the orchestrator:

```text
=== PHASE HANDOFF ===
Phase:     Design
Status:    complete  (or: blocked — <reason>)
Feature:   <feature description>
Branch:    <branch or tbd>
Artifacts: .claude/workflow/<slug>/design.md
Decisions: <bullet list of key design decisions>
For next:  <what Planning needs to know: key constraints, open questions
            resolved, anything that will shape the task list>
Open:      <user questions still pending, or "none">
=== END HANDOFF ===
```
