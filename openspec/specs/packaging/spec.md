# packaging Specification

## Purpose
TBD - created by archiving change update-homebrew-formula-sidecar. Update Purpose after archive.
## Requirements
### Requirement: Homebrew Sidecar Distribution

The Homebrew formula (`packaging/homebrew/privacyclaw.rb`) SHALL distribute the
Python SLM sidecar (`privacyclaw-slm-sidecar`) as a directly invocable binary
in the Homebrew `bin/` directory, isolated in a Homebrew-managed virtualenv with
all required pip dependencies pre-installed.

#### Scenario: Sidecar installed alongside Rust binary
- **WHEN** a user runs `brew install privacyclaw`
- **THEN** both `privacyclaw` (Rust binary) and `privacyclaw-slm-sidecar` (Python script) are present in the Homebrew `bin/` directory

#### Scenario: Sidecar uses virtualenv Python
- **WHEN** the sidecar script is invoked
- **THEN** its shebang resolves to `opt_libexec/bin/python3`, the Homebrew-managed virtualenv Python, not the system Python

#### Scenario: Shebang survives brew upgrade
- **WHEN** `brew upgrade privacyclaw` is run
- **THEN** `opt_libexec` resolves to the new keg's libexec and the sidecar shebang remains valid

### Requirement: Sidecar Version Flag

The Python SLM sidecar script SHALL implement a `--version` / `-V` flag that
identifies itself without requiring any pip dependencies to be importable.

#### Scenario: Version flag returns version string
- **WHEN** `privacyclaw-slm-sidecar --version` is executed
- **THEN** the output matches `privacyclaw-slm-sidecar 0.1.0` and the process exits 0

#### Scenario: Version flag bypasses dependency guard
- **WHEN** `privacyclaw-slm-sidecar --version` is executed in an environment where fastapi, uvicorn, httpx, or pydantic are not installed
- **THEN** the version string is printed and the process exits 0 without attempting to import any pip dependency

### Requirement: pip Dependency Isolation

The formula SHALL declare all direct and required transitive pip dependencies as
pinned `resource` blocks with SHA-256 checksums. The `pydantic-core` package
SHALL use pre-built platform-specific wheels (not sdist) to avoid requiring a
Rust toolchain on end-user machines.

#### Scenario: All pip deps available in virtualenv
- **WHEN** the formula is installed on a machine without pip, virtualenv, or a Rust toolchain
- **THEN** fastapi, uvicorn, httpx, pydantic, and all transitive dependencies are importable from the sidecar's virtualenv Python

#### Scenario: pydantic-core uses platform wheel on Apple Silicon
- **WHEN** installing on an Apple Silicon Mac
- **THEN** the pydantic-core resource resolves to an arm64 wheel tagged macosx_11_0 or lower (no compilation required)

#### Scenario: pydantic-core uses platform wheel on Intel Mac
- **WHEN** installing on an Intel Mac
- **THEN** the pydantic-core resource resolves to an x86_64 wheel tagged macosx_11_0 or lower (no compilation required)

### Requirement: Homebrew Formula Smoke Test

The formula `test do` block SHALL include a sidecar smoke test that verifies the
sidecar binary is installed and self-identifies via the `--version` flag.

#### Scenario: brew test passes for sidecar
- **WHEN** `brew test privacyclaw` is run in the Homebrew sandbox
- **THEN** the sidecar `--version` assertion passes without requiring pip deps, network access, or the llama-server binary

### Requirement: Homebrew Caveats for T3 Pipeline

The formula `caveats` block SHALL document the T3 model setup workflow and how
to invoke the Python sidecar for advanced use.

#### Scenario: Caveats mention sidecar invocation
- **WHEN** a user runs `brew info privacyclaw` after install
- **THEN** the caveats include instructions for invoking `privacyclaw-slm-sidecar` and the model management CLI

