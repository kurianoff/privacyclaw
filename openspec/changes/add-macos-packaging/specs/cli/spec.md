## MODIFIED Requirements

### Requirement: CLI Subcommands

The binary SHALL expose the following subcommands via clap: `init`, `start`, `stop`, `network-enable`, `network-disable`, `ca-path`, `reset-ca`, `export`, `uninstall`.

#### Scenario: init subcommand

- **WHEN** the user runs `privacyclaw init`
- **THEN** CA certificate generation is performed and setup instructions are printed

#### Scenario: start subcommand

- **WHEN** the user runs `privacyclaw start`
- **THEN** the MITM proxy and dashboard server start and listen on configured addresses
- **AND** startup information is printed including proxy address, dashboard URL, and intercepted domains

#### Scenario: start with tray flag

- **WHEN** the user runs `privacyclaw start --tray`
- **THEN** the proxy and dashboard start as above
- **AND** a macOS menu bar icon appears with a status indicator and control menu

#### Scenario: stop subcommand

- **WHEN** the user runs `privacyclaw stop`
- **THEN** the PID file at `~/.config/privacyclaw/privacyclaw.pid` is read
- **AND** SIGTERM is sent to the process; if it does not exit within 5 seconds, SIGKILL is sent
- **AND** the PID file is removed and "Proxy stopped" is printed to stdout

#### Scenario: stop when proxy is not running

- **WHEN** `privacyclaw stop` is run and no PID file exists (or the PID is not alive)
- **THEN** "Proxy is not running" is printed and the command exits with code 0

#### Scenario: network-enable subcommand

- **WHEN** the user runs `privacyclaw network-enable`
- **THEN** a single native macOS admin credentials dialog is shown via osascript
- **AND** on successful authentication, `/etc/hosts` entries are added for all intercept domains pointing to `127.0.0.1`
- **AND** pf redirect rules are applied for port 443 → 16441
- **AND** a LaunchDaemon is installed for pf boot persistence
- **AND** original `/etc/hosts` and `/etc/pf.conf` are backed up to `~/.config/privacyclaw/backup/`
- **AND** "Network proxy enabled" is printed to stdout

#### Scenario: network-disable subcommand

- **WHEN** the user runs `privacyclaw network-disable`
- **THEN** a native macOS admin credentials dialog is shown
- **AND** on successful authentication, all `# privacyclaw` lines are removed from `/etc/hosts`
- **AND** pf rules are reverted and the LaunchDaemon is removed
- **AND** "Network proxy disabled" is printed to stdout

#### Scenario: ca-path subcommand

- **WHEN** the user runs `privacyclaw ca-path`
- **THEN** the absolute path to the CA certificate file is printed to stdout

#### Scenario: reset-ca subcommand

- **WHEN** the user runs `privacyclaw reset-ca`
- **THEN** the existing CA is deleted and a new one is generated

#### Scenario: export subcommand

- **WHEN** the user runs `privacyclaw export --format json --output file.json`
- **THEN** all conversations and messages are exported to the specified file in JSON format

#### Scenario: uninstall subcommand

- **WHEN** the user runs `privacyclaw uninstall`
- **THEN** the proxy process is stopped (if running), the LaunchAgent is unloaded and removed, `/etc/hosts` privacyclaw entries are reverted, pf rules are reverted, the pf LaunchDaemon is removed, the CA is removed from the System keychain, and binaries and the app bundle are deleted
- **AND** user data in `~/.config/privacyclaw/` is preserved
- **AND** a per-step summary is printed with ✓ / ⚠ / ✗ for each action

#### Scenario: uninstall with purge flag

- **WHEN** the user runs `privacyclaw uninstall --purge`
- **THEN** all system artefacts are removed as above
- **AND** `~/.config/privacyclaw/` is deleted entirely (logs, DB, models, config, CA, backups)

### Requirement: Configuration File

The system SHALL load configuration from a TOML file at `~/.config/privacyclaw/config.toml` with sensible defaults when the file is absent. Default listening addresses SHALL be: HTTP proxy `127.0.0.1:16440`, network proxy `127.0.0.1:16441`, llama-cpp sidecar `127.0.0.1:16442`, dashboard `127.0.0.1:16443`.

#### Scenario: Config loaded from file

- **WHEN** a config file exists at the default path
- **THEN** it is loaded and its values override defaults

#### Scenario: Config defaults used

- **WHEN** no config file exists
- **THEN** the proxy starts with default values: HTTP proxy on `127.0.0.1:16440`, dashboard on `127.0.0.1:16443`, default intercept domains

#### Scenario: Custom config path

- **WHEN** the user passes `--config ./custom.toml`
- **THEN** the specified config file is loaded instead of the default

#### Scenario: Config hot-reload via API

- **WHEN** a PATCH request is made to `/api/config`
- **THEN** the running process updates its in-memory config without restart (except for port changes)
- **AND** the updated values are persisted to `config.toml`
