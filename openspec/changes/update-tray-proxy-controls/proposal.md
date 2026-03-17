# Change: Update Tray Icon with Proxy Controls and Status Indicator

## Why

The current macOS tray icon is purely informational: it shows a static icon with no Start/Stop control and no visual indication of whether the proxy is running. HTTP Proxy is a cosmetic-only checkbox (always checked, never toggled). Users have no way to pause and resume interception without quitting and restarting the process.

## What Changes

- `generate_icon_rgba(size, running: bool)` — adds `running` parameter; green dot when running, red dot when stopped.
- `make_icon(running: bool)` — passes flag through to `generate_icon_rgba`.
- `TrayState` struct — encapsulates all resources (config, cert cache, store, ws_tx, pii, tokio handle) needed to spawn/abort tasks from the tray event loop.
- `build_menu(proxy_running, http_proxy_on, network_proxy_on, pii_level)` — two new parameters; menu items enabled/disabled based on proxy state; new `start_stop` menu item.
- `struct Ids` — gains `start_stop` and `http_proxy` fields.
- `tray::run(dashboard_url, shutdown, state: TrayState)` — replaces long parameter list; drives the new Start/Stop event loop with `start_proxy()` / `stop_proxy()` helpers.
- `run_tray_mode` in `src/main.rs` — absorbs permanent-task setup duties from `cmd_start` for the tray path; no longer calls `cmd_start`; builds `TrayState`; spawns dashboard, PII-eviction, and rotation tasks at launch; writes/removes PID.
- `cmd_start` — unchanged for the non-tray CLI path.
- Dashboard is permanent: starts at tray launch, runs until process exit; unaffected by Stop Proxy.
- Stop Proxy aborts the HTTP CONNECT listener task and disables network routing if active.
- HTTP Proxy toggle (while running): spawns or aborts the HTTP CONNECT listener task.
- Tray launches in Running state (auto-calls Start Proxy on entry — backward compatible).
- `docs/CLI.md` — new sections: Tray Icon States, Start/Stop Proxy, HTTP Proxy Toggle.
- `README.md` — updated tray section.
- OpenSpec delta: `cli` spec updated with new tray requirements.

## Impact

- Affected specs: `cli`
- Affected code: `src/tray.rs` (primary), `src/main.rs` (`run_tray_mode`)
- No new Cargo dependencies
- No database schema changes
- No Windows/Linux changes
- Breaking change to `generate_icon_rgba` signature (internal; existing tests updated in-change)
