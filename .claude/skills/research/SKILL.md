---
name: research
description: Phase 2 of the modernize workflow. For every breaking or major dependency update found in Audit, web-searches changelogs and migration guides, maps affected code paths, estimates migration complexity, and produces an ordered update plan. Contrarian challenges exclusions, sequencing, and complexity estimates — including web-searching for known regressions on complex migrations. Can be invoked standalone by passing an Audit Phase Handoff as the argument.
context: fork
argument-hint: <audit phase handoff>
---

# Phase 2 — Research

You are the **Phase 2 coordinator**. Your job is to turn the Audit catalog
into a concrete, risk-assessed, Contrarian-validated migration plan that Migrate
can execute without ambiguity.

Input: **$ARGUMENTS**

Extract from the input:
- `scope`: the modernization scope
- `branch`: the modernize branch
- `audit_catalog`: path to the audit catalog (from Audit handoff `Artifacts`)
- `for_next`: context from Audit (Tier 3 dep names, conflict warnings, complexity flags)
- `user_exclusions`: any additional deps the user excluded at the Audit gate

---

## Agent coordination protocol

Try team-based coordination first:

```text
TeamCreate({ name: "research-team",
             agents: ["general-purpose", "investigator", "architect", "contrarian"] })
SendMessage({ to: "general-purpose", message: "<task + context>" })
```

Fall back to sequential `Agent` tool calls if `TeamCreate` fails. Do not
retry teams more than once.

**Fast path:** if the Audit catalog contains only Tier 1 deps (no Tier 2, Tier 3,
or unused deps after exclusions), skip Steps 1–5 entirely and return a Phase
Handoff with `Status: blocked — only patch/minor updates remain`. The orchestrator
will fast-path directly to Migrate.

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

### Step 1 — General-purpose agent: changelog research

Invoke **general-purpose** (with web search) with the audit catalog path and
user exclusions. Task:

> Read the audit catalog at `<audit_catalog>`.
> For every dependency in **Tier 2** and **Tier 3** (skip Tier 1 and excluded
> deps):
>
> 1. Web search: "<crate-name> <current-version> to <latest-version> migration guide"
> 2. Web search: "<crate-name> changelog <latest-version>"
> 3. If the crate has a GitHub repo, check its CHANGELOG.md or RELEASES.md
>    for the relevant version range
> 4. For each breaking change, record:
>    - Old API (function signature, type, trait method, feature flag)
>    - New API replacement
>    - Whether `cargo fix` handles it automatically (yes / partial / no)
>    - Estimated effort: trivial (rename/re-import) | moderate (logic change) |
>      complex (architectural adaptation, multiple call sites, async changes)
>    - Migration guide URL (if exists)
> 5. Note whether any deps **must be migrated together** as a group (e.g. rustls
>    and tokio-rustls share a major version coupling — bumping one requires the
>    other simultaneously or `cargo` dependency resolution will fail)
> 6. Confidence in migration notes: high (official guide exists) | medium
>    (changelog only) | low (inferred from diff/issues)
>
> Produce an Agent Handoff with migration notes per dep, grouped-migration
> flags, and confidence levels.

### Step 2 — Investigator: verify code impact completeness

Invoke **investigator** with the Step 1 handoff and audit catalog. Task:

> For each Tier 2 and Tier 3 dep, using the specific old API names from the
> migration notes:
> - Search `<scope>` exhaustively for every usage of each old API: function
>   calls, type annotations, trait implementations, `use` imports, feature
>   flags in `Cargo.toml`
> - Cross-check against the Audit catalog's "files affected" list — identify
>   any files the Audit missed
> - Identify **compatibility shims**: wrappers, adapter types, or
>   `#[allow(deprecated)]` blocks that insulate the rest of the code from
>   the dep's API. Note whether migrating the dep requires changing the shim
>   only (low blast radius) or all call sites (high blast radius)
> - For grouped-migration deps (flagged in Step 1): verify that their shared
>   files are correctly identified and that no file touches only one of the
>   group (which would indicate the grouping is incomplete)
>
> Produce an Agent Handoff with a complete, verified per-dep impact map:
> file:line references for every old API usage, shim identification, and
> corrections to the Audit catalog's file lists.

### Step 3 — Architect: migration plan

Invoke **architect** with the Step 1 and Step 2 handoffs and the audit catalog.
Task:

> Using the migration notes (Step 1) and verified impact map (Step 2),
> produce an ordered migration plan. The plan must respect:
>
> **Sequencing rules:**
> - Tier 1 batch always first (no API changes, fastest)
> - Tier 2 before Tier 3 within compatible dependency order
> - Low-level crates before high-level crates that depend on them
>   (e.g. `rustls` before `tokio-rustls` before any code using both)
> - Grouped-migration deps treated as a single atomic task
>
> **Per-task requirements:**
> - Dep name and version bump (current → target)
> - Tier classification and complexity (trivial | moderate | complex)
> - Whether this is a group migration (list all grouped deps)
> - Complete file list with specific old API → new API changes per file
> - Verification criterion: exactly how to confirm completion
>   (e.g. "compiles with no deprecated warnings for this dep; cargo test green")
> - Confidence level (from Step 1 research)
> - Migration guide URL
> - Explicit dependencies on other tasks (must complete X before Y)
>
> Save the draft plan. Produce an Agent Handoff with the task list.

### Step 4 — Investigator: verify plan completeness

Invoke **investigator** with the Architect handoff. Task:

> Review the draft migration plan. For each task:
> - Verify the file list is complete — are there any usages of the old API
>   not listed in the plan's per-file changes?
> - Verify the dependency ordering is correct — if task B touches a file
>   that task A also modifies, is B correctly listed as dependent on A?
> - Identify any task marked "trivial" or "moderate" where the impact map
>   reveals more than 5 distinct call sites — flag for complexity review
>
> Produce an Agent Handoff with: missing files, incorrect dependencies,
> complexity underestimates. Do NOT propose fixes.

### Step 5 — Contrarian: challenge the plan

Invoke **contrarian** with all prior handoffs and the draft migration plan.
Task:

> Challenge the migration plan on every dimension:
>
> **Exclusions — should we update this at all?**
> - Is the "latest stable" version actually stable? Web search:
>   "<dep> <latest-version> regression known issue bug" for every Tier 3 dep
> - Does this update raise MSRV? Web search: "<dep> <version> minimum rust version"
> - Does this update change the license? Web search: "<dep> <version> license"
> - Is there a transitive dep conflict that cargo cannot resolve automatically?
>   (Check the conflict warnings from the Audit handoff)
> - Are there deps that should NOT be updated because they are pinned for a
>   functional reason not visible in comments?
>
> **Sequencing — is the order correct?**
> - Does any task depend on another that is not marked as a dependency?
> - Would the proposed sequence leave the codebase in a non-compilable
>   intermediate state between tasks?
> - Are grouped deps correctly grouped? Are any deps missing from a group?
>
> **Complexity — are estimates realistic?**
> - For every task marked "trivial": is it actually trivial given the file
>   count and call site count from the Investigator?
> - For every task marked "moderate" or "complex": web search
>   "<dep> <version> migration difficulty community experience" to see if
>   others report higher complexity
> - For low-confidence tasks (confidence = low): flag as needing extra
>   Developer iterations in Migrate
>
> **Investigator findings — addressed?**
> - Are all missing files from Step 4 now in the plan?
> - Are all incorrect dependencies corrected?
>
> Produce an Agent Handoff with:
> - Exclusion list (deps to remove from plan, with reason)
> - Revised complexity estimates (with rationale)
> - Sequencing corrections
> - Low-confidence flags requiring extra care in Migrate

### Step 6 — Architect: finalize plan

Pass the Contrarian and Investigator handoffs to **architect**. Task:

> Incorporate all Contrarian and Investigator findings:
> - Remove excluded deps from the plan
> - Revise complexity estimates where challenged
> - Fix sequencing errors
> - Add missing files to affected tasks
> - Mark low-confidence tasks explicitly
> - Re-sequence if needed after removals
>
> Save the final plan to `.claude/workflow/<slug>/migration-plan.md`.
> Produce an Agent Handoff confirming all findings addressed.

Save `.claude/workflow/<slug>/migration-plan.md`:

```markdown
# Migration Plan

Generated: <date>
Scope: <scope>
Branch: <branch>

## Exclusions (do not migrate)
| Dep | Reason |
|-----|--------|

## Unused Deps to Remove (Audit catalog, Contrarian-confirmed)
| Dep | Action: remove from Cargo.toml |
|-----|-------------------------------|

## Batch A — Tier 1: Patch/Minor (no API changes)
Apply all at once. Verify: cargo test + cargo clippy clean.
| Dep | Current | Target | Direct/Transitive |
|-----|---------|--------|-------------------|

## Batch B — Tier 2: Minor with Deprecations
Migrate in dependency order.

### Task B<N>: <dep> <current> → <target>
- Complexity: trivial | moderate | complex
- Confidence: high | medium | low
- Migration guide: <URL or "none">
- Group migration with: <dep list or "none">
- Files and changes:
  - `<file>:<line>` — `<old API>` → `<new API>`
- Verify: <criterion>
- Depends on: <task ID list or "none">

## Batch C — Tier 3: Major/Breaking
Migrate in dependency order.

### Task C<N>: <dep> <current> → <target>
- Complexity: trivial | moderate | complex
- Confidence: high | medium | low
- Migration guide: <URL or "none">
- Group migration with: <dep list or "none">
- Files and changes:
  - `<file>:<line>` — `<old API>` → `<new API>`
- Verify: <criterion>
- Depends on: <task ID list or "none">
- ⚠ Low confidence: <note if applicable>
```

---

## Team cleanup

```text
SendMessage({ to: "general-purpose", message: {type: "shutdown_request"} })
SendMessage({ to: "investigator",    message: {type: "shutdown_request"} })
SendMessage({ to: "architect",       message: {type: "shutdown_request"} })
SendMessage({ to: "contrarian",      message: {type: "shutdown_request"} })
TeamDelete()
```

---

## Phase completion

Phase 2 is complete when:
- `migration-plan.md` exists and covers all non-excluded Tier 2 and Tier 3 deps
- Contrarian's exclusion list and revised estimates are incorporated
- Investigator's file completeness check findings are addressed
- Architect has confirmed all findings resolved

Produce a **Phase Handoff**:

```text
=== PHASE HANDOFF ===
Phase:     Research
Status:    complete  (or: blocked — only patch/minor updates remain)
Scope:     <scope>
Branch:    <branch>
Artifacts:
  .claude/workflow/<slug>/migration-plan.md
Decisions:
  - <exclusions with rationale>
  - <grouped migrations>
  - <complexity revisions from Contrarian>
For next:  <what Migrate needs: total task count, Batch A dep count,
            Batch B task count, Batch C task count, which tasks are complex
            or low-confidence (need extra Developer iterations), grouped
            migration task IDs, migration plan path>
Open:      <user decisions on exclusions or scope, or "none">
=== END HANDOFF ===
```
