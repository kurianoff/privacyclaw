# Tasks: update-tray-proxy-controls

## 1. Icon generation (`src/tray.rs`)

- [ ] 1.1 Change `generate_icon_rgba(size: u32)` to `generate_icon_rgba(size: u32, running: bool)`: when `running = true` use green centre dot RGB `(0, 200, 80)`; when `running = false` use red centre dot RGB `(200, 50, 50)`. Remove teal dot.
  **Verify:** existing test `generate_icon_rgba_centre_is_teal` is updated to call `generate_icon_rgba(32, true)` and assert green values; a second new test `generate_icon_rgba_centre_stopped_is_red` calls `generate_icon_rgba(32, false)` and asserts red values.

- [ ] 1.2 Change `make_icon()` to `make_icon(running: bool)` and thread the flag through to `generate_icon_rgba`.
  **Verify:** compiles; existing call site in `tray::run()` updated in task 5.1.

- [ ] 1.3 Update the test `generate_icon_rgba_correct_size` to call `generate_icon_rgba(size, true)`.
  **Verify:** `cargo test generate_icon_rgba` passes (3 tests: size, running-centre, stopped-centre; background test updated to pass `true`).

- [ ] 1.4 Update the test `generate_icon_rgba_background_is_navy` to call `generate_icon_rgba(32, true)`.
  **Verify:** `cargo test` passes with no compile errors.

## 2. TrayState struct (`src/tray.rs`)

- [ ] 2.1 Define `pub(crate) struct TrayState` with fields:
  - `proxy_running: bool`
  - `http_listener_on: bool`
  - `http_task: Option<tokio::task::JoinHandle<()>>`
  - `cert_cache: crate::ca::cert_gen::CertCache`
  - `cfg: Arc<crate::config::Config>`
  - `cfg_mgr: Arc<crate::config::ConfigManager>`
  - `store: crate::storage::Store`
  - `ws_tx: tokio::sync::broadcast::Sender<crate::dashboard::WsEvent>`
  - `pii: crate::pii::PiiCtx`
  - `rt: tokio::runtime::Handle`
  **Verify:** `cargo build` compiles the new struct with no warnings.

## 3. Menu IDs and build_menu (`src/tray.rs`)

- [ ] 3.1 Add `start_stop: tray_icon::menu::MenuId` and `http_proxy: tray_icon::menu::MenuId` fields to `struct Ids`.
  **Verify:** all existing `Ids` construction sites updated; no compile errors.

- [ ] 3.2 Change `build_menu(network_proxy_on: bool, pii_level: &str)` signature to `build_menu(proxy_running: bool, http_proxy_on: bool, network_proxy_on: bool, pii_level: &str) -> (Menu, Ids)`.
  **Verify:** compiles; no behaviour change yet (pass `true` for `proxy_running` and `http_proxy_on` at existing call sites in tasks 5.x).

- [ ] 3.3 Implement proxy-running menu layout in `build_menu`:
  - New `start_stop` `MenuItem` at top: label `"Stop Proxy"` when `proxy_running`, `"Start Proxy"` when not.
  - `http_proxy` `CheckMenuItem`: `enabled = proxy_running`, `checked = http_proxy_on && proxy_running`.
  - `network_proxy` `CheckMenuItem`: `enabled = proxy_running`, `checked = network_proxy_on && proxy_running`.
  - `pii_sub` `Submenu`: `enabled = proxy_running`.
  - `open_dashboard` `MenuItem`: always `enabled = true`.
  - `quit` `MenuItem`: always `enabled = true`.
  - Store `start_stop.id()` and `http_proxy.id()` in `Ids`.
  **Verify:** menu layout matches design doc section "Menu layout (stopped state)" and "Menu layout (running state)".

## 4. run_tray_mode refactor (`src/main.rs`)

- [ ] 4.1 In `run_tray_mode`: load CA bundle; open `storage::Store`; build `CertCache`; build `PiiCtx` via `build_pii_ctx`; create `broadcast::channel`; create `ConfigManager`. This replicates the setup currently performed inside `cmd_start`.
  **Verify:** all values are constructed without error; `cargo build` passes.

- [ ] 4.2 Spawn the **dashboard task** permanently: `rt.spawn(dashboard::run(...))`. Spawn the **PII vault eviction task** permanently (if pii is Some). Spawn the **log rotation task** permanently via `rt.spawn(rotation_loop(store.clone()))`.
  **Verify:** tasks run on background runtime; dashboard accessible at configured URL immediately after tray icon appears.

- [ ] 4.3 Write PID file (`pid::write_pid()`) after spawning permanent tasks. Register cleanup: call `pid::remove_pid()` after `tray::run()` returns. Remove the `rt.spawn(cmd_start(...))` call.
  **Verify:** PID file exists while tray is running; removed after quit.

- [ ] 4.4 Build `TrayState` with `proxy_running: true`, `http_listener_on: true`, `http_task: None` (HTTP task not yet spawned — `tray::run()` will spawn it on entry in task 5.1), and all cloned resources. Pass to `tray::run(dashboard_url, shutdown, state)`.
  **Verify:** `tray::run()` receives all required state; `cargo build` passes.

## 5. tray::run() event loop (`src/tray.rs`)

- [ ] 5.1 Change `tray::run()` signature to `pub fn run(dashboard_url: String, shutdown: Arc<Notify>, mut state: TrayState)`. On entry:
  - Derive local variables from `state.cfg`: `domains: Vec<String>`, `proxy_port: u16`.
  - Derive initial `network_proxy_on: bool` via `crate::network_helper::is_enabled()`.
  - Derive initial `pii_level: String` via `derive_pii_level(&state.cfg.pii.mode, true, false, false)` (tiers unknown until first poll).
  - Call `start_proxy(&mut state)` to spawn the HTTP listener and set `proxy_running = true`.
  - Build initial menu: `build_menu(true, true, network_proxy_on, &pii_level)`.
  - Create tray icon with `make_icon(true)`.
  - Remove old parameters (`network_proxy_on`, `pii_mode`, `domains`, `proxy_port`).
  **Verify:** compiles; tray launches in running state with green icon and "Stop Proxy" menu item.

- [ ] 5.2 Implement `fn start_proxy(state: &mut TrayState)`: spawns HTTP CONNECT listener via `state.rt.spawn(proxy::run(...))`, stores handle in `state.http_task`, sets `state.proxy_running = true`, sets `state.http_listener_on = true`.
  **Verify:** HTTP proxy accepts connections after `start_proxy` is called.

- [ ] 5.3 Implement `fn stop_proxy(state: &mut TrayState)`: aborts `state.http_task` if Some and sets `state.http_task = None`; if `network_helper::is_enabled()`, spawns background thread to call `network_helper::disable()`; sets `state.proxy_running = false`.
  **Note:** `network_helper::disable()` takes no arguments — it reads `/etc/hosts` internally. `domains` and `proxy_port` are not needed here; they are only needed by the `network_proxy` toggle (enable path) in task 5.8.
  **Verify:** HTTP listener stops accepting; network routing disabled if it was active; `state.http_task` is `None` after call.

- [ ] 5.4 Add `start_stop` event handler in the menu event loop: if `state.proxy_running`, call `stop_proxy()`; else call `start_proxy()`. After either action, rebuild menu and update tray icon via `tray.set_icon()` and `tray.set_menu()`.
  **Verify:** clicking Stop Proxy shows red icon and grayed items; clicking Start Proxy shows green icon and enabled items.

- [ ] 5.5 Add `http_proxy` event handler: if `state.proxy_running`:
  - If `http_task` is Some (listener running): abort task, set `state.http_task = None`, set `state.http_listener_on = false`.
  - If `http_task` is None (listener stopped): spawn new HTTP CONNECT listener task (same as `start_proxy` but without changing `proxy_running`), store handle in `state.http_task`, set `state.http_listener_on = true`.
  Rebuild menu with updated state.
  **Verify:** toggling HTTP Proxy off stops the listener (port released); `state.http_task` is `None`; toggling back on restarts it and `state.http_task` is Some.

- [ ] 5.6 Update the `quit` handler: call `stop_proxy()` before `shutdown.notify_waiters()`.
  **Verify:** quit cleans up HTTP listener and network routing before exit.

- [ ] 5.7 Update the config poll section: pass `state.proxy_running` and `state.http_listener_on` to `build_menu(...)` so menu rebuild respects current proxy state.
  **Verify:** after stopping the proxy, a config poll still updates `pii_level` and `network_proxy_on` in state but does not re-enable grayed items.

- [ ] 5.8 Update `network_proxy` event handler: move `domains` and `proxy_port` derivation to top of `run()` from `state.cfg`; pass as closure captures. Remove `domains` and `proxy_port` from `tray::run()` parameters (they are now derived from `state.cfg`).
  **Verify:** network proxy toggle still works identically to current behavior.

## 6. Spec delta and docs

- [ ] 6.1 Validate the OpenSpec change: `openspec validate update-tray-proxy-controls --strict`. Fix any issues.
  **Verify:** command exits with 0 and no errors.

- [ ] 6.2 Update `docs/CLI.md`: add section "Tray Icon States" (green-eye = running, red-eye = stopped), section "Start/Stop Proxy" (what each action does, dashboard unaffected), section "HTTP Proxy Toggle" (controls CONNECT listener only).
  **Verify:** file exists and contains the three new sections.

- [ ] 6.3 Update `README.md` tray section: note icon state colours, mention dashboard always accessible even when proxy is stopped.
  **Verify:** README tray section updated.

## 7. Validation

- [ ] 7.1 `cargo build` with `--features tray` passes with no warnings.
  **Verify:** zero compiler warnings, zero clippy warnings (`cargo clippy --features tray -- -D warnings`).

- [ ] 7.2 `cargo test` passes (all existing tests plus 2 new icon tests from task 1).
  **Verify:** test output shows all passing; count of tray tests = previous count + 2.
