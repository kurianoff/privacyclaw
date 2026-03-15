# Change: Add macOS Packaging and Configuration UI

## Why

Privacyclaw requires manual setup that is too complex for most users: generating CAs, adding system trust anchors, configuring pf firewall rules, sourcing and running a llama-server binary, and editing TOML files to adjust behavior. Packaging the tool as a native macOS experience closes the gap between CLI prototype and usable product. Users should be able to install, configure, and run the full stack — including Tier 3 PII protection — without touching a terminal.

## What Changes

- **BREAKING**: Default ports change to avoid conflicts with common dev tooling:
  - HTTP proxy: `127.0.0.1:8080` → `127.0.0.1:16440`
  - Network proxy: `127.0.0.1:4443` → `127.0.0.1:16441`
  - llama-cpp sidecar: `127.0.0.1:8081` → `127.0.0.1:16442`
  - Dashboard: `127.0.0.1:8443` → `127.0.0.1:16443`

- **New capability — packaging**: macOS `.pkg` installer and Homebrew distribution (formula + cask). The `.pkg` handles CA installation, LaunchAgent setup, and privilege helper installation. The Homebrew cask distributes the `.app` bundle; the formula distributes the CLI binary.

- **New capability — menu bar app**: macOS system tray icon (agent app, no Dock icon) for at-a-glance status and quick start/stop. Both the `.app` bundle and the Homebrew cask include the tray app. CLI-only formula install does not.

- **New capability — model management**: built-in catalog of four supported GGUF models with dashboard UI for download and activation. The `llama-server` binary is bundled in the installer; models are downloaded on demand.

- **New capability — network privilege helper**: `privacyclaw network-enable` / `network-disable` subcommands apply and revert pf rules using a native macOS admin dialog (`osascript`). Works identically whether installed via `.pkg` or Homebrew. The `.pkg` additionally installs a LaunchDaemon for boot-persistence; Homebrew users use `brew services` for the proxy daemon itself.

- **Modified capability — dashboard**: new Configuration panel with live toggles for HTTP proxy, network proxy, and PII tiers (with dependency enforcement). New Model Management panel with download/activate controls. Backed by a new config REST API with hot-reload.

- **Modified capability — CLI**: adds `network-enable` and `network-disable` subcommands. Updates documented default ports.

## Impact

- Affected specs: `cli`, `dashboard`, new `packaging`, new `model-management`
- Affected code: `config.rs` (hot-reload, new ports), `main.rs` (tray event loop, new subcommands), `dashboard/` (config API endpoints + UI), `pii/tier3.rs` (sidecar port), build infrastructure (Makefile, scripts/)
- Existing users with hardcoded `8080`/`8443` configs will need to update — the TOML override continues to work
