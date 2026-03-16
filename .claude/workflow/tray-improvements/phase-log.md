# Phase Log: Privacyclaw Tray Improvements

## Phase 1 — Design

=== PHASE HANDOFF ===
Phase:     Design
Status:    complete
Feature:   Privacyclaw Tray Improvements — icon status, Start/Stop Proxy,
           dashboard-always-running, HTTP Proxy toggle, docs update
Branch:    feature/tray-improvements
Artifacts: .claude/workflow/tray-improvements/design.md

Decisions:
- Dashboard is permanent (starts at tray launch, never stopped by Stop Proxy).
  Stop Proxy only kills the HTTP CONNECT listener and disables network routing.
- TrayState struct encapsulates all resources needed to spawn/abort tasks;
  passed into tray::run() replacing the current long parameter list.
- HTTP toggle state resets to ON on each Start (predictable, not persisted).
- Tray launches in Running state (backward compatible with current behavior).
- icon uses generate_icon_rgba(size, running: bool); green dot = running,
  red dot = stopped; no new Cargo deps (pure pixel generation).
- JoinHandle::abort() is the stop mechanism for the HTTP listener task;
  acceptable for a dev tool, cancels at the TcpListener::accept() await point.
- run_tray_mode absorbs the duties of cmd_start for the tray path:
  opens Store, spawns dashboard/pii-eviction/rotation tasks, writes PID.
  cmd_start is unchanged for the non-tray CLI path.

For next (Planning):
- The main implementation files are src/tray.rs and src/main.rs (run_tray_mode).
  All other files are docs/spec updates.
- generate_icon_rgba existing tests must be updated (signature adds `running: bool`).
- Two new pixel-level tests needed for running=true and running=false icon colors.
- The menu now needs start_stop and http_proxy IDs in struct Ids (http_proxy was
  built but its ID was never stored — it was cosmetic-only before).
- build_menu gains two new parameters: proxy_running and http_proxy_on.
- openspec change-id to create: update-tray-proxy-controls (affects cli spec).

Open: none
=== END HANDOFF ===

## Phase 2 — Planning

=== PHASE HANDOFF ===
Phase:     Planning
Status:    complete
Feature:   Privacyclaw Tray Improvements — icon status, Start/Stop Proxy,
           dashboard-always-running, HTTP Proxy toggle, docs update
Branch:    feature/tray-improvements
Artifacts:
  openspec/changes/update-tray-proxy-controls/proposal.md
  openspec/changes/update-tray-proxy-controls/tasks.md
  openspec/changes/update-tray-proxy-controls/specs/cli/spec.md

Decisions:
- Change-id is `update-tray-proxy-controls`; affects only the `cli` spec capability.
- tasks.md has 7 groups, 20 tasks total. Groups: (1) icon generation, (2) TrayState struct,
  (3) menu IDs + build_menu, (4) run_tray_mode refactor, (5) tray::run() event loop,
  (6) spec + docs, (7) validation.
- Tasks must be done in order: group 1 and 2 are independent of each other but both must
  precede group 3; groups 3+4 must precede group 5.
- `stop_proxy()` takes only `&mut TrayState` — `network_helper::disable()` needs no args.
- `http_task` must be set to `None` after abort in both stop_proxy and the http_proxy toggle-off path.
- Initial `network_proxy_on` and `pii_level` for the first `build_menu` call are derived
  inside `tray::run()` on entry (not passed in from run_tray_mode).
- The MODIFIED CLI Subcommands requirement carries forward all scenarios from the
  add-macos-packaging delta (to avoid loss at archive time).

For next:
  OpenSpec change-id: `update-tray-proxy-controls`. 20 tasks across 7 groups.
  Primary files: `src/tray.rs` (all tray logic) and `src/main.rs` (run_tray_mode only).
  Key constraint: `generate_icon_rgba` signature changes in task 1.1 break 2 existing tests
  that must be updated in the same task group before cargo test can pass.
  The `cmd_start` function is NOT modified; only `run_tray_mode` changes.

Open: none
=== END HANDOFF ===

## Phase 3 — Development

=== PHASE HANDOFF ===
Phase: Development
Status: complete
Feature: Privacyclaw Tray Improvements — icon status, Start/Stop Proxy, dashboard-always-running, HTTP Proxy toggle, docs update
Branch: feature/tray-improvements
Artifacts:
  .claude/workflow/tray-improvements/impl-log.md
  openspec/changes/update-tray-proxy-controls/tasks.md
  src/tray.rs (primary implementation)
  src/main.rs (run_tray_mode refactor)
  docs/CLI.md (new file)
  README.md (tray section added)

Decisions:
- All 20 tasks implemented directly on feature/tray-improvements without per-task worktree branches.
- TrayState.cfg_mgr held per spec but unused inside tray::run(); suppressed with #[allow(dead_code)].
- docs/CLI.md did not exist — created from scratch.
- README had no prior tray section; inserted before Commands table.
- openspec validate update-tray-proxy-controls --strict exits 0 with no errors.

For next (Testing):
- Build: cargo build --features tray is clean (zero warnings, zero clippy warnings).
- Tests: 391 pass with --features tray. 4 new tray icon tests all pass.
- Focus: Start/Stop Proxy round-trip, HTTP toggle lifecycle, stop_proxy cleanup, dashboard permanence, config poll after stop.
- Known complexity: network_helper::disable() called on background thread from stop_proxy; dismissed dialog logs warning but doesn't propagate.

Open: none
=== END HANDOFF ===
