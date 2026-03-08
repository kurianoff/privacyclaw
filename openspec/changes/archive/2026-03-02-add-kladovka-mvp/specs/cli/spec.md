## ADDED Requirements

### Requirement: CLI Subcommands

The binary SHALL expose the following subcommands via clap: `init`, `start`, `ca-path`, `reset-ca`, `export`.

#### Scenario: init subcommand

- **WHEN** the user runs `kladovka init`
- **THEN** CA certificate generation is performed and setup instructions are printed

#### Scenario: start subcommand

- **WHEN** the user runs `kladovka start`
- **THEN** the MITM proxy and dashboard server start and listen on configured addresses
- **AND** startup information is printed including proxy address, dashboard URL, and intercepted domains

#### Scenario: ca-path subcommand

- **WHEN** the user runs `kladovka ca-path`
- **THEN** the absolute path to the CA certificate file is printed to stdout

#### Scenario: reset-ca subcommand

- **WHEN** the user runs `kladovka reset-ca`
- **THEN** the existing CA is deleted and a new one is generated

#### Scenario: export subcommand

- **WHEN** the user runs `kladovka export --format json --output file.json`
- **THEN** all conversations and messages are exported to the specified file in JSON format

### Requirement: Configuration File

The system SHALL load configuration from a TOML file at a platform-appropriate path (`~/.config/kladovka/config.toml`) with sensible defaults when the file is absent.

#### Scenario: Config loaded from file

- **WHEN** a config file exists at the default path
- **THEN** it is loaded and its values override defaults

#### Scenario: Config defaults used

- **WHEN** no config file exists
- **THEN** the proxy starts with default values: proxy on `127.0.0.1:8080`, dashboard on `127.0.0.1:8443`, default intercept domains

#### Scenario: Custom config path

- **WHEN** the user passes `--config ./custom.toml`
- **THEN** the specified config file is loaded instead of the default

### Requirement: Structured Logging

The system SHALL use the `tracing` crate for structured logging with a configurable level from the config file (`trace`, `debug`, `info`, `warn`, `error`).

#### Scenario: Log level from config

- **WHEN** `[logging].level = "debug"` is set in the config
- **THEN** debug-level log messages are emitted to stderr
