---
name: audit
description: Phase 1 of the modernize workflow. Uses CLI tools and web search to build a complete picture of the codebase's currency — security advisories, outdated dependencies, unused deps, toolchain version, Rust edition. Contrarian challenges tier classifications before the catalog is finalized. Can be invoked standalone or as part of /modernize.
context: fork
argument-hint: "<scope description> [BRANCH: modernize/<slug>]"
---

# Phase 1 — Audit

You are the **Phase 1 coordinator**. Your job is to produce a complete,
accurate, and Contrarian-validated picture of how current the codebase is —
from the Rust toolchain down to every dependency — and to classify each finding
by update risk so that Research knows where to focus.

Input: **$ARGUMENTS**

Extract from the input:
- `scope`: the code area to modernize (or "entire codebase" if omitted)
- `BRANCH`: the modernize branch (if provided; otherwise record as `tbd`)
- `user_exclusions`: any deps the user pre-excluded (from orchestrator context)

---

## Agent coordination protocol

Try team-based coordination first:

```text
TeamCreate({ name: "audit-team",
             agents: ["general-purpose", "investigator", "contrarian"] })
SendMessage({ to: "general-purpose", message: "<task + context>" })
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

### Step 1 — General-purpose agent: CLI audit

Invoke **general-purpose** (with web search and CLI access). Task:

> Install any missing tools, then run all of the following and record output:
>
> ```bash
> # Toolchain currency
> rustup show
> rustup check
> rustc --version
> cargo --version
>
> # Dependency currency
> cargo install cargo-outdated 2>/dev/null || true
> cargo outdated --depth 1 2>&1   # direct deps vs latest on crates.io
>
> # Security
> cargo install cargo-audit 2>/dev/null || true
> cargo audit 2>&1
>
> # Unused dependencies
> cargo install cargo-machete 2>/dev/null || true
> cargo machete 2>&1
>
> # Full dependency tree (direct vs transitive)
> cargo tree --depth 1 2>&1
> ```
>
> Also read `Cargo.toml` and record:
> - The current `edition` field
> - Any `[patch]` sections (these are intentional overrides — do not classify
>   overridden crates as candidates for standard version bumps)
> - Any `# pinned` or `# do not update` comments adjacent to dep entries
>   (these are explicit exclusions)
>
> Produce an Agent Handoff with:
> - Active Rust toolchain version and latest stable (from `rustup check`)
> - Current Rust edition
> - Full `cargo outdated` output (raw)
> - Full `cargo audit` output (raw)
> - Full `cargo machete` output (raw — unused dep candidates)
> - List of `[patch]` overrides and pinned/commented deps (pre-exclusions)

### Step 2 — General-purpose agent: web research per dependency

Invoke **general-purpose** again with the cargo outdated output. Task:

> For every dependency listed as outdated in the `cargo outdated` output
> (skip any in the pre-exclusion list from Step 1):
>
> 1. Web search: "<crate-name> crates.io latest version" to confirm the
>    latest stable version (not a pre-release)
> 2. Determine the gap type:
>    - Patch: x.y.Z → x.y.Z' (same major and minor)
>    - Minor: x.Y.z → x.Y'.z' (same major, higher minor)
>    - Major: X.y.z → X'.y'.z' (higher major)
> 3. For major version gaps only: web search
>    "<crate-name> <current-major> to <target-major> breaking changes migration"
>    to determine whether breaking API changes exist
> 4. Record the release date of the latest stable version
>
> Also web search the latest stable Rust toolchain version if `rustup check`
> shows an update is available: "rust stable release <year>"
>
> Produce an Agent Handoff with a raw dependency table:
>
> | Dep | Current | Latest | Gap type | Direct/Transitive | Breaking changes? | Notes |
> |-----|---------|--------|----------|-------------------|-------------------|-------|

### Step 3 — Investigator: code impact mapping

Invoke **investigator** with the Step 2 handoff and the `cargo machete` output.
Task:

> For every dependency flagged as having breaking changes (major version gap):
> - Find every file in `<scope>` that directly imports or uses this dependency
> - Identify the specific APIs in use: function calls, type names, trait impls,
>   feature flags in Cargo.toml
> - Look for `#[allow(deprecated)]` suppressions anywhere in scope — list each
>   with the dep it relates to (if determinable) and its file:line
> - Look for `// TODO: update`, `// FIXME: upgrade`, or similar comments
>   related to dependencies — these are signals of known tech debt
>
> For every dep flagged by `cargo machete` as unused:
> - Verify the finding: search for any use of the dep's crate name in `<scope>`
> - Confirm whether it is truly unused or used only in tests/build scripts
>
> Do NOT propose fixes. Produce an Agent Handoff mapping each dep to:
> - Files and API surface affected (for breaking deps)
> - Confirmed unused status (for machete findings)
> - `#[allow(deprecated)]` and debt comment locations

### Step 4 — Contrarian: challenge tier classifications

Invoke **contrarian** with all three prior handoffs. Task:

> Review the raw dependency table and investigator findings.
> Challenge every tier classification that may be wrong:
>
> - Deps classified as Tier 1 (patch/minor, no API change) where the
>   changelog or release notes suggest subtle behavior changes — bump to Tier 2
> - Deps classified as Tier 2 (minor with deprecations) where the deprecated
>   APIs are load-bearing and removal is non-trivial — bump to Tier 3
> - Deps classified as Tier 3 (major/breaking) where the migration is actually
>   trivial (e.g. a rename only) — bump down to Tier 2
> - Any dep the Investigator shows is used only via a thin wrapper — is the
>   wrapper insulating us from the breaking change?
> - Any dep where the "latest stable" version is actually a recent major release
>   with known regressions (web search: "<dep> <latest-version> regression bug")
> - `cargo machete` unused dep findings — are any of these actually used
>   indirectly (proc macros, re-exports)? Challenge any that seem risky to remove.
>
> Also verify: do any proposed updates conflict with each other at the
> `cargo` dependency resolution level? (e.g., dep A@2.0 requires dep B@1.x
> but we also want dep B@2.0)
>
> Produce an Agent Handoff with:
> - Revised tier for each reclassified dep (with rationale)
> - Confirmed or retracted unused dep findings
> - Dep conflict warnings
> - Any dep that should be excluded entirely from modernization (with reason)

### Step 5 — Build and save catalog

Using all four handoffs, build the tiered update catalog.
Save it to `.claude/workflow/<slug>/audit-catalog.md`:

```markdown
# Modernization Audit Catalog

Generated: <date>
Scope: <scope>
Branch: <branch>

## Toolchain
- Rust toolchain: <current> → <latest> (status: current | update available)
- Rust edition: <current> → <latest available> (status: current | upgrade available)

## Security Advisories
| Dep | Advisory ID | CVSS | Severity | Description |
|-----|-------------|------|----------|-------------|
(or: none found)

## Unused Dependencies (cargo machete, Contrarian-confirmed)
| Dep | Confirmed unused? | Safe to remove? |
|-----|-------------------|-----------------|

## Tier 1 — Patch/Minor (no API changes)
| Dep | Current | Latest | Direct/Transitive |
|-----|---------|--------|-------------------|

## Tier 2 — Minor with Deprecations
| Dep | Current | Latest | Deprecated APIs in use | Files affected |
|-----|---------|--------|------------------------|----------------|

## Tier 3 — Major/Breaking
| Dep | Current | Latest | Breaking API surface | Files affected | Complexity estimate |
|-----|---------|--------|----------------------|----------------|---------------------|

## Tier 4 — Edition Upgrade
- Current edition: <edition>
- Latest stable edition: <edition>
- Upgrade available: yes | no

## Excluded / Pre-Pinned (do not modernize)
| Dep | Reason |
|-----|--------|
```

---

## Security advisory severity thresholds

When surfacing security advisories in the Phase Handoff `Open` field:
- **Critical** (surface immediately, halt): CVSS ≥ 7.0 OR `cargo audit`
  reports severity = `error`
- **Warning** (surface in catalog, proceed): CVSS < 7.0 AND severity =
  `warning`

---

## Team cleanup

```text
SendMessage({ to: "general-purpose", message: {type: "shutdown_request"} })
SendMessage({ to: "investigator",    message: {type: "shutdown_request"} })
SendMessage({ to: "contrarian",      message: {type: "shutdown_request"} })
TeamDelete()
```

---

## Phase completion

Phase 1 is complete when:
- `audit-catalog.md` covers all four tiers
- All direct dependencies are classified
- Contrarian has reviewed and finalized tier assignments
- Security advisories are identified (or confirmed absent)

Produce a **Phase Handoff**:

```text
=== PHASE HANDOFF ===
Phase:     Audit
Status:    complete  (or: blocked — <reason>)
Scope:     <scope>
Branch:    <branch or tbd>
Artifacts: .claude/workflow/<slug>/audit-catalog.md
Decisions:
  - <Contrarian reclassifications with rationale>
  - <Confirmed unused deps>
For next:  <what Research needs: count per tier, names of Tier 3 deps,
            any conflict warnings, complexity flags from Investigator>
Open:
  - <critical security advisories: dep, advisory ID, CVSS — or "none">
=== END HANDOFF ===
```
