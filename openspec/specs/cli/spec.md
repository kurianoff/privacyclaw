# cli Specification

## Purpose
TBD - created by archiving change add-kladovka-mvp. Update Purpose after archive.
## Requirements
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

The system SHALL use the `tracing` crate for structured logging with a configurable level and format. The level is set from the config file (`trace`, `debug`, `info`, `warn`, `error`); the format is set via `[logging].format` (`"json"` or `"text"`, default `"json"`). The subscriber SHALL be initialized as the first action in `main()`, before `Config::load`, using `RUST_LOG` or a hardcoded `INFO` default as the bootstrap filter, then updated to the configured level after config is loaded.

#### Scenario: Log level from config

- **WHEN** `[logging].level = "debug"` is set in the config
- **THEN** debug-level log messages are emitted to stderr

#### Scenario: Log format defaults to JSON

- **WHEN** `[logging].format` is absent from the config
- **THEN** log output is emitted as newline-delimited JSON objects
- **AND** each record contains `ts`, `level`, `target`, and `msg` fields at minimum

#### Scenario: RUST_LOG env var takes precedence

- **WHEN** `RUST_LOG=claudovka=trace` is set in the environment
- **THEN** the effective log level is TRACE regardless of `[logging].level` in the config

### Requirement: test-pii Subcommand

The `claudovka test-pii` subcommand SHALL run the configured PII detection tiers against user-supplied text and print all detected entities in a human-readable table. It SHALL not require the proxy to be running.

#### Scenario: Detections printed as table

- **WHEN** the user runs `claudovka test-pii "My email is john@acme.com"`
- **THEN** stdout contains a table with columns: Type, Original, Synthetic, Tier, Confidence
- **AND** the row shows `EMAIL | john@acme.com | alice.brown@example.com | 1 | 1.0`

#### Scenario: Locale flag

- **WHEN** the user adds `--locale in-IN`
- **THEN** Tier 1 loads the Indian locale pack in addition to the default patterns

#### Scenario: JSON output

- **WHEN** the user adds `--format json`
- **THEN** stdout is a JSON array of detection objects (for programmatic use)

---

### Requirement: models Subcommand

The `claudovka models` subcommand SHALL manage the download and installation of optional ML model files required for Tier 2 (GLiNER ONNX) and Tier 3 (Anonymizer SLM GGUF). Models SHALL be stored in the configured `pii.ner.model_path` directory.

#### Scenario: Install GLiNER model

- **WHEN** the user runs `claudovka models install gliner-pii-base`
- **THEN** the proxy downloads the ONNX model (~200MB) to `~/.config/claudovka/models/`
- **AND** prints a progress indicator during download
- **AND** verifies the sha256 checksum after download

#### Scenario: Install with existing model

- **WHEN** the model is already installed at the target path
- **THEN** the command skips download and prints `Already installed`

#### Scenario: List installed models

- **WHEN** the user runs `claudovka models list`
- **THEN** stdout lists all installed models with name, version, size, and path

#### Scenario: Network error during download

- **WHEN** the download fails due to a network error
- **THEN** the partial file is deleted
- **AND** an error message is printed
- **AND** the exit code is non-zero

---

### Requirement: benchmark Subcommand

The `claudovka benchmark` subcommand SHALL run the PII detection pipeline against a local evaluation dataset and report precision, recall, and F1 per entity type.

#### Scenario: Benchmark against local fixture

- **WHEN** the user runs `claudovka benchmark`
- **THEN** the pipeline runs against bundled test fixtures
- **AND** outputs a summary table of F1, precision, recall per entity type

#### Scenario: Tier-specific benchmark

- **WHEN** the user runs `claudovka benchmark --tier 1`
- **THEN** only Tier 1 (regex) is evaluated

#### Scenario: HTML report

- **WHEN** the user runs `claudovka benchmark --report html`
- **THEN** an HTML report is written to `./benchmark-report.html`

---

### Requirement: PII Override Flags on start and network-start

The `claudovka start` and `claudovka network-start` subcommands SHALL accept optional `--pii` and `--pii-llm` flags that override the PII mode set in the configuration file.

#### Scenario: PII enabled via flag

- **WHEN** the user runs `claudovka start --pii`
- **THEN** `pii.mode` is set to `"replace"` and Tiers 1+2 are active (equivalent to `pii.tiers.regex = true, pii.tiers.ner = true`)

#### Scenario: PII with SLM sidecar

- **WHEN** the user runs `claudovka start --pii --llm`
- **THEN** Tier 3 is also enabled and `claudovka` attempts to start the llama-server sidecar process

#### Scenario: No flags — default behavior unchanged

- **WHEN** the user runs `claudovka start` without `--pii`
- **THEN** PII mode defaults to the config file value (default `"off"`)
- **AND** behavior is identical to Phase 1

### Requirement: --log-file Global CLI Flag

The binary SHALL accept a `--log-file <PATH>` global flag on all subcommands. When provided, it overrides the `[logging].file` value from the configuration file and activates file log output regardless of the config file setting.

#### Scenario: Log file path set via flag

- **WHEN** the user runs `claudovka start --log-file /tmp/claudovka-debug.log`
- **THEN** log records are written to `/tmp/claudovka-debug.log` in addition to stderr
- **AND** the file is created if it does not exist (the parent directory must already exist)
- **AND** the `[logging].file` value in `config.toml` is ignored for this invocation

#### Scenario: Flag absent — config file controls file output

- **WHEN** the user runs `claudovka start` without `--log-file`
- **THEN** file output is controlled entirely by `[logging].file` in the config
- **AND** if `[logging].file` is absent, no log file is written

#### Scenario: Flag works with all subcommands

- **WHEN** the user runs `claudovka test-pii --log-file /tmp/out.log "test text"`
- **THEN** the PII detection log records (TRACE through INFO) are written to `/tmp/out.log`
- **AND** the `test-pii` command still prints its human-readable table to stdout as usual

#### Scenario: Non-writable path

- **WHEN** the user passes `--log-file /nonexistent/dir/app.log` and the parent directory does not exist
- **THEN** the proxy logs a WARN to stderr and continues without file output
- **AND** startup is not aborted due to this error

