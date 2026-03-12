## ADDED Requirements

### Requirement: Version Labeling

Every claudovka artifact — binary, `.pkg`, `.dmg`, dashboard — SHALL carry a consistent, traceable version identifier. The version SHALL follow semantic versioning (`MAJOR.MINOR.PATCH`) defined in `Cargo.toml` as the single source of truth. At build time, a `build.rs` script SHALL capture the short git commit hash and ISO-8601 build date and embed them as compile-time environment variables so the running binary can always report its exact provenance. All distribution filenames SHALL include the version string. The `GET /api/version` dashboard endpoint and the `claudovka --version` CLI flag SHALL return the same version data.

#### Scenario: CLI version flag

- **WHEN** the user runs `claudovka --version`
- **THEN** the output is `claudovka <version> (<git-hash> <build-date>)`, e.g. `claudovka 0.2.0 (a1b2c3d 2026-03-08)`

#### Scenario: Dashboard version endpoint

- **WHEN** a GET request is made to `/api/version`
- **THEN** JSON is returned: `{ "version": "0.2.0", "git_hash": "a1b2c3d", "build_date": "2026-03-08" }`

#### Scenario: Version shown in dashboard header

- **WHEN** the dashboard is open in a browser
- **THEN** the header displays `claudovka v0.2.0` fetched from `/api/version` on load

#### Scenario: Versioned distribution filenames

- **WHEN** `make pkg` is run with version `0.2.0`
- **THEN** the output file is `claudovka-0.2.0.pkg` (or `.dmg`)
- **AND** the Homebrew formula references the same version string

#### Scenario: Startup log includes version

- **WHEN** `claudovka start` is run
- **THEN** the first WARN-level log line includes `version = "0.2.0"` and `git_hash = "a1b2c3d"` as structured fields

### Requirement: macOS .pkg Installer

The project SHALL produce a macOS `.pkg` installer that installs claudovka as a ready-to-use system service. The installer SHALL include the claudovka binary, the `llama-server` universal binary, and the macOS `.app` bundle. A postinstall script SHALL: generate the CA if not already present, add it to the System keychain with full trust, install a LaunchAgent at `~/Library/LaunchAgents/com.claudovka.proxy.plist` to start the proxy at login, and print a summary of installed components to the installer log.

#### Scenario: First-time install

- **WHEN** a user runs the `.pkg` installer and grants admin credentials
- **THEN** the binary, llama-server, and app bundle are placed in `/usr/local/bin/` and `/Applications/`
- **AND** the CA is generated (if absent) and installed to the System keychain
- **AND** a LaunchAgent is installed so the proxy starts at next login
- **AND** the dashboard is reachable at `http://localhost:16443` without further configuration

#### Scenario: Reinstall does not clobber existing CA

- **WHEN** the installer runs and a CA already exists at `~/.config/claudovka/ca/ca.pem`
- **THEN** the postinstall script skips CA generation and keychain installation
- **AND** existing conversation history is preserved

#### Scenario: Uninstall

- **WHEN** the user runs `claudovka uninstall` or a provided uninstall script
- **THEN** the LaunchAgent is unloaded and removed, the binary and app bundle are deleted, and the CA is removed from the System keychain

### Requirement: Homebrew Formula (CLI)

The project SHALL maintain a Homebrew formula in a private tap (`homebrew-claudovka`) that installs the claudovka CLI binary and the `llama-server` binary as a formula resource. `brew services start claudovka` SHALL manage the proxy as a LaunchAgent.

#### Scenario: brew install and start

- **WHEN** the user runs `brew install <tap>/claudovka && brew services start claudovka`
- **THEN** the proxy starts on ports 16440 (HTTP) and 16443 (dashboard)
- **AND** `claudovka` is available on PATH

#### Scenario: brew services stop

- **WHEN** the user runs `brew services stop claudovka`
- **THEN** the proxy process terminates and the LaunchAgent is unloaded

### Requirement: Homebrew Cask (.app bundle)

The project SHALL maintain a Homebrew cask (`claudovka-app`) that installs the `.app` bundle via a `.dmg`. The cask SHALL place the app in `/Applications/Claudovka.app` and link the CLI binary to `/usr/local/bin/claudovka`.

#### Scenario: cask install and launch

- **WHEN** the user runs `brew install --cask <tap>/claudovka-app` and opens the app
- **THEN** the menu bar icon appears, the proxy starts, and the dashboard is reachable at `http://localhost:16443`

### Requirement: macOS App Bundle (.app)

The project SHALL produce a macOS `.app` bundle with `LSUIElement = true` (agent app: no Dock icon). The bundle SHALL contain the claudovka binary, llama-server binary, and app icon. On launch the app SHALL start the HTTP proxy and dashboard, then show the menu bar icon. The bundle SHALL be buildable with `make app` and packageable with `make pkg`.

#### Scenario: Launch from Applications

- **WHEN** the user double-clicks `Claudovka.app`
- **THEN** no Dock icon appears
- **AND** a menu bar icon appears with a status indicator
- **AND** the HTTP proxy starts on port 16440 and dashboard on 16443

#### Scenario: llama-server extraction

- **WHEN** the app launches for the first time
- **AND** `~/.config/claudovka/bin/llama-server` does not exist
- **THEN** the bundled llama-server binary is copied to that path and made executable

### Requirement: macOS Menu Bar App

The menu bar icon SHALL provide the following menu items: Open Dashboard, separator, HTTP Proxy toggle (on/off with checkmark), Network Proxy toggle (on/off with checkmark, triggers admin dialog if enabling for the first time), separator, PII Protection (submenu: Off / Tier 1 / Tier 2 / Tier 3), separator, Quit.

#### Scenario: Open Dashboard

- **WHEN** the user clicks "Open Dashboard"
- **THEN** the system default browser opens `http://localhost:16443`

#### Scenario: HTTP proxy toggle

- **WHEN** the user toggles HTTP Proxy off in the menu
- **THEN** the proxy stops accepting new connections and the checkmark is removed

#### Scenario: Network proxy enable with admin prompt

- **WHEN** the user enables Network Proxy for the first time
- **THEN** a native macOS admin credentials dialog appears
- **AND** on successful auth, pf rules are applied and the network proxy starts on port 16441

#### Scenario: PII tier selection respects dependencies

- **WHEN** the user selects "Tier 2" from the PII submenu but Tier 1 is disabled
- **THEN** Tier 1 is enabled automatically before Tier 2 is activated

### Requirement: LaunchAgent for Auto-Start

A LaunchAgent plist SHALL be provided at `com.claudovka.proxy.plist`. It SHALL start `claudovka start` at login with stdout/stderr directed to `~/.config/claudovka/logs/proxy.log`. It SHALL be installed by the `.pkg` postinstall script and by the Homebrew formula's `brew services` integration.

#### Scenario: Proxy starts at login

- **WHEN** the user logs into macOS after installation
- **THEN** the claudovka proxy starts automatically within 5 seconds
- **AND** log output is written to `~/.config/claudovka/logs/proxy.log`

#### Scenario: LaunchAgent restart on crash

- **WHEN** the claudovka process exits unexpectedly
- **THEN** launchd restarts it after a 5-second delay (KeepAlive = true)

### Requirement: Network Privilege Helper

`claudovka network-enable` and `claudovka network-disable` SHALL manage two system resources that together route intercepted LLM API traffic through the network proxy: `/etc/hosts` entries for the configured intercept domains, and macOS pf rules to redirect port 443 TCP traffic to the network proxy port (16441). Before making any changes the system SHALL snapshot the original state of both resources so they can be restored exactly. Both commands SHALL use `osascript` to request admin credentials via a single native dialog. The same mechanism is used whether claudovka was installed via `.pkg` or Homebrew. The `.pkg` postinstall script MAY call `network-enable` automatically if the user opted in during install.

#### Scenario: network-enable applies /etc/hosts entries and pf rules

- **WHEN** the user runs `claudovka network-enable` (or clicks the dashboard/menu toggle)
- **THEN** a single native macOS admin credentials dialog appears
- **AND** on success, the original `/etc/hosts` content is saved to `~/.config/claudovka/backup/hosts.bak`
- **AND** entries mapping each intercept domain to `127.0.0.1` are appended to `/etc/hosts` (marked with `# claudovka`)
- **AND** `/etc/pf.anchors/claudovka` is written with the port 443 → 16441 redirect rule
- **AND** the original `/etc/pf.conf` is saved to `~/.config/claudovka/backup/pf.conf.bak`
- **AND** the anchor include is appended to `/etc/pf.conf` and the ruleset is reloaded
- **AND** a LaunchDaemon at `/Library/LaunchDaemons/com.claudovka.pf.plist` is installed and loaded for boot persistence

#### Scenario: network-disable reverts /etc/hosts and pf rules

- **WHEN** the user runs `claudovka network-disable`
- **THEN** a native macOS admin credentials dialog appears
- **AND** on success, all lines marked `# claudovka` are removed from `/etc/hosts`
- **AND** the pf anchor is flushed and the include removed from `/etc/pf.conf`
- **AND** the LaunchDaemon is unloaded and deleted
- **AND** the backup files in `~/.config/claudovka/backup/` are removed

#### Scenario: network-enable is idempotent

- **WHEN** `claudovka network-enable` is run when rules and hosts entries are already active
- **THEN** no error is reported and entries are verified/refreshed without re-prompting

#### Scenario: /etc/hosts entries scoped to intercept domains

- **WHEN** network proxy is enabled
- **THEN** only the domains listed in `intercept.domains` config are added to `/etc/hosts`
- **AND** adding or removing a domain from config and re-running `network-enable` updates `/etc/hosts` accordingly

### Requirement: Full Uninstaller

`claudovka uninstall` SHALL perform a complete, reversible removal of all changes claudovka made to the system. It SHALL be runnable at any time — even if the proxy is currently running — and SHALL NOT require a separate uninstall script or installer package. A `--purge` flag SHALL additionally delete all user data (logs, database, downloaded models, config, CA files). Without `--purge`, only system integration artefacts are removed; user data is preserved.

The uninstaller SHALL undo every system change made by installation and `network-enable`, in reverse order:

1. Stop the running proxy process (if any)
2. Unload and remove the LaunchAgent (`com.claudovka.proxy.plist`)
3. If network proxy was enabled: revert `/etc/hosts` (remove all `# claudovka` lines), flush the pf anchor, remove the pf include from `/etc/pf.conf`, unload and remove the pf LaunchDaemon (`com.claudovka.pf.plist`)
4. Remove the CA certificate from the System keychain
5. Remove the binary from `/usr/local/bin/claudovka`
6. Remove `/Applications/Claudovka.app` (if present)
7. Remove `~/.config/claudovka/bin/llama-server`
8. With `--purge`: remove `~/.config/claudovka/` entirely (models, logs, DB, config, CA, backups)
9. Print a per-step summary: each action with ✓ (done), ⚠ (skipped — already absent), or ✗ (failed)

Steps 3 and 4 require admin credentials; the uninstaller SHALL request them via a single `osascript` dialog if any privileged steps are needed.

#### Scenario: Clean uninstall without purge

- **WHEN** the user runs `claudovka uninstall` and grants admin credentials
- **THEN** the proxy is stopped, the LaunchAgent removed, `/etc/hosts` and pf are reverted, the CA is removed from keychain, and binaries are deleted
- **AND** `~/.config/claudovka/` (logs, DB, models, config) is left intact
- **AND** a summary is printed listing each action and its outcome

#### Scenario: Full purge

- **WHEN** the user runs `claudovka uninstall --purge`
- **THEN** all steps above are performed
- **AND** `~/.config/claudovka/` is removed entirely
- **AND** the summary confirms "All claudovka data deleted"

#### Scenario: Uninstall when network proxy was never enabled

- **WHEN** the user runs `claudovka uninstall` and network proxy was never enabled
- **THEN** the pf and `/etc/hosts` steps are skipped (shown as ⚠ in summary)
- **AND** no admin dialog is shown for those steps (only keychain removal requires admin)

#### Scenario: Uninstall is safe if proxy is running

- **WHEN** `claudovka uninstall` is run while the proxy process is active
- **THEN** the process is stopped gracefully before any files are removed
- **AND** in-flight connections are drained or closed cleanly
