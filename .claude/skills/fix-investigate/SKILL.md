---
name: fix-investigate
description: Phase 1 of the fix workflow. Investigator traces the execution path from symptom to root cause — identifying the exact file:line where behavior diverges from expectation, the mechanism, and the complete fix surface. Contrarian challenges the diagnosis until every alternative cause is ruled out or incorporated. Produces a Root Cause Analysis document. Can be invoked standalone or as part of /fix.
argument-hint: "<symptom description> [BOUNDARIES: <do-not-touch list>] [BRANCH: fix/<slug>]"
context: fork
---

# Phase 1 — Fix-Investigate

You are the **Phase 1 coordinator**. Your job is to produce a precise,
Contrarian-validated Root Cause Analysis that Phase 2 (Planning) can turn
into a minimal, targeted fix without ambiguity.

Input: **$ARGUMENTS**

Extract from the input:
- `symptom`: what the user observed (the bug description)
- `boundaries`: do-not-touch zones. Record as `none` if omitted.
- `BRANCH`: the fix branch (if provided by orchestrator; otherwise record as `tbd`)
- `WORKTREE`: the absolute path to the fix worktree (if provided; otherwise
  derive as `$(git rev-parse --show-toplevel)/../worktrees/fix-<slug>`)

---

## Working directory

**All operations in this phase must happen inside `<WORKTREE>`, never in the
main repository working tree.**

Rules that apply to this coordinator and to every agent it invokes:
- File writes: `<WORKTREE>/<relative-path>`
- Codebase reads: agents read source files from `<WORKTREE>/` — the main repo
  working tree must not be read directly during investigation
- **Every agent message must include `WORKTREE: <worktree_path>`** so agents
  look at the correct copy of the code.

---

## Agent coordination protocol

Try team-based coordination first:

```text
TeamCreate({ name: "fix-investigate-team", agents: ["investigator", "contrarian"] })
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

### Step 1 — Investigator: root cause trace

Invoke **investigator** with the symptom and boundaries. Task:

> Working directory: `<WORKTREE>` — search and read files only within this
> directory. Do not access the main repository working tree.
> WORKTREE: `<worktree_path>`
>
> Your goal is to find the **exact root cause** of this symptom: `<symptom>`.
>
> **Do NOT propose fixes. Do NOT touch any code in `<boundaries>`.**
>
> Work systematically:
>
> 1. **Identify entry points.** From the symptom description, determine which
>    code paths could be involved. Start from the user-visible surface (CLI
>    command, network event, HTTP handler) and trace inward.
>
> 2. **Trace exhaustively.** For each plausible code path, follow the execution
>    step by step — function calls, state mutations, async awaits, error returns.
>    Do not stop at the first suspicious line; keep going to find where the
>    actual divergence from correct behavior occurs.
>
> 3. **Find the divergence point.** The root cause is the specific location
>    where the code does something different from what it should — a wrong
>    assumption, missing check, incorrect state transition, off-by-one, race
>    condition, or incorrect value. Distinguish root cause from downstream
>    symptoms (a panic at line 80 may be caused by a wrong value set at line 40).
>
> 4. **Document the mechanism.** Explain precisely how the divergence point
>    causes the observed symptom. Walk through the causal chain step by step.
>
> 5. **Identify the fix surface.** List every file and approximate line range
>    that must change to correct the root cause. Be conservative — only include
>    what actually needs to change. Do NOT include files that are downstream
>    symptoms or that would only need to change in a broader refactor.
>
> 6. **Note confidence.** If you cannot find a definitive root cause, say so
>    explicitly and explain what you cannot rule out.
>
> Produce an Agent Handoff with:
> - Root cause location: `file:line`
> - Root cause mechanism: one paragraph, precise causal chain
> - Execution path trace: ordered list of `file:function:line` steps from entry
>   to root cause
> - Fix surface: list of files and line ranges that must change
> - Confidence: high (definitive) | medium (most likely) | low (suspected)
> - Alternative explanations ruled out (if any)

### Step 2 — Contrarian: challenge the diagnosis

Invoke **contrarian** with the Investigator handoff. Task:

> Working directory: `<WORKTREE>` — read files only within this directory.
> Do not access the main repository working tree.
> WORKTREE: `<worktree_path>`
>
> Review the root cause diagnosis. Your job is to stress-test the diagnosis —
> not to propose fixes.
>
> Challenge every claim that is not definitively proven:
>
> - **Is this the root cause or a symptom?** Could the actual root cause be
>   upstream of what the Investigator identified? Trace the code path one level
>   further if needed.
>
> - **Are there alternative code paths** that lead to the same symptom but via
>   a different mechanism? If so, are they ruled out, or does the root cause
>   need to account for both?
>
> - **Is the fix surface complete?** Are there other places in the codebase
>   where the same incorrect assumption exists, that would also need to change
>   for the fix to hold? Search for similar patterns.
>
> - **Is the causal chain correct?** Walk through the Investigator's execution
>   trace step by step. Flag any step where the reasoning is inferential rather
>   than proven by reading the actual code.
>
> - **Boundary safety.** Does the fix surface overlap with `<boundaries>`? If
>   so, flag it explicitly — the fix may need a different approach.
>
> - **Confidence calibration.** If the Investigator reported medium or low
>   confidence, push harder: what additional evidence would raise confidence?
>   Can you find it by reading more code?
>
> Produce an Agent Handoff with:
> - Challenged claims (each with the specific code evidence that challenges it)
> - Additional fix surface items not identified by Investigator (if any)
> - Revised confidence level (if lower than Investigator's)
> - Boundary conflicts (if any)
> - Items confirmed correct (explicitly call out what you verified and agree with)

### Step 3 — Investigator: respond to challenges

Pass the Contrarian handoff back to **investigator**. Task:

> Working directory: `<WORKTREE>` — search and read files only within this
> directory. Do not access the main repository working tree.
> WORKTREE: `<worktree_path>`
>
> Respond to each of Contrarian's challenges:
>
> - For each challenged claim: re-read the relevant code and either confirm the
>   original diagnosis with additional evidence, or revise the root cause.
> - For each additional fix surface item Contrarian identified: verify by reading
>   the code. Add to fix surface if confirmed; dismiss with evidence if not.
> - Update confidence level based on the combined investigation.
>
> Produce an updated Agent Handoff with the revised root cause diagnosis.
> Mark which claims were revised and why.

**Repeat Steps 2–3** until Contrarian's handoff contains no unresolved challenges.
Track the round number.

**Maximum 3 rounds.** If Contrarian still has unresolved challenges after 3
rounds, record the disagreement in the RCA and surface it as an `Open` item
in the Phase Handoff for the user to resolve.

---

### Step 4 — Save Root Cause Analysis

Save the validated RCA to `<WORKTREE>/.claude/workflow/<slug>/rca.md`:

```markdown
# Root Cause Analysis: <symptom description>

Date: <date>
Branch: <branch>

## Symptom
<What the user observed — exact description>

## Root Cause
**Location**: `<file>:<line>`
**Confidence**: high | medium | low

<One paragraph: precise causal chain explaining exactly how the code at this
location causes the observed symptom. Every step in the chain must be traceable
to actual code that was read.>

## Execution Path
Trace from entry point to root cause:
1. `<file>:<function>` — <what happens here>
2. `<file>:<function>` — <what happens here>
...
N. `<file>:<line>` — **root cause** — <the divergence from correct behavior>

## Fix Surface
Files that must change to correct the root cause:

| File | Lines | What changes |
|------|-------|--------------|
| `<file>` | `<line range>` | <what needs to be different> |

## Boundaries (do not touch)
<boundaries or none>

## Alternative Explanations Ruled Out
<list of alternative root causes investigated and why they were ruled out,
or "none investigated — confidence is high">

## Contrarian Review
<Challenges raised and how the Investigator responded. If challenges remain
unresolved, document them here for user review.>
Round(s): <N>
Outcome: consensus reached | unresolved — see Open items
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

Phase 1 is complete when:
- Root cause is identified to at least medium confidence
- Contrarian has no unresolved challenges (or remaining disagreements are
  documented and surfaced to the user)
- Fix surface is confirmed complete by both agents
- `rca.md` is saved

Produce a **Phase Handoff** (in Design handoff format, so `plan` can consume it):

```text
=== PHASE HANDOFF ===
Phase:     Fix-Investigate
Status:    complete  (or: blocked — <reason>)
Feature:   fix: <symptom description>
Branch:    <branch or tbd>
Artifacts:
  <worktree_path>/.claude/workflow/<slug>/rca.md
Decisions:
  - Root cause: <file:line> — <one-line mechanism>
  - Confidence: high | medium | low
  - Fix surface: <N> files — <file list>
  - Contrarian rounds: <N> — consensus reached | <unresolved items>
For next:  RCA is at <worktree_path>/.claude/workflow/<slug>/rca.md. Plan must scope tasks to
           the minimal fix surface identified in the RCA — do not expand scope
           beyond what the root cause requires. Fix surface: <file list>.
           Confidence: <level>. <Any specific caution for planning if confidence
           is not high.>
Open:      <unresolved Contrarian challenges for user — or "none">
=== END HANDOFF ===
```
