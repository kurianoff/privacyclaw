## ADDED Requirements

### Requirement: --log-file Global CLI Flag

The binary SHALL accept a `--log-file <PATH>` global flag on all subcommands. When provided, it overrides the `[logging].file` value from the configuration file and activates file log output regardless of the config file setting.

#### Scenario: Log file path set via flag

- **WHEN** the user runs `privacyclaw start --log-file /tmp/privacyclaw-debug.log`
- **THEN** log records are written to `/tmp/privacyclaw-debug.log` in addition to stderr
- **AND** the file is created if it does not exist (the parent directory must already exist)
- **AND** the `[logging].file` value in `config.toml` is ignored for this invocation

#### Scenario: Flag absent — config file controls file output

- **WHEN** the user runs `privacyclaw start` without `--log-file`
- **THEN** file output is controlled entirely by `[logging].file` in the config
- **AND** if `[logging].file` is absent, no log file is written

#### Scenario: Flag works with all subcommands

- **WHEN** the user runs `privacyclaw test-pii --log-file /tmp/out.log "test text"`
- **THEN** the PII detection log records (TRACE through INFO) are written to `/tmp/out.log`
- **AND** the `test-pii` command still prints its human-readable table to stdout as usual

#### Scenario: Non-writable path

- **WHEN** the user passes `--log-file /nonexistent/dir/app.log` and the parent directory does not exist
- **THEN** the proxy logs a WARN to stderr and continues without file output
- **AND** startup is not aborted due to this error

## MODIFIED Requirements

### Requirement: Structured Logging

The system SHALL use the `tracing` crate for structured logging with a configurable level and format. The level is set from the config file (`trace`, `debug`, `info`, `warn`, `error`); the format is set via `[logging].format` (`"json"` or `"text"`, default `"json"`). The subscriber SHALL be initialized as the first action in `main()`, before `Config::load`, using `RUST_LOG` or a hardcoded `INFO` default as the bootstrap filter, then updated to the configured level after config is loaded.

#### Scenario: Log level from config

- **WHEN** `[logging].level = "debug"` is set in the config
- **THEN** debug-level log messages are emitted to stderr

#### Scenario: Log format defaults to JSON

- **WHEN** `[logging].format` is absent from the config
- **THEN** log output is emitted as newline-delimited JSON objects
- **AND** each record contains `ts`, `level`, `target`, and `msg` fields at minimum

#### Scenario: RUST_LOG env var takes precedence

- **WHEN** `RUST_LOG=privacyclaw=trace` is set in the environment
- **THEN** the effective log level is TRACE regardless of `[logging].level` in the config
