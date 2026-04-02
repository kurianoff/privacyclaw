# Change: Add Model Weight Management — Auto-Download on First Run for T3

## Why

When a user enables Tier 3 (SLM) in config but has no model downloaded, the proxy starts silently without T3 — no error, no download, no guidance. The four catalog models also have empty SHA-256 fields, skipping integrity verification on every download. Users lack documented guidance on model size/quality/latency tradeoffs.

## What Changes

- **`src/models/mod.rs`**: Populate SHA-256 checksums for all four GGUF catalog entries (smollm2-135m, qwen2.5-0.5b, llama-3.2-1b, phi-3-mini-3.8b).
- **`src/dashboard/mod.rs`**: Extract `llama_server_bin_path()` helper to eliminate hardcoded path duplication.
- **`src/dashboard/mod.rs`**: Change `dashboard::run()` signature to accept an external `SidecarHandle` parameter instead of creating one internally.
- **`src/main.rs`**: Hoist `SidecarHandle` creation into `cmd_start`; pass handle to `dashboard::run()`.
- **`src/main.rs`**: Add `ensure_slm_model()` async function — checks T3 state, auto-downloads `smollm2-135m` if needed, starts sidecar, persists config.
- **`src/main.rs`** (`run_tray_mode`): Reorder to build tokio runtime before `load_proxy_resources` so `ensure_slm_model` can run via `rt.block_on()`.
- **`docs/PII-SETUP.md`**: Add "Choosing a Model" section with size/RAM/latency/quality comparison table.
- **`openspec/changes/add-macos-packaging/specs/model-management/spec.md`**: Append "Auto-Download on First Run" requirement.

## Impact

- Affected specs: model-management
- Affected code: `src/main.rs`, `src/dashboard/mod.rs`, `src/models/mod.rs`, `docs/PII-SETUP.md`, `openspec/changes/add-macos-packaging/specs/model-management/spec.md`
- No breaking changes to external API or config schema
- `dashboard::run()` signature change is internal (only two call sites: `cmd_start` and `run_tray_mode`)
