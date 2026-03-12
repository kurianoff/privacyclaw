# Tasks: add-macos-packaging

## 0. Version Labeling

- [x] 0.1 Create `claudovka/build.rs`: run `git rev-parse --short HEAD` and capture ISO-8601 date; emit `cargo:rustc-env=GIT_HASH=...` and `cargo:rustc-env=BUILD_DATE=...`
- [x] 0.2 Create `claudovka/src/version.rs`: expose `VERSION`, `GIT_HASH`, `BUILD_DATE` constants and `version_string()` helper returning `"<ver> (<hash> <date>)"`
- [x] 0.3 Add `mod version;` to `main.rs`; update `#[command(...)]` to use `version_string()` as the clap version string; emit startup WARN log with `version` and `git_hash` fields
- [x] 0.4 Add `GET /api/version` endpoint to `dashboard/mod.rs` returning `{ version, git_hash, build_date }`
- [x] 0.5 Fetch `/api/version` on dashboard load and display `claudovka v<X.Y.Z>` in the header
- [x] 0.6 Add `VERSION := $(shell cargo metadata --no-deps --format-version 1 | jq -r '.packages[0].version')` to `Makefile`; use in pkg/dmg filenames
- [x] 0.7 Unit test: `version_string()` contains the semver, a 7-char hex hash, and a date in YYYY-MM-DD format

## 1. Port Standardisation

- [x] 1.1 Update default ports in `config.rs`: HTTP proxy → 16440, network proxy → 16441, llama-cpp → 16442, dashboard → 16443
- [x] 1.2 Update `PiiSlmConfig::default()` endpoint to `http://127.0.0.1:16442`
- [x] 1.3 Update all references in `README`, `USAGE_GUIDE.md`, and NOTE files
- [x] 1.4 Verify `cargo build` and `cargo test` pass with new defaults

## 2. Config Hot-Reload Infrastructure

- [x] 2.1 Wrap `Config` in `Arc<RwLock<Config>>` and thread it through `main.rs`, proxy modules, dashboard, and PII pipeline
- [x] 2.2 Implement `ConfigManager`: `get()`, `patch(partial_json) -> Result<PatchResult>`, `save_to_disk()` methods
- [x] 2.3 Add `PatchResult { ok: bool, restart_required: bool }` type
- [x] 2.4 Enforce PII tier dependency rules in `ConfigManager::patch()`: Tier 2 requires Tier 1; Tier 3 requires Tier 1 + Tier 2
- [x] 2.5 Unit tests for `ConfigManager::patch()` covering dependency validation and port-change detection

## 3. Configuration REST API

- [x] 3.1 Add `GET /api/config` endpoint returning sanitised config JSON (no internal paths, no secrets)
- [x] 3.2 Add `PATCH /api/config` endpoint: parse partial JSON, delegate to `ConfigManager::patch()`, persist, return `PatchResult`
- [x] 3.3 Broadcast `config_changed` WebSocket event after successful patch (include changed keys in payload)
- [x] 3.4 Integration test: PATCH pii.tiers.ner = true, verify response and persisted config

## 4. Dashboard Configuration UI

- [x] 4.1 Add a Settings panel to `index.html` (gear icon in header → slide-in panel)
- [x] 4.2 Implement proxy toggles: HTTP Proxy on/off, Network Proxy on/off; wire to `PATCH /api/config`
- [x] 4.3 Implement PII mode selector (Off / Detect-only / Replace) and tier toggles with dependency lock-out
- [x] 4.4 Auto-enable parent tiers when enabling a dependent tier (single PATCH call)
- [x] 4.5 Show restart-required banner when API responds with `restart_required: true`
- [x] 4.6 Populate settings panel values on open via `GET /api/config`

## 5. Model Management

- [x] 5.1 Define `ModelCatalog` static array in Rust with the four GGUF models (id, name, url, sha256, size, ram)
- [x] 5.2 Implement `GET /api/models` endpoint: merge catalog metadata with on-disk state (downloaded, active, progress)
- [x] 5.3 Implement `POST /api/models/:id/download`: start background download, stream `model_download_progress` WS events
- [x] 5.4 Verify SHA-256 checksum after download; delete partial file on mismatch and send `model_download_error` WS event
- [x] 5.5 Implement `DELETE /api/models/:id/download` (cancel in-progress download)
- [x] 5.6 Implement `POST /api/models/:id/activate`: stop running sidecar, start new sidecar on port 16442, update config
- [x] 5.7 Implement `POST /api/models/deactivate`: stop sidecar, disable Tier 3
- [x] 5.8 Implement `DELETE /api/models/:id`: reject if active, delete file, update state
- [x] 5.9 Add model management table to the Settings panel in the dashboard UI with live WS progress updates
- [x] 5.10 Unit tests for catalog endpoint; integration test for download progress event sequence

## 6. CLI Network Privilege Helper

- [x] 6.1 Implement `/etc/hosts` snapshot: before any change, write original content to `~/.config/claudovka/backup/hosts.bak`
- [x] 6.2 Implement `/etc/hosts` writer: append one `127.0.0.1 <domain> # claudovka` line per intercept domain; skip if already present
- [x] 6.3 Implement `/etc/hosts` reverter: remove all lines containing `# claudovka`; verify no claudovka entries remain
- [x] 6.4 Implement `claudovka network-enable`: snapshot hosts + pf.conf, write hosts entries, write `/etc/pf.anchors/claudovka`, patch `/etc/pf.conf`, run `pfctl`, install LaunchDaemon — all via a single `osascript` admin dialog
- [x] 6.5 Implement `claudovka network-disable`: remove `# claudovka` hosts lines, flush pf anchor, remove pf.conf include, unload + remove LaunchDaemon
- [x] 6.6 Write the LaunchDaemon plist template (`com.claudovka.pf.plist`) that re-applies the anchor at boot
- [x] 6.7 Handle idempotency: re-running `network-enable` when already active verifies and refreshes entries without re-prompting
- [x] 6.8 Handle domain list changes: re-running `network-enable` after adding/removing a domain from config updates `/etc/hosts` accordingly
- [x] 6.9 Wire dashboard Network Proxy toggle to call `network-enable`/`network-disable` via subprocess
- [ ] 6.10 Manual test: enable/disable network proxy; verify `/etc/hosts` entries, pf rules, and LaunchDaemon lifecycle; confirm LLM API traffic routes through the proxy

### 6. Tests (Network Privilege Helper)

- [x] 6.T1 Unit test: `build_hosts_entries(domains)` produces one `127.0.0.1 <domain> # claudovka` line per domain, no duplicates
- [x] 6.T2 Unit test: `remove_claudovka_lines(content)` removes all `# claudovka` lines and leaves all other lines intact
- [x] 6.T3 Unit test: `has_claudovka_entries(content)` returns true iff at least one `# claudovka` line is present
- [x] 6.T4 Unit test: idempotency — calling `build_hosts_entries` on already-modified content does not add duplicate entries
- [x] 6.T5 Unit test: domain list change — adding a domain produces a new entry; removing one drops the old entry
- [x] 6.T6 Unit test: `build_pf_anchor(port)` produces the correct `rdr pass on lo0` rule string for the given port

## 6b. Full Uninstaller

- [x] 6b.1 Implement `claudovka uninstall` orchestrator: ordered step list with per-step result (done / skipped / failed)
- [x] 6b.2 Step: gracefully stop running proxy (SIGTERM + wait up to 5s, then SIGKILL)
- [x] 6b.3 Step: unload and remove LaunchAgent (`~/Library/LaunchAgents/com.claudovka.proxy.plist`)
- [x] 6b.4 Step: if network proxy was ever enabled — revert `/etc/hosts` (remove `# claudovka` lines) via osascript admin dialog
- [x] 6b.5 Step: flush pf anchor, remove include from `/etc/pf.conf`, unload + remove pf LaunchDaemon via osascript
- [x] 6b.6 Step: remove CA from System keychain (`security remove-trusted-cert` + `security delete-certificate`) via osascript
- [x] 6b.7 Step: delete `/usr/local/bin/claudovka` and `/Applications/Claudovka.app` (if present)
- [x] 6b.8 Step: delete `~/.config/claudovka/bin/llama-server`
- [x] 6b.9 Step (`--purge` only): `rm -rf ~/.config/claudovka/` — models, logs, DB, config, CA, backups
- [x] 6b.10 Print final summary table with ✓ / ⚠ / ✗ per step; exit non-zero if any step failed
- [x] 6b.11 Unit tests: mock each privileged step; verify ordering and that `--purge` flag gates data deletion
- [ ] 6b.12 Manual test: full install → enable network proxy → `uninstall` → confirm system is clean; repeat with `--purge`

### 6b. Tests (Uninstaller)

- [x] 6b.T1 Unit test: `UninstallRunner::run()` executes steps in the correct order (stop → LaunchAgent → network → CA → binary → app → llama-server)
- [x] 6b.T2 Unit test: `--purge` flag adds the data-deletion step; without `--purge` the step is absent
- [x] 6b.T3 Unit test: a step that finds its target already absent returns `Outcome::Skipped`, not `Outcome::Failed`
- [x] 6b.T4 Unit test: a step that encounters an OS error returns `Outcome::Failed(msg)` and does not abort subsequent steps
- [x] 6b.T5 Unit test: `stop_proxy` with no PID file returns `Outcome::Skipped`
- [x] 6b.T6 Unit test: `remove_launch_agent` when plist file is absent returns `Outcome::Skipped`
- [x] 6b.T7 Unit test: network-was-never-enabled path skips pf/hosts steps and does not attempt osascript
- [x] 6b.T8 Unit test: summary print includes one line per step with ✓ / ⚠ / ✗ symbol

## 7. Menu Bar App

- [x] 7.1 Add `tray-icon` to `Cargo.toml` (optional dep, `tray` feature; `muda` is re-exported by tray-icon)
- [x] 7.2 Implement tray event loop on main thread; move tokio runtime start to background thread (`run_tray_mode` in main.rs)
- [x] 7.3 Build menu: Open Dashboard, HTTP Proxy toggle (checked), Network Proxy toggle (checked), PII submenu, separator, Quit (`src/tray.rs`)
- [x] 7.4 Reflect live proxy state in menu (check marks update when config changes via WS `config_changed` event)  <!-- implemented via polling GET /api/config every ~5s -->
- [x] 7.5 Network Proxy menu toggle calls `network-enable`/`network-disable` and shows system auth dialog (via subprocess)
- [x] 7.6 Add `--tray` flag to `claudovka start` that enters menu bar mode
- [x] 7.7 Add app icon (512×512 PNG + ICNS) to `assets/` (procedural icon: navy background + white ring + teal dot)

### 7. Tests (Menu Bar App)

- [ ] 7.T1 Manual test: launch `claudovka start --tray`; confirm menu bar icon appears and no Dock icon is shown
- [ ] 7.T2 Manual test: toggle HTTP Proxy off from the menu; confirm the proxy stops accepting connections; toggle on, confirm it resumes
- [ ] 7.T3 Manual test: select Network Proxy toggle when disabled; confirm native macOS admin dialog appears; confirm pf rules are applied on success
- [ ] 7.T4 Manual test: switch PII mode from Off → Tier 1 via the menu submenu; confirm `GET /api/config` reflects the new mode
- [ ] 7.T5 Manual test: click "Open Dashboard"; confirm the system browser opens `http://localhost:16443`
- [x] 7.T6 Unit test: `build_pii_menu_items(config)` returns correct checked/unchecked states for each tier

## 8. macOS App Bundle

- [x] 8.1 Create `Claudovka.app` bundle skeleton: `Contents/{MacOS,Resources,Info.plist}` (`make app` target)
- [x] 8.2 Write `Info.plist` with `LSUIElement = true`, bundle identifier `com.claudovka.app`, minimum macOS 13
- [x] 8.3 Write launch script (`Contents/MacOS/claudovka-app`) that calls `claudovka start --tray`
- [x] 8.4 Add `make app` Makefile target that builds the binary (arm64 + x86_64 universal with `--features tray`) and assembles the bundle
- [ ] 8.5 Validate: open app on macOS, confirm no Dock icon, confirm menu bar icon and proxy start

### 8. Tests (App Bundle)

- [x] 8.T1 Unit test: `extract_llama_server(bundle_path, dest_path)` copies the binary and sets executable bit; skips if already present
- [ ] 8.T2 Manual test: build `make app`; open `Claudovka.app`; confirm `LSUIElement` suppresses Dock icon
- [ ] 8.T3 Manual test: remove `~/.config/claudovka/bin/llama-server` before launch; confirm it is extracted on first start
- [x] 8.T4 Automated test: `make app` produces `Claudovka.app/Contents/Info.plist` with `LSUIElement = true` and correct bundle identifier

## 9. macOS .pkg Installer

- [x] 9.1 Write postinstall script: generate CA if absent, `security add-trusted-cert`, install LaunchAgent plist
- [x] 9.2 Write LaunchAgent plist (`com.claudovka.proxy.plist`) for auto-start at login
- [x] 9.3 Add `make pkg` Makefile target using `pkgbuild` + `productbuild` to produce `claudovka-<version>.pkg`
- [ ] 9.4 Test end-to-end install on a clean macOS VM; verify CA trusted, proxy auto-starts, dashboard reachable

### 9. Tests (.pkg Installer)

- [x] 9.T1 Unit test: `postinstall_should_skip_ca()` returns true when `~/.config/claudovka/ca/ca.pem` already exists
- [x] 9.T2 Unit test: postinstall script (`packaging/postinstall`) is executable and contains `security add-trusted-cert` invocation
- [x] 9.T3 Automated test: `make pkg` produces `dist/claudovka-<version>.pkg`; verify the package list (`pkgutil --payload-files`) includes the binary and LaunchAgent plist
- [ ] 9.T4 Manual test (clean macOS VM): run `.pkg`, verify CA in System keychain, proxy starts at login, `http://localhost:16443` reachable
- [ ] 9.T5 Manual test: reinstall on existing installation; verify CA is not overwritten and existing data survives

## 10. Homebrew Distribution

- [x] 10.1 Create `homebrew-claudovka` repository structure with formula and cask directories
- [x] 10.2 Write Homebrew formula `claudovka.rb`: downloads binary tarball + llama-server resource, configures `brew services` plist
- [x] 10.3 Write Homebrew cask `claudovka-app.rb`: installs `.dmg` → `/Applications/Claudovka.app`, links CLI binary
- [ ] 10.4 Test `brew install <tap>/claudovka && brew services start claudovka` on macOS
- [ ] 10.5 Test `brew install --cask <tap>/claudovka-app` and launch from Applications

### 10. Tests (Homebrew Distribution)

- [x] 10.T1 Automated test: `claudovka.rb` formula parses without error (`brew audit --strict <tap>/claudovka`)
- [x] 10.T2 Automated test: `claudovka-app.rb` cask parses without error (`brew audit --cask <tap>/claudovka-app`)
- [ ] 10.T3 Manual test: `brew install <tap>/claudovka && brew services start claudovka`; verify proxy on 16440, dashboard on 16443
- [ ] 10.T4 Manual test: `brew services stop claudovka`; verify process exits cleanly
- [ ] 10.T5 Manual test: `brew install --cask <tap>/claudovka-app`; open from Applications; verify menu bar icon

## 11. Proxy Start/Stop Toggle

- [x] 11.1 Write PID file on proxy start (`~/.config/claudovka/claudovka.pid`); remove on clean shutdown
- [x] 11.2 Implement `claudovka stop`: read PID file, send SIGTERM, wait 5s, SIGKILL if needed, remove PID file
- [x] 11.3 Handle `claudovka stop` when proxy is not running: print message, exit 0
- [x] 11.4 Implement `POST /api/proxy/stop` dashboard endpoint: drain connections, stop listener, broadcast `proxy_status { running: false }`, remove PID file
- [x] 11.5 Implement `POST /api/proxy/start` dashboard endpoint: resume listener, broadcast `proxy_status { running: true }`
- [x] 11.6 Implement `GET /api/proxy/status` endpoint returning running state, http_proxy, network_proxy, pii_mode
- [x] 11.7 Add start/stop button + status indicator (green/red) to dashboard header; update via `proxy_status` WS events
- [x] 11.8 Unit test: PID file written on start, removed on stop; `stop` with no PID exits cleanly
- [x] 11.9 Integration test: `POST /api/proxy/stop` → verify listener closed; `POST /api/proxy/start` → verify listener resumes
- [ ] 11.10 Test: PII mode change while connections are active does not produce connection errors

### 11. Tests (Proxy Start/Stop Toggle)

- [x] 11.T1 Unit test: `pid::write_pid()` creates the file; `pid::read_pid()` returns the written PID; `pid::remove_pid()` deletes it
- [x] 11.T2 Unit test: `pid::read_pid()` returns `None` when file does not exist
- [x] 11.T3 Unit test: `cmd_stop()` with no PID file prints "not running" message and exits 0
- [x] 11.T4 Unit test: `cmd_stop()` with a valid PID sends SIGTERM and waits; returns `Ok(())` when process exits within 5s
- [x] 11.T5 Unit test: `GET /api/proxy/status` JSON response includes `running`, `http_proxy`, `network_proxy`, `pii_mode` fields
- [ ] 11.T6 Integration test: `POST /api/proxy/stop` triggers shutdown and broadcasts `proxy_status { running: false }` WS event
- [ ] 11.T7 Manual test: start proxy, hit `POST /api/proxy/stop`, confirm listener closes and new connections are refused

## 12. PII Configuration Tests

### 12a. PII Mode (Off / Detect-only / Replace)

- [x] 12a.1 Unit test: `pii.mode = "off"` — pipeline not called, outbound bytes unchanged, no log lines emitted
- [x] 12a.2 Unit test: `pii.mode = "detect-only"` — spans logged at INFO, outbound body byte-identical to input
- [x] 12a.3 Unit test: `pii.mode = "replace"` — body modified, vault populated, log lines emitted
- [x] 12a.4 Integration test: PATCH config to switch `off → replace` mid-run; verify next request is redacted
- [ ] 12a.5 Integration test: dashboard PII panel reflects current mode from `GET /api/config`

### 12b. Tier 1 (Regex) Tests

- [x] 12b.1 Unit test: email detection — `contact@example.com` → `Email` span, replacement contains no `@`
- [x] 12b.2 Unit test: phone detection — E.164 and local formats both produce `Phone` spans
- [x] 12b.3 Unit test: SSN detection — `123-45-6789` → `Ssn` span, confidence 1.0
- [x] 12b.4 Unit test: API key detection — `sk-proj-abc123` and `Bearer eyJ...` → `ApiKey`/`BearerToken` spans
- [x] 12b.5 Unit test: false-positive guard — `version 1.2.3`, `call us at noon` → empty span list
- [x] 12b.6 Unit test: multiple entity types in one message — two non-overlapping spans, both replaced
- [x] 12b.7 Unit test: `pii.tiers.regex = false` — Tier 1 not invoked, pipeline returns `None`
- [x] 12b.8 Verify all Tier 1 entity type variants are covered by at least one test (`cargo test` + tarpaulin or llvm-cov line check)

### 12c. Tier 2 (NER/GLiNER) Tests

- [x] 12c.1 Unit test: `pii.tiers.ner = false` → `PiiPipeline.tier2` is `None`, only Tier 1 spans returned
- [x] 12c.2 Integration test: PATCH config enables Tier 2 without Tier 1 → 422 response
- [x] 12c.3 Unit test: model file absent → startup WARN logged, pipeline continues with Tier 1 only (no panic)
- [x] 12c.4 Unit test using `Tier2Detector` stub: `PersonName` span detected for "Alice" → included in merged spans
- [x] 12c.5 Unit test: overlapping Tier 1 + Tier 2 spans → `merge_spans` returns exactly one span (higher confidence wins)
- [x] 12c.6 Unit test: `merge_spans` — non-overlapping spans both preserved, sorted by start offset

### 12d. Tier 3 (SLM/llama-server) Tests

- [x] 12d.1 Unit test: `pii.tiers.slm = false` → `PiiPipeline.slm` is `None`, no sidecar spawned
- [x] 12d.2 Integration test: PATCH config enables Tier 3 without Tier 2 → 422 response
- [x] 12d.3 Integration test: `POST /api/models/:id/activate` with model not downloaded → 409 response
- [x] 12d.4 Unit test: model activation stops existing sidecar and starts new one with correct model path and port 16442
- [x] 12d.5 Unit test (mock SLM via `TcpListener`): SLM returns `[0]` → only span 0 confirmed, span 1 discarded
- [x] 12d.6 Unit test (mock SLM — no listener): timeout → all candidates returned unchanged, WARN logged
- [x] 12d.7 Unit test (mock SLM — HTTP 500): all candidates returned unchanged, WARN logged
- [x] 12d.8 Integration test: model selection change (`POST /api/models/:b/activate`) updates `pii.slm.model_id` in config and on disk
- [x] 12d.9 Run full `cargo test` suite including all Tier 3 mock tests; confirm zero failures

## 13. Validation

- [x] 13.1 `cargo test` passes with new port defaults, config hot-reload, and all PII tier tests
- [ ] 13.2 `openspec validate add-macos-packaging --strict` passes
- [ ] 13.3 Manual smoke test: install via `.pkg`, toggle each PII tier in dashboard, download a model, activate Tier 3, verify traffic is redacted
- [ ] 13.4 Manual smoke test: stop proxy via dashboard button, verify connections drop; restart via button, verify traffic flows again
- [ ] 13.5 Manual smoke test: install via `brew cask`, open menu bar app, enable network proxy with admin dialog, verify `/etc/hosts` and pf entries
