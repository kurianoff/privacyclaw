---
name: modernize
description: Orchestrate the full modernization workflow (Audit → Research → Migrate → Upgrade). Web-researches the latest stable versions of every dependency and toolchain component, plans migrations, implements them with a developer cycle, and optionally upgrades the Rust edition. Standalone skill — not part of the implement flow.
argument-hint: "<scope description>"
---

# Orchestrator — modernize

You are the **modernization orchestrator**. Your job is narrow: set up git,
invoke each phase skill in order, pass compact handoffs between them, maintain
the Phase Log, surface decisions to the user, and gate progression at each
mandatory checkpoint. You do not research, implement, or review anything yourself.

Scope: **$ARGUMENTS** (default: entire codebase if omitted)

---

## Design notes

This orchestrator mirrors the `implement` skill architecture:

- **Context isolation per phase.** Each phase skill runs in a forked subagent
  context. The orchestrator is the only agent that holds the Phase Log.
- **Compact handoffs.** Each phase skill returns a structured Phase Handoff.
  The orchestrator passes only that document to the next phase — not the full
  phase log.
- **Independent phase invocability.** Each sub-skill (`/audit`, `/research`,
  `/migrate`, `/upgrade`) can be invoked standalone.
- **Three mandatory user gates** exist because each subsequent phase is more
  disruptive than the last. The user must explicitly approve at each gate.
- **No auto-merge.** Modernization branches touch foundational dependencies.
  The user reviews and merges manually after all phases complete.

---

## Phase Handoff format

Every phase skill must return a Phase Handoff in this exact format:

```text
=== PHASE HANDOFF ===
Phase:     <Audit | Research | Migrate | Upgrade>
Status:    <complete | blocked — reason>
Scope:     <scope description>
Branch:    modernize/<slug>
Artifacts: <newline-separated list of file paths created or modified>
Decisions: <bullet list of key decisions made in this phase>
For next:  <2–4 sentences: what the next phase needs to know>
Open:      <bullet list of items needing user input, or "none">
=== END HANDOFF ===
```

Store the full Phase Log in `.claude/workflow/modernize-<slug>/phase-log.md`.
Append each handoff as it arrives.

---

## User progress report format

After **every** phase completes, post a progress report before invoking
the next phase:

```
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✅ Phase <N> — <Phase Name> complete
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

**What happened**
<2–4 sentences: what agents ran, what tools they used, what they found.
Be concrete — e.g. "Audit found 2 security advisories (RUSTSEC-2024-0001,
RUSTSEC-2024-0002), 4 Tier-1 patch updates, 3 Tier-3 breaking updates.
Contrarian reclassified tokio from Tier 1 to Tier 2 due to deprecated APIs.">

**Key decisions**
<bullet list, each with one-line rationale>

**Artifacts created / modified**
<bullet list from handoff Artifacts field, one-sentence description each>

**What goes to Phase <N+1>**
<verbatim "For next:" field from the Phase Handoff>

**Open items** (if any)
<list, or omit section>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

## User check-in policy

**After Phase 1 (Audit):** pause and present the tiered catalog to the user.
Show counts per tier, list any security advisories, and the Contrarian
exclusion flags. Ask:

> "Audit complete. Here is the catalog:
> - Security advisories: <count> — <IDs and severity>
> - Tier 1 (patch/minor, no API change): <count deps>
> - Tier 2 (minor with deprecations): <count deps>
> - Tier 3 (major/breaking): <count deps> — <dep names>
> - Tier 4 (edition upgrade available): <yes/no>
> - Contrarian exclusions: <list with reason, or none>
>
> Ready to proceed to Research? Any deps to add to the exclusion list?"

Wait for explicit confirmation. Incorporate any user-added exclusions into
the context passed to Research.

**After Phase 2 (Research):** pause and present the migration plan. Ask:

> "Research complete. Migration plan:
> - Batch A (patch/minor batch): <count deps>
> - Batch B (minor with deprecations): <count tasks>
> - Batch C (major/breaking): <count tasks> — <dep names and complexity>
> - Contrarian exclusions added: <list with reason, or none>
> - Deps requiring group migration: <list, or none>
>
> Approve this plan? Any changes to the exclusion list before I start Migration?"

Wait for explicit confirmation. Incorporate any user amendments into the
handoff context passed to Migrate.

**Before Phase 4 (Upgrade):** always ask explicitly, even if the user said
"proceed" earlier:

> "Migration complete. Edition upgrade to `<target>` is the final phase.
> This is the most disruptive step and requires explicit opt-in.
> Proceed with edition upgrade, or stop here?"

**After Phase 3 (Migrate)** and **Phase 4 (Upgrade):** post the progress
report and proceed automatically (no additional gate).

---

## Step 1 — Git setup

Derive a slug: `YYYY-MM-DD-<scope-slug>` where scope-slug is the scope
description lowercased with spaces replaced by hyphens, truncated to 20 chars
(use `all` if scope is entire codebase).

Example: scope "entire codebase", date 2026-03-16 → slug `2026-03-16-all`

```bash
git checkout main && git pull
git checkout -b modernize/<slug>
mkdir -p .claude/workflow/modernize-<slug>
```

Tell the user: "Branch `modernize/<slug>` created. Starting Phase 1 — Audit."

---

## Step 2 — Invoke Phase 1: Audit

```text
Skill("audit", "<scope>\nBRANCH: modernize/<slug>")
```

Wait for the Phase Handoff. Append to phase log.

**If the handoff `Open` field contains critical security advisories** (CVSS
≥ 7.0 or `cargo audit` severity = error): surface them immediately before
the regular progress report. Ask the user whether to address them as an
emergency fix on main before continuing with the broader modernization.

**Post the Phase 1 progress report.**

**User check-in:** present the tiered catalog and wait for approval.
Incorporate user-added exclusions as an amendment note appended to the
Audit handoff content before passing to Research.

---

## Step 3 — Invoke Phase 2: Research

Tell the user: "Starting Phase 2 — Research."

```text
Skill("research", "<audit handoff content>\nUSER_EXCLUSIONS: <any user-added exclusions>")
```

Wait for the Phase Handoff. Append to phase log.

**Fast path — no breaking changes:** if Research returns
`Status: blocked — only patch/minor updates remain`, skip to Migrate with a
synthetic Research handoff:

```text
=== PHASE HANDOFF ===
Phase:     Research
Status:    complete (fast-path — no Tier 2 or Tier 3 deps after exclusions)
Scope:     <scope>
Branch:    modernize/<slug>
Artifacts: (none — fast path, no migration plan created)
Decisions:
  - No breaking changes found; Batch A (patch/minor) only
For next:  Execute Batch A only. No migration plan file exists.
           Apply all Tier 1 dep bumps from the audit catalog in one batch.
Open:      none
=== END HANDOFF ===
```

**Post the Phase 2 progress report.**

**User check-in:** present the migration plan summary and wait for approval.

---

## Step 4 — Invoke Phase 3: Migrate

Tell the user: "Starting Phase 3 — Migration. This phase updates dependencies
and adapts code to new APIs — it may take a while."

```text
Skill("migrate", "<research handoff content>")
```

Wait for the Phase Handoff. Append to phase log.

**Post the Phase 3 progress report.** Highlight any reverted or blocked deps:

> "These deps were reverted after failing to migrate cleanly: [list].
> These deps are blocked and need user input: [list].
> You may want to address them manually or defer to a future run."

**User check-in for Upgrade:** ask whether to proceed with edition upgrade.

---

## Step 5 — Invoke Phase 4: Upgrade (conditional)

Only if the user confirms:

Tell the user: "Starting Phase 4 — Edition Upgrade."

```text
Skill("upgrade", "<migrate handoff content>")
```

Wait for the Phase Handoff. Append to phase log.

**If the Upgrade handoff returns `Status: blocked`:** surface the reason and
the unresolved Contrarian challenges to the user. Ask whether to merge the
modernize branch as-is (without the edition upgrade) or to defer.

**Post the Phase 4 progress report** (if upgrade ran and completed).

---

## Step 6 — Final report

After all phases complete (or after Phase 3 if upgrade was skipped/declined):

```bash
cargo audit
cargo outdated --depth 1
cargo clippy -- -D warnings
cargo test
```

Report to the user:

```text
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
✅ Modernization complete
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━

Branch: modernize/<slug>
Phase log: .claude/workflow/modernize-<slug>/phase-log.md

Deps updated:  <count>
Deps reverted: <count> — <names, or "none">
Deps blocked:  <count> — <names and reasons, or "none">
Edition upgrade: <completed to <target> | skipped | blocked>

Final checks:
  cargo audit:   <clean | advisories remaining>
  cargo outdated: <clean | pinned/excluded deps remaining>
  cargo clippy:  <clean | warnings>
  cargo test:    <pass count>

To merge:
  git checkout main && git merge --no-ff modernize/<slug>
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
```

---

## Orchestration rules

- Invoke phases strictly in order.
- Pass only the Phase Handoff to the next phase — not the full phase log.
- Never skip Audit or Research (unless Research fast-path applies).
- If a phase skill returns `Status: blocked`, surface `Open` items to the
  user and wait before continuing.
- If a phase skill returns a malformed or missing handoff, ask the user
  whether to re-invoke the phase or proceed manually.
