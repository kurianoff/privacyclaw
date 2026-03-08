## ADDED Requirements

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

## MODIFIED Requirements

### Requirement: start Subcommand

The `claudovka start` subcommand SHALL accept optional PII flags that override the configuration file.

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
