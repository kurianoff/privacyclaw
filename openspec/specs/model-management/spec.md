# model-management Specification

## Purpose
TBD - created by archiving change add-model-weight-mgmt. Update Purpose after archive.
## Requirements
### Requirement: Auto-Download on First Run

The system SHALL automatically download `smollm2-135m` at startup when T3 is
enabled (`pii.tiers.slm = true`) but no model is active (either
`pii.slm.model_id` is unset or the model file is absent from `models_dir`).
The download MUST block proxy startup (not run in the background), display a
progress bar to the terminal, and log an INFO message before starting. On
success the system MUST update `pii.slm.model_id` in config, persist to disk,
and start the llama-server sidecar. On failure the system MUST log a WARN, disable T3 for
this session (fail-open), and continue startup without Tier 3.

#### Scenario: T3 enabled, no model present

- **GIVEN** `pii.tiers.slm = true` and no model file in `models_dir`
- **WHEN** the user runs `privacyclaw start`
- **THEN** `smollm2-135m` is downloaded with a terminal progress bar
- **AND** startup completes with T3 active using `smollm2-135m`

#### Scenario: Auto-download fails, proxy continues

- **GIVEN** `pii.tiers.slm = true` and no model file in `models_dir`
- **WHEN** the user runs `privacyclaw start` and the download fails (no network, server error, or checksum mismatch)
- **THEN** a WARN is logged with the failure reason
- **AND** T3 is disabled for this session
- **AND** the proxy starts normally with T1/T2 protection only

#### Scenario: Model already present, no auto-download

- **GIVEN** `pii.tiers.slm = true` and a valid model file exists in `models_dir`
- **WHEN** the user runs `privacyclaw start`
- **THEN** no download is initiated and startup proceeds at normal speed

### Requirement: Catalog SHA-256 Integrity

The built-in model catalog SHALL contain non-empty SHA-256 checksums for all
four GGUF entries. The system SHALL reject any download whose post-download
SHA-256 does not match the catalog value, delete the partial file, and surface
a descriptive error.

#### Scenario: Checksum verification on download

- **GIVEN** a catalog entry with a non-empty `sha256` field
- **WHEN** the GGUF file is downloaded
- **THEN** the downloaded file's SHA-256 is verified against the catalog value
- **AND** if they match, the file is kept and the download is considered successful
- **AND** if they do not match, the file is deleted and an error is returned

### Requirement: Shared llama-server Binary Path Resolution

The system SHALL resolve the path to the managed `llama-server` binary through
a single shared helper (`llama_server_bin_path()`) used by both the dashboard
activation flow and the auto-download startup flow, preventing path drift
between the two code paths.

#### Scenario: Consistent binary path across flows

- **GIVEN** a `llama-server` binary at `{config_dir}/bin/llama-server`
- **WHEN** auto-download starts a sidecar OR dashboard activates a model
- **THEN** both flows resolve the binary path identically

