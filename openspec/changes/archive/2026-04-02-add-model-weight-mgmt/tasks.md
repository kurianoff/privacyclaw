## 1. Catalog SHA-256 Values

- [x] 1.1 Download each of the four GGUF files from HuggingFace and compute SHA-256. Run all four in parallel (separate terminal tabs or background jobs) to minimize wall-clock time (~2.3 GB largest file):
  - `smollm2-135m-instruct-q4_k_m.gguf` (~90 MB) — URL in `catalog()[0].url`
  - `qwen2.5-0.5b-instruct-q4_k_m.gguf` (~400 MB) — URL in `catalog()[1].url`
  - `Llama-3.2-1B-Instruct-Q4_K_M.gguf` (~700 MB) — URL in `catalog()[2].url`
  - `Phi-3-mini-4k-instruct-q4.gguf` (~2.3 GB) — URL in `catalog()[3].url`
  - Command per file: `curl -L <url> | shasum -a 256` (macOS) or `curl -L <url> | sha256sum` (Linux)
- [x] 1.2 In `src/models/mod.rs`, populate the `sha256: ""` field of each `ModelInfo` entry in the static `CATALOG` slice with the computed lowercase hex string.
- [x] 1.3 Verify: `cargo build` succeeds. The existing `download_with_bar` code already checks `info.sha256` when non-empty (line 280 of `src/models/mod.rs`) — no new test code needed for the checksum path itself.

## 2. Extract llama-server Binary Path Helper

- [x] 2.1 In `src/dashboard/mod.rs`, add a `pub(crate)` free function near the top of the file:
  ```rust
  pub(crate) fn llama_server_bin_path() -> std::path::PathBuf {
      crate::config::default_config_dir().join("bin/llama-server")
  }
  ```
- [x] 2.2 Replace the hardcoded `crate::config::default_config_dir().join("bin/llama-server")` at line 474 of `src/dashboard/mod.rs` with `llama_server_bin_path()`.
- [x] 2.3 Verify: `cargo build` succeeds; behavior is identical (pure refactor).

## 3. Hoist SidecarHandle — dashboard::run() Signature Change

- [x] 3.1 In `src/dashboard/mod.rs`, make `SidecarHandle` accessible from `main.rs`:
  - Change `type SidecarHandle = ...` to `pub(crate) type SidecarHandle = ...`.
- [x] 3.2 In `src/dashboard/mod.rs`, change `pub async fn run(...)` to accept `sidecar: SidecarHandle` as an additional parameter (add as last parameter):
  - Remove the line `let sidecar: SidecarHandle = Arc::new(Mutex::new(None));` from the function body.
  - The `sidecar` variable is then used identically in the rest of the function (cloned into each connection handler).
- [x] 3.3 In `src/main.rs` (`cmd_start`), add the following immediately after the function's opening variable declarations and before any other logic (so it is available for both `ensure_slm_model` in Task 4 and the dashboard spawn):
  ```rust
  let sidecar: dashboard::SidecarHandle = Arc::new(Mutex::new(None));
  ```
  Pass `sidecar.clone()` to `dashboard::run(...)` at the dashboard spawn site (currently line 677).
- [x] 3.4 In `src/main.rs` (`run_tray_mode`), add a corresponding `SidecarHandle` creation and pass it to `dashboard::run(...)` at the dashboard spawn site (currently line 394). This is completed as part of Task 5 where the tray path is restructured.
- [x] 3.5 Verify: `cargo build` succeeds. Dashboard activate/deactivate flow is unaffected (the handle is now passed in rather than created internally, but used identically).

## 4. ensure_slm_model Function

- [x] 4.1 In `src/main.rs`, add the following `use` if not already present:
  ```rust
  use std::sync::Mutex;
  ```
  Also ensure `serde_json` is accessible (it is already used in `main.rs` at line 807 — no new import needed).
- [x] 4.2 In `src/main.rs`, add `async fn ensure_slm_model(cfg: &mut Config, cfg_mgr: &Arc<ConfigManager>, sidecar: &dashboard::SidecarHandle)`.
- [x] 4.3 Implement the early-return guard at the top of `ensure_slm_model`:
  ```rust
  if !cfg.pii.tiers.slm { return; }
  let models_dir = cfg.resolved_models_dir();
  if let Some(ref id) = cfg.pii.slm.model_id.clone() {
      if !id.is_empty() && crate::models::is_downloaded(&models_dir, id) {
          tracing::debug!(model_id = %id, "T3 model already present, skipping auto-download");
          return;
      }
  }
  ```
  Note: `resolved_models_dir()` is defined in `src/config.rs` at line 242. `is_downloaded()` is defined in `src/models/mod.rs` at line 123 — takes `(models_dir: &Path, id: &str)`.
- [x] 4.4 Implement the download call:
  ```rust
  tracing::info!(model_id = "smollm2-135m", size_mb = 90, "T3 enabled but no model active; auto-downloading");
  let smollm2_info = &crate::models::catalog()[0]; // smollm2-135m is index 0
  let result = crate::models::download_with_bar(smollm2_info, &models_dir).await;
  ```
  Note: `download_with_bar` is defined in `src/models/mod.rs` at line 210 — takes `(info: &'static ModelInfo, models_dir: &Path)`. `catalog()` returns `&'static [ModelInfo]` so `&catalog()[0]` satisfies the `'static` lifetime.
- [x] 4.5 Implement the failure branch (download Err):
  ```rust
  match result {
      Err(e) => {
          tracing::warn!(err = %e, "auto-download failed; T3 disabled for this session");
          cfg.pii.tiers.slm = false;
          return;
      }
      Ok(()) => { /* continue to success branch */ }
  }
  ```
- [x] 4.6 Implement the success branch after download Ok:
  - Set `cfg.pii.slm.model_id = Some("smollm2-135m".to_string())`.
  - Do NOT mutate `cfg.pii.slm.endpoint`. The config default is already `http://127.0.0.1:16442` (see `src/config.rs` line 177) — the correct sidecar port. Mutating it would override any user-configured custom endpoint.
  - Persist config:
    ```rust
    let patch = serde_json::json!({"pii": {"slm": {"model_id": "smollm2-135m"}}});
    if let Err(e) = cfg_mgr.patch(patch).await {
        tracing::warn!(err = %e, "config patch failed; model_id not persisted");
    } else if let Err(e) = cfg_mgr.save_to_disk().await {
        tracing::warn!(err = %e, "config save failed; model_id not persisted across restarts");
    }
    ```
  - Start sidecar via `spawn_blocking` (required because `SidecarProcess::start` calls `std::thread::sleep` in its probe loop):
    ```rust
    let bin = dashboard::llama_server_bin_path();
    let model_file = models_dir.join("smollm2-135m-instruct-q4_k_m.gguf");
    match tokio::task::spawn_blocking(move || {
        crate::pii::tier3::SidecarProcess::start(&bin, &model_file, 16442, 30u64)
    }).await {
        Ok(Ok(proc)) => {
            if let Ok(mut guard) = sidecar.lock() { *guard = Some(proc); }
            tracing::info!(model_id = "smollm2-135m", "model downloaded and sidecar started");
        }
        Ok(Err(e)) => {
            tracing::warn!(err = %e, "sidecar failed to start; T3 disabled for this session");
            cfg.pii.tiers.slm = false;
        }
        Err(e) => {
            tracing::warn!(err = %e, "sidecar spawn_blocking panicked; T3 disabled for this session");
            cfg.pii.tiers.slm = false;
        }
    }
    ```
- [x] 4.7 In `cmd_start`, make `cfg` mutable at the top of the function body:
  ```rust
  let mut cfg = cfg; // shadow the parameter to allow mutation before Arc::new(cfg)
  ```
  Insert this immediately after the function signature's opening brace, before any other use of `cfg`.
- [x] 4.8 In `cmd_start`, call `ensure_slm_model` after creating `sidecar` (Task 3.3) and before `build_pii_ctx`. The call sequence must be:
  ```rust
  let sidecar: dashboard::SidecarHandle = Arc::new(Mutex::new(None));
  ensure_slm_model(&mut cfg, &cfg_mgr, &sidecar).await;
  let pii: PiiCtx = build_pii_ctx(&cfg, pii_flag);
  ```
- [x] 4.9 Write unit tests:
  - Test A: `ensure_slm_model` with `cfg.pii.tiers.slm = false` → function returns without touching `model_id` or sidecar. This test requires no network or binary.
  - Test B: `ensure_slm_model` with `tiers.slm = true`, `model_id = Some("smollm2-135m")`, and a real `.gguf` file written to a tmpdir → function returns without download; sidecar handle remains `None`. Use `tempfile::tempdir()` and write a zero-byte file named `smollm2-135m.gguf`.
  - Test C (download-error path): This path calls `download_with_bar` which makes a real HTTP request. Use the mock server pattern from `src/models/mod.rs` tests (lines ~418–440): spin up a `mockito::Server`, point the catalog URL to it, configure it to return a non-200, and verify `cfg.pii.tiers.slm` becomes `false`. If `mockito` is not already a dev-dependency, add it.
  - Do not attempt to test the sidecar-start path in unit tests — `SidecarProcess::start` requires the actual `llama-server` binary.
- [x] 4.10 Verify: `cargo test` passes.

## 5. Tray Path Reorder

- [x] 5.1 In `src/main.rs` (`run_tray_mode`), reorder the initialization sequence. The new order must be:
  1. `Config::load(...)` — already first, unchanged.
  2. Resolve `config_path` — unchanged.
  3. Add `let mut cfg = cfg_result;` to keep cfg mutable (if not already).
  4. Build tokio runtime `rt` — move this block (currently after `load_proxy_resources`) to immediately after step 2.
  5. Construct `cfg_mgr`: `let cfg_mgr = ConfigManager::new(cfg.clone(), Some(config_path.clone()));` — this requires `cfg` to be available (it is).
  6. Create `SidecarHandle`: `let sidecar: dashboard::SidecarHandle = Arc::new(Mutex::new(None));`
  7. Call `rt.block_on(ensure_slm_model(&mut cfg, &Arc::new(cfg_mgr.clone()), &sidecar));`
  8. Call `load_proxy_resources(&mut cfg, ...)` — moved to after step 7.
  9. Remaining tray setup and dashboard spawn — pass `sidecar` to `dashboard::run(...)` (completing Task 3.4).
- [x] 5.2 Note on logging order: `load_proxy_resources` initializes the tracing subscriber. Log lines from `ensure_slm_model` (step 7) will use the default subscriber (env-filter fallback). This is acceptable — the INFO/WARN messages from `ensure_slm_model` are important enough to show regardless; they will appear on stdout before the full logging setup. Do not attempt to reorder logging init further.
- [x] 5.3 Verify: `cargo build` succeeds. If a tray smoke-test is feasible in the CI environment, run it; otherwise confirm manually that `run_tray_mode` does not panic at startup.

## 6. PII-SETUP.md Documentation

- [x] 6.1 In `docs/PII-SETUP.md`, insert the following new section between the end of "## Performance Notes" (line 236) and "## Vault Persistence" (line 259). The exact insertion point is after the last line of the Performance Notes section content:
  ```markdown
  ## Choosing a Model

  When Tier 3 is enabled, privacyclaw uses a locally-running GGUF model. The
  four catalog models differ in size, memory usage, and detection quality:

  | Model ID        | Size   | RAM    | Latency (typical) | Quality |
  |-----------------|--------|--------|-------------------|---------|
  | smollm2-135m    | 90 MB  | 300 MB | ~100 ms/turn      | Good    |
  | qwen2.5-0.5b   | 400 MB | 800 MB | ~250 ms/turn      | Better  |
  | llama-3.2-1b    | 700 MB | 1.2 GB | ~500 ms/turn      | Better+ |
  | phi-3-mini-3.8b | 2.3 GB | 3.5 GB | ~1–2 s/turn       | Best    |

  **Recommendation:** Start with `smollm2-135m`. It is auto-downloaded on first
  run when T3 is enabled and no model is active. If you need higher accuracy for
  edge-case PII, upgrade to `qwen2.5-0.5b` with:

      privacyclaw models install qwen2.5-0.5b
      privacyclaw models activate qwen2.5-0.5b

  Latency figures assume Apple M-series hardware. Intel/AMD CPUs will be 2–3x
  slower; the timeout (`pii.slm.timeout_ms`, default 5000 ms) controls fail-open
  behavior if the model is too slow.
  ```
- [x] 6.2 Verify: the section appears between Performance Notes and Vault Persistence; table renders correctly.

## 7. OpenSpec: Add Requirement to add-macos-packaging

- [x] 7.1 In `openspec/changes/add-macos-packaging/specs/model-management/spec.md`, append the following three requirements within the existing `## ADDED Requirements` section (do not add a new `## ADDED Requirements` header — append after the last existing `#### Scenario:` block):

  ```markdown
  ### Requirement: Auto-Download on First Run

  When T3 is enabled in config (`pii.tiers.slm = true`) but no model is active
  (either `pii.slm.model_id` is unset or the model file is absent from
  `models_dir`), the system SHALL automatically download `smollm2-135m` before
  starting the sidecar. The download MUST block proxy startup until complete,
  display a progress bar, log an INFO message before downloading. On success the
  system MUST update `pii.slm.model_id` in config, persist to disk, and start
  the llama-server sidecar. On failure the system MUST log a WARN, disable T3
  for this session (fail-open), and continue startup without Tier 3.

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
  ```

- [x] 7.2 Run `openspec validate add-macos-packaging --strict` — confirm the updated change still passes.
- [x] 7.3 Run `openspec validate add-model-weight-mgmt --strict` — confirm this change passes.
