# Design Document: Tray Improvements

**Feature:** Privacyclaw Tray Improvements
**Branch:** `feature/tray-improvements`
**Slug:** `tray-improvements`
**Revision:** 4 (final)

---

## Context

The current tray icon (`src/tray.rs`) provides a static menu with:

- Open Dashboard (always enabled)
- HTTP Proxy (CheckMenuItem, always checked/enabled — cosmetic only, non-functional)
- Network Proxy (CheckMenuItem, live-toggled via `network_helper`)
- PII Protection submenu (live-updated via `/api/config` poll every ~5s)
- Quit

Proxy services (HTTP listener, Network listener, Dashboard) always start together on `run_tray_mode` (via `cmd_start`). There is no Start/Stop control from the tray. The tray icon never changes color.

---

## Goals

1. **Status indicator** — icon is green-eye when running, red-eye when stopped.
2. **Start/Stop Proxy** — new top menu item; controls all services. All other items disabled when stopped.
3. **Dashboard-only mode** — the tray process always binds the Dashboard listener. Starting the proxy enables HTTP/Network interception. Stopping stops HTTP and Network listeners (and disables network proxy routing if active), but the Dashboard remains accessible.
4. **HTTP Proxy toggle** — ON starts the HTTP CONNECT listener, OFF stops it. Dashboard stays up.
5. **Network Proxy toggle** — unchanged: ON calls `network_helper::enable`, OFF calls `network_helper::disable`.
6. **PII toggles** — unchanged: runs `privacyclaw config --protection-level <level>`.
7. **Docs** — update `CLI.md`, `README.md`, and any affected openspec specs.

---

## Non-Goals

- No Windows/Linux tray changes.
- No change to the non-tray `start` CLI path.
- No new dashboard UI changes.
- No persistence of proxy on/off state to disk across process restarts.

---

## Key Architecture Decision: Dashboard Lifecycle

**Decision:** The dashboard starts at tray launch and remains running until process exit. It is NOT stopped when the user clicks "Stop Proxy."

**Rationale:** The feature spec says "Proxy can run with no listeners (dashboard only). Starting proxy starts the Dashboard." This implies the dashboard is always present once the tray is running. Stopping/restarting the dashboard dynamically introduces port-rebind races, sidecar lifecycle complexity (`SidecarHandle` is internal to `dashboard::run`), and clean-shutdown coordination. Simpler: dashboard is permanent; Start/Stop only controls the HTTP CONNECT listener and network routing.

**Implication:** "Stop Proxy" = stop HTTP CONNECT listener + disable Network Proxy routing. "Start Proxy" = start HTTP CONNECT listener (if HTTP toggle is on).

---

## User-Facing Behaviour

### Menu layout (stopped state)

```text
[red-eye icon]
─────────────────────────
Start Proxy              ← enabled
─────────────────────────
HTTP Proxy     [ ]       ← disabled (grayed)
Network Proxy  [ ]       ← disabled (grayed)
─────────────────────────
PII Protection ▶         ← disabled (grayed)
─────────────────────────
Open Dashboard           ← ENABLED (dashboard is always running)
─────────────────────────
Quit Privacyclaw         ← always enabled
```

### Menu layout (running state)

```text
[green-eye icon]
─────────────────────────
Stop Proxy               ← enabled
─────────────────────────
HTTP Proxy     [✓]       ← enabled, toggleable
Network Proxy  [✓/off]   ← enabled, toggleable
─────────────────────────
PII Protection ▶         ← enabled
─────────────────────────
Open Dashboard           ← enabled
─────────────────────────
Quit Privacyclaw         ← always enabled
```

### Start Proxy action

1. If `http_listener_on` toggle is true (default: true on each fresh Start), spawn HTTP CONNECT listener task.
2. Icon switches to green-eye.
3. HTTP Proxy, Network Proxy, PII, and Open Dashboard items become enabled.

### Stop Proxy action

1. Abort HTTP CONNECT listener task (if running).
2. If network proxy is enabled (`network_helper::is_enabled()`), call `network_helper::disable()` on a background thread.
3. Icon switches to red-eye.
4. HTTP Proxy, Network Proxy, and PII submenu become disabled.
5. "Open Dashboard" remains enabled (dashboard still running).

### HTTP Proxy toggle (while running only)

- Toggle ON: spawn new HTTP CONNECT listener task.
- Toggle OFF: abort the HTTP CONNECT listener task.
- Dashboard always stays running.

### Network Proxy toggle (while running only)

- Unchanged from current implementation.

### Quit

- Calls Stop Proxy sequence (abort HTTP task, disable network if active).
- Unloads LaunchAgent (existing behavior).
- Calls `shutdown.notify_waiters()` to terminate tokio tasks (dashboard included).

---

## Architecture

### Icon generation

`generate_icon_rgba(size: u32, running: bool)` adds a `running` parameter:

- `running = true` → green centre dot: RGB `(0, 200, 80)`
- `running = false` → red centre dot: RGB `(200, 50, 50)`
- Lens ring and background unchanged.

`make_icon(running: bool)` passes the flag through.

### State held in `tray::run()`

```rust
struct TrayState {
    proxy_running: bool,
    http_listener_on: bool,    // tracks the HTTP toggle checkbox state
    http_task: Option<tokio::task::JoinHandle<()>>,
    cert_cache: CertCache,     // cheaply cloneable (Arc inside)
    cfg: Arc<Config>,
    cfg_mgr: Arc<ConfigManager>,
    store: Store,              // opened once at launch, shared with dashboard
    ws_tx: broadcast::Sender<WsEvent>,
    pii: PiiCtx,
    rt: tokio::runtime::Handle,
}
```

`dashboard_task` is NOT in `TrayState` — it is spawned once at tray launch in `run_tray_mode` and runs for the process lifetime.

### `run_tray_mode` responsibilities

After this change, `run_tray_mode`:

1. Loads config, builds `CertCache`, `PiiCtx`, `ConfigManager` (same as before).
2. Opens `storage::Store`.
3. Creates `broadcast::channel` for WS events.
4. Spawns **dashboard task** (permanent, for process lifetime).
5. Spawns **PII vault eviction task** (permanent).
6. Spawns **log rotation task** (permanent).
7. Writes PID file.
8. Creates tokio `Runtime`, calls `tray::run()` with `TrayState`.
9. On return from `tray::run()`, removes PID file and shuts down runtime.

No longer calls `cmd_start`. The `cmd_start` function is unchanged for the non-tray path.

### `tray::run()` signature change

```rust
pub fn run(
    dashboard_url: String,
    shutdown: Arc<Notify>,
    state: TrayState,
)
```

Replaces the current long parameter list. `TrayState` encapsulates everything needed to spawn/abort tasks.

### Menu IDs

New/changed IDs in `struct Ids`:

- `start_stop` — new; the Start/Stop menu item
- `http_proxy` — now tracked (item previously existed but its ID was not stored)
- `open_dashboard` — existing, now conditionally disabled when stopped
- `network_proxy` — existing, unchanged
- `pii_*` — existing, unchanged
- `quit` — existing, unchanged

### Menu construction

```rust
fn build_menu(
    proxy_running: bool,
    http_proxy_on: bool,
    network_proxy_on: bool,
    pii_level: &str,
) -> (Menu, Ids)
```

When `proxy_running = false`:

- `start_stop` label = "Start Proxy", enabled = true
- `http_proxy` enabled = false
- `network_proxy` enabled = false
- `pii_sub` enabled = false
- `open_dashboard` enabled = true (dashboard always running)
- `quit` enabled = true

When `proxy_running = true`:

- `start_stop` label = "Stop Proxy", enabled = true
- `http_proxy` enabled = true, checked = `http_proxy_on`
- `network_proxy` enabled = true, checked = `network_proxy_on`
- `pii_sub` enabled = true
- `open_dashboard` enabled = true
- `quit` enabled = true

### Config poll interaction

When proxy is stopped, the dashboard is still running. Config polls (`fetch_config_state`) continue to succeed and update `network_proxy_on` and `pii_level`. This is correct behavior.

### HTTP Proxy start/stop implementation

```rust
// Start HTTP listener
let (cfg, cc, st, wt, pi) = (
    state.cfg.clone(), state.cert_cache.clone(),
    state.store.clone(), state.ws_tx.clone(), state.pii.clone(),
);
let handle = state.rt.spawn(async move {
    if let Err(e) = proxy::run(cfg, cc, st, wt, pi).await {
        tracing::error!(err = %e, "proxy error");
    }
});
state.http_task = Some(handle);

// Stop HTTP listener
if let Some(h) = state.http_task.take() {
    h.abort();
}
```

`proxy::run` calls `TcpListener::bind` on entry. Aborting via `JoinHandle::abort()` cancels the task at the `.await` in the accept loop; the socket is dropped and the port is released.

### Abort rationale

`JoinHandle::abort()` sends a cooperative cancellation token. `proxy::run` is a `loop { listener.accept().await? }` — the `.await` is a cancellation point. The task exits promptly and the TCP socket is dropped. For a developer tool with infrequent Start/Stop, no port-rebind race is expected.

---

## Components and Data Model Changes

| File | Change |
| --- | --- |
| `src/tray.rs` | `generate_icon_rgba(size, running)`; `TrayState` struct; `build_menu(proxy_running, http_on, net_on, pii_level)`; new `Ids` fields; updated `tray::run()`; `start_proxy()` / `stop_proxy()` helpers |
| `src/main.rs` | `run_tray_mode`: removes call to `cmd_start`; builds `TrayState` directly; spawns dashboard/rotation/pii-eviction tasks permanently; passes `TrayState` to `tray::run()` |
| `docs/CLI.md` | Document Start/Stop Proxy, HTTP toggle, dashboard-always-running |
| `README.md` | Update tray section: icon states, menu layout, dashboard note |
| `openspec/specs/cli/spec.md` | Add/modify requirements for tray icon status, Start/Stop, HTTP toggle |

No new Cargo dependencies. No database schema changes.

---

## Integration Points — Verified Against Code

| Integration | Finding | Design Response |
| --- | --- | --- |
| `proxy::run` (`src/proxy/mod.rs:16`) | Simple accept loop; `TcpListener::bind` on entry; no internal shutdown channel | Spawn via rt handle; abort via `JoinHandle::abort()` |
| `dashboard::run` (`src/dashboard/mod.rs:101`) | Internal `SidecarHandle`, `download_tracker`, two internal broadcast channels | Dashboard is permanent — no stop/restart needed |
| `ProxyState` (dashboard) | Has `shutdown: Notify` used by dashboard UI to stop the process | Unchanged; still propagates to process-level `Notify` |
| `storage::Store` | `Clone` is cheap (Arc inside) | Opened once in `run_tray_mode`; shared with dashboard and HTTP tasks |
| `CertCache` | `Clone` is cheap (Arc inside) | Built once in `run_tray_mode`; stored in `TrayState` |
| `broadcast::channel` | `Sender::clone()` for fan-out | Created once; `ws_tx` in `TrayState`; dashboard and HTTP tasks each get a clone |
| PII vault eviction task | Spawned in `cmd_start` | Moved to `run_tray_mode`; spawned once for process lifetime |
| Log rotation task | Spawned in `cmd_start` | Moved to `run_tray_mode`; spawned once for process lifetime |
| `pid::write_pid` / `remove_pid` | Currently in `cmd_start` | Moved to `run_tray_mode`: write on entry, remove on exit |
| `network_helper::is_enabled()` | Reads `/etc/hosts` synchronously | Called on Stop Proxy to decide whether to call `disable()` |
| `network_helper::disable()` | Blocking I/O + osascript | Called on background thread (existing pattern) |

---

## Risks and Mitigations

| Risk | Severity | Mitigation |
| --- | --- | --- |
| Port bind race on rapid Start/Stop | Low | Dev tool; user won't cycle faster than 1s. `abort()` drops the socket before the new task binds. |
| Rapid HTTP toggle ON while abort still in-flight | Low | Menu events are processed sequentially on the main thread. At least one 50ms `pump_run_loop` cycle occurs between abort and the next spawn — sufficient for the OS to release the port. |
| Dashboard `ProxyState.shutdown` confusion | Low | Dashboard shutdown signal still propagates to process-level `Notify` as before. |
| `http_task` handle goes invalid if task panics | Low | Task errors are logged; `JoinHandle::abort()` on a finished handle is a no-op. |
| `tray::run()` becomes proxy orchestrator | Medium | Mitigated by `start_proxy()` / `stop_proxy()` helper functions keeping the event loop readable. |
| `generate_icon_rgba` signature change breaks existing tests | Low | Existing tests call `generate_icon_rgba(32)`. Update call sites to `generate_icon_rgba(32, true/false)` and add two new color tests. |
| SQLite write interrupted by task abort | Low | rusqlite drops the connection on abort, rolling back any open transaction cleanly. No corruption risk. |

---

## Open Questions (all resolved)

1. **Initial state on launch** — **Running** (backward compatible; tray auto-calls Start Proxy on entry).
2. **HTTP toggle state after Stop/Start** — Reset to ON on each Start. Simple and predictable.
3. **"No listeners" state** — Dashboard is always running. Start/Stop controls HTTP + network only. Dashboard URL is always accessible.
4. **Network proxy on Stop** — Yes, Stop Proxy calls `network_helper::disable()` if currently enabled. Prevents dangling DNS entries.
5. **Abort vs graceful shutdown** — `abort()` is acceptable for a dev tool; no user data at risk.
6. **Launch mode** — HTTP starts on at launch. Network starts off (user-driven via toggle, as today).

---

## Docs Plan

### `CLI.md` additions

- Section: "Tray Icon States" — green-eye (running), red-eye (stopped).
- Section: "Start/Stop Proxy" — describes what each action does.
- Section: "HTTP Proxy Toggle" — controls CONNECT listener, dashboard unaffected.

### `README.md`

- Update tray screenshot/description.
- Note that dashboard is always accessible even when proxy is stopped.

### openspec delta

- Change-id: `update-tray-proxy-controls`
- Affected spec: `cli`
- Adds requirements for: tray icon status, Start/Stop Proxy, HTTP Proxy toggle, dashboard-always-running.
