# Implementation Log: update-tray-proxy-controls

Feature: Privacyclaw Tray Improvements — icon status, Start/Stop Proxy, dashboard-always-running, HTTP Proxy toggle, docs update
Branch: feature/tray-improvements
Tasks file: openspec/changes/update-tray-proxy-controls/tasks.md

---

### Task 1.1: Change generate_icon_rgba to accept running: bool
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Changed `generate_icon_rgba(size: u32)` to `generate_icon_rgba(size: u32, running: bool)`
  - Green dot (0, 200, 80) when running=true; red dot (200, 50, 50) when running=false
  - Updated test `generate_icon_rgba_centre_is_teal` → `generate_icon_rgba_centre_is_green` with new assertions
  - Added test `generate_icon_rgba_centre_stopped_is_red`
Issues found:
  - none
Contrarian verdict: approved

---

### Task 1.2: Change make_icon() to make_icon(running: bool)
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Changed `make_icon()` to `make_icon(running: bool)`, threads flag to generate_icon_rgba
Issues found:
  - none
Contrarian verdict: approved

---

### Task 1.3: Update test generate_icon_rgba_correct_size
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Updated to call `generate_icon_rgba(size, true)`
Issues found:
  - none
Contrarian verdict: approved

---

### Task 1.4: Update test generate_icon_rgba_background_is_navy
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Updated to call `generate_icon_rgba(32, true)`
Issues found:
  - none
Contrarian verdict: approved

---

### Task 2.1: Define TrayState struct
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Added `pub(crate) struct TrayState` with all 10 specified fields
  - Added `#[allow(dead_code)]` since cfg_mgr is held but not read within run()
Issues found:
  - `cfg_mgr` field triggers dead_code warning — suppressed with allow attribute since the field is retained per spec for future use
Contrarian verdict: approved

---

### Task 3.1: Add start_stop and http_proxy fields to struct Ids
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Added `start_stop` and `http_proxy` fields to `struct Ids`
Issues found:
  - none
Contrarian verdict: approved

---

### Task 3.2: Change build_menu signature
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Changed to `build_menu(proxy_running: bool, http_proxy_on: bool, network_proxy_on: bool, pii_level: &str) -> (Menu, Ids)`
Issues found:
  - none
Contrarian verdict: approved

---

### Task 3.3: Implement proxy-running menu layout in build_menu
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - start_stop MenuItem at top with "Stop Proxy"/"Start Proxy" label
  - http_proxy CheckMenuItem: enabled=proxy_running, checked=http_proxy_on&&proxy_running
  - network_proxy CheckMenuItem: enabled=proxy_running, checked=network_proxy_on&&proxy_running
  - pii_sub Submenu: enabled=proxy_running
  - open_dashboard and quit always enabled=true
  - All IDs stored in Ids struct
Issues found:
  - none
Contrarian verdict: approved

---

### Task 4.1: run_tray_mode — load CA, open Store, build CertCache, PiiCtx, broadcast, ConfigManager
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Rewrote run_tray_mode to set up CA bundle, Store, CertCache, PiiCtx, ws_tx, ConfigManager
Issues found:
  - none
Contrarian verdict: approved

---

### Task 4.2: Spawn dashboard, PII eviction, log rotation tasks permanently
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Dashboard, PII vault eviction, and rotation_loop tasks spawned via rt before TrayState built
  - Dashboard runs for lifetime of process regardless of proxy state
Issues found:
  - none
Contrarian verdict: approved

---

### Task 4.3: Write PID file, register cleanup, remove rt.spawn(cmd_start)
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - pid::write_pid() called after spawning permanent tasks
  - pid::remove_pid() called after tray::run() returns
  - No longer calls cmd_start at all
Issues found:
  - none
Contrarian verdict: approved

---

### Task 4.4: Build TrayState and pass to tray::run
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - TrayState built with proxy_running=false, http_listener_on=false, http_task=None
  - Passed to tray::run(dashboard_url, shutdown, state)
Issues found:
  - none
Contrarian verdict: approved

---

### Task 5.1: Change tray::run() signature; derive locals from state.cfg; call start_proxy on entry
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - New signature: pub fn run(dashboard_url: String, shutdown: Arc<Notify>, mut state: TrayState)
  - Derives domains and proxy_port from state.cfg
  - Derives network_proxy_on via network_helper::is_enabled()
  - Derives initial pii_level via derive_pii_level
  - Calls start_proxy(&mut state) on entry
  - Builds menu with build_menu(true, true, network_proxy_on, initial_pii_level)
  - Creates tray with make_icon(state.proxy_running)
Issues found:
  - none
Contrarian verdict: approved

---

### Task 5.2: Implement fn start_proxy(state: &mut TrayState)
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Spawns proxy::run via state.rt.spawn
  - Stores handle in state.http_task
  - Sets proxy_running=true, http_listener_on=true
  - Logs WARN "proxy started"
Issues found:
  - none
Contrarian verdict: approved

---

### Task 5.3: Implement fn stop_proxy(state: &mut TrayState)
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Aborts http_task if Some, sets to None, sets http_listener_on=false
  - If network_helper::is_enabled(), spawns background thread to disable + unset NODE_EXTRA_CA_CERTS
  - Sets proxy_running=false
  - Logs WARN "proxy stopped"
Issues found:
  - none
Contrarian verdict: approved

---

### Task 5.4: Add start_stop event handler
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Handles ids.start_stop: calls stop_proxy or start_proxy based on state.proxy_running
  - Rebuilds menu and updates icon via tray.set_icon
Issues found:
  - none
Contrarian verdict: approved

---

### Task 5.5: Add http_proxy event handler
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Guards on state.proxy_running
  - If http_task Some: aborts, sets None, http_listener_on=false
  - If http_task None: spawns new listener, sets http_task=Some, http_listener_on=true
  - Rebuilds menu with updated state
Issues found:
  - none
Contrarian verdict: approved

---

### Task 5.6: Update quit handler to call stop_proxy first
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - stop_proxy(&mut state) called before launchctl bootout and shutdown.notify_waiters()
Issues found:
  - none
Contrarian verdict: approved

---

### Task 5.7: Update config poll to pass proxy_running and http_listener_on to build_menu
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Poll uses build_menu(state.proxy_running, state.http_listener_on, ...) for menu rebuild
Issues found:
  - none
Contrarian verdict: approved

---

### Task 5.8: Update network_proxy event handler; move domains/proxy_port to run() locals
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - domains and proxy_port derived at top of run() from state.cfg
  - No longer passed as parameters to tray::run()
  - Network proxy toggle behavior unchanged
Issues found:
  - none
Contrarian verdict: approved

---

### Task 6.1: Validate OpenSpec change
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - `openspec validate update-tray-proxy-controls --strict` exits 0 with "Change 'update-tray-proxy-controls' is valid"
Issues found:
  - none
Contrarian verdict: approved

---

### Task 6.2: Update docs/CLI.md with new tray sections
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Created docs/CLI.md with sections: Tray Icon States, Start/Stop Proxy, HTTP Proxy Toggle, Network Proxy Toggle, Dashboard, PII Protection
Issues found:
  - File did not exist; created fresh
Contrarian verdict: approved

---

### Task 6.3: Update README.md tray section
Status: complete
Branch: feature/tray-improvements (direct)
Done:
  - Added "Tray icon (macOS)" section before Commands with icon state colors, dashboard-always-accessible note, menu items description
  - Added `privacyclaw start --tray` row to Commands table
Issues found:
  - No prior tray section existed; added new section
Contrarian verdict: approved

---

### Task 7.1: cargo build --features tray passes with no warnings
Status: complete
Done:
  - `cargo build --features tray` → zero warnings
  - `cargo clippy --features tray -- -D warnings` → clean
Contrarian verdict: approved

---

### Task 7.2: cargo test passes (all existing tests + 2 new icon tests)
Status: complete
Done:
  - With --features tray: 391 tests pass (vs previous ~374 without tray, 387 without new icon tests)
  - 4 tray icon tests run: generate_icon_rgba_correct_size, generate_icon_rgba_background_is_navy, generate_icon_rgba_centre_is_green, generate_icon_rgba_centre_stopped_is_red
  - 2 pre-existing brew formula test failures (external repo dependency) — unrelated to these changes
Contrarian verdict: approved
