# Design: macOS Packaging and Configuration UI

## Context

Claudovka is a Rust CLI binary that runs a MITM proxy, a network proxy, and a local dashboard. The packaging effort introduces four orthogonal concerns: distribution format, a system-tray UI layer, live configuration management, and on-demand model lifecycle. Each has meaningful trade-offs that must be settled before implementation begins.

## Goals / Non-Goals

- **Goals**: seamless macOS install for non-developer users; GUI-accessible controls for all features; bundled llama-server so Tier 3 requires no manual steps; consistent behaviour across `.pkg` and Homebrew install paths.
- **Non-Goals**: Windows or Linux packaging (deferred); full signed/notarized distribution pipeline (noted but not in scope of this change); automatic model updates; remote config management.

## Decisions

### 1. Menu bar app: `tray-icon` + `muda` Rust crates

**Decision**: implement the menu bar UI in pure Rust using [`tray-icon`](https://crates.io/crates/tray-icon) and [`muda`](https://crates.io/crates/muda) rather than a thin Swift wrapper.

- `tray-icon` requires the event loop to run on the main thread on macOS. The async tokio runtime runs on a background thread pool; the main thread drives the tray event loop.
- The app bundle sets `LSUIElement = true` in `Info.plist` (agent app: no Dock icon, no application menu).
- A `--tray` CLI flag enables tray mode; the `.app` bundle's launch script passes this flag automatically. Plain `claudovka start` from a terminal works unchanged.
- **Alternative considered**: Swift launcher that spawns the Rust daemon. Rejected — adds a second language, complicates the build, and `tray-icon` already provides native macOS integration.

### 2. Network proxy privilege escalation: `osascript` admin dialog

**Decision**: use `osascript -e 'do shell script "..." with administrator privileges'` to request root access for pf rule management.

- Shows a native macOS credentials dialog; works from any context (terminal, app, service).
- `claudovka network-enable` writes a pf anchor at `/etc/pf.anchors/claudovka`, adds an include to `/etc/pf.conf` if not present, and calls `pfctl -f /etc/pf.conf`.
- For boot persistence it installs a LaunchDaemon at `/Library/LaunchDaemons/com.claudovka.pf.plist` (via the same admin auth) that re-applies the anchor on boot.
- `claudovka network-disable` reverts all of the above (removes anchor, removes include, unloads and removes the LaunchDaemon).
- **Brew vs .pkg difference**: the `.pkg` postinstall script calls `network-enable` automatically (with admin auth granted by the installer); brew users call `claudovka network-enable` once manually or from the dashboard toggle. Behaviour and outcome are identical.
- **Alternative considered**: SMJobBless privilege helper. More secure isolation but significant signing/entitlements complexity — deferred to a future hardening change.

### 3. Config hot-reload: `Arc<RwLock<Config>>`

**Decision**: replace the current owned `Config` value with `Arc<RwLock<Config>>` threaded through all subsystems.

- A `ConfigManager` wraps the `RwLock`, validates incoming patches, persists to TOML, and broadcasts a `config_changed` WebSocket event.
- Changes to PII tier settings take effect on the next proxied request (no restart needed).
- Changes to listening addresses (proxy port, dashboard port) emit a UI warning: "Port changes require a restart."
- **Why not file-watch hot-reload**: avoids inotify/FSEvents complexity; config changes always go through the dashboard API where validation is enforced.

### 4. llama-server bundling

**Decision**: ship a pre-built `llama-server` binary inside the installer and app bundle.

- Source: the official llama.cpp GitHub Releases asset for macOS (`llama-server-macos-arm64`, `llama-server-macos-x86_64`). Version pinned in the Makefile.
- Stored at `~/.config/claudovka/bin/llama-server` on first launch (extracted from bundle if not present).
- The `.pkg` ships both architectures; the brew formula downloads the matching arch as a resource.
- **Alternative considered**: building llama.cpp from source as part of the Rust build. Rejected — llama.cpp is a large C++ project; binary distribution is standard practice and keeps build times sane.

### 5. Model catalog: hardcoded metadata + HuggingFace direct URLs

**Decision**: embed a static catalog of four GGUF models with direct HuggingFace download URLs, expected SHA-256 checksums, and RAM estimates. No HuggingFace token required (all are public).

| Model | Q4 file size | RAM | URL pattern |
|---|---|---|---|
| SmolLM2-135M | ~90 MB | ~300 MB | HF `HuggingFaceTB/SmolLM2-135M-Instruct-GGUF` |
| Qwen2.5-0.5B | ~400 MB | ~800 MB | HF `Qwen/Qwen2.5-0.5B-Instruct-GGUF` |
| Llama-3.2-1B | ~700 MB | ~1.2 GB | HF `bartowski/Llama-3.2-1B-Instruct-GGUF` |
| Phi-3-mini 3.8B | ~2.3 GB | ~3.5 GB | HF `microsoft/Phi-3-mini-4k-instruct-gguf` |

- Download uses chunked HTTP with progress events streamed via WebSocket.
- Checksum verified after download; partial files deleted on failure or cancel.
- Active model stored in `config.toml` as `pii.slm.model_id`; sidecar restarted when it changes.

### 6. Distribution: `.pkg` + Homebrew formula + Homebrew cask

**Decision**: maintain three distribution artifacts from a single Makefile:

1. **`.pkg`** — full installer for non-developer users. Includes the `.app` bundle, `llama-server` binary (universal), and postinstall scripts that install the CA, LaunchAgent, and optionally the network LaunchDaemon.
2. **Homebrew formula** — CLI binary + bundled `llama-server` resource. `brew services start claudovka` manages the LaunchAgent. No tray app.
3. **Homebrew cask** — distributes the `.app` bundle as a `.dmg`. Targets the same user segment as `.pkg` but installs via Homebrew. Includes the tray app.

- The tap is a separate `homebrew-claudovka` Git repository (can be a private repo or hosted on the same GitHub account).

## Risks / Trade-offs

- **pf rule fragility**: other tools (VPN software, Little Snitch) can interfere with pf. `network-disable` attempts a clean rollback but cannot guarantee pf state is restored if the user manually edited `pf.conf` between enable and disable.
- **llama-server version drift**: the pinned binary may lag behind llama.cpp releases. A future `claudovka update-llama-server` command can refresh it.
- **tray-icon macOS event loop**: `tray-icon` requires `NSApplication` on macOS. If Apple changes the API this may break. Mitigation: the tray is purely additive; `claudovka start` without `--tray` is unaffected.
- **Port collisions**: 16440–16443 are chosen to be obscure but are not IANA-reserved. A future config UI port-selection feature can handle collisions.

## Migration Plan

Existing users with default config (no `config.toml`) will see ports change on upgrade. The changelog and README will document the new defaults. Users with explicit `[proxy].listen` or `[network_proxy].listen` in their TOML are unaffected.

## Open Questions

- Code signing / notarization: the `.pkg` and `.app` require a Developer ID certificate for Gatekeeper. This can be handled via a CI signing step (GitHub Actions + Secrets) in a follow-up. For now, users may need to right-click → Open on first launch.
- Homebrew tap hosting: private tap on GitHub (`github.com/<user>/homebrew-claudovka`) is sufficient for now. Public tap can be a follow-up if the tool is open-sourced.
