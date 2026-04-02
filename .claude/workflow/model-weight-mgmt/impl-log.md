# Implementation Log — Model Weight Management (add-model-weight-mgmt)

Feature: Model Weight Management — auto-download on first run for T3
Branch: feature/model-weight-mgmt
Phase: Development (Phase 3)

---

## Task Status Summary

| Task | Title | Status |
|------|-------|--------|
| 1 | Catalog SHA-256 Values | deferred |
| 2 | Extract llama-server Binary Path Helper | complete |
| 3 | Hoist SidecarHandle | complete |
| 4 | ensure_slm_model Function | complete |
| 5 | Tray Path Reorder | complete |
| 6 | PII-SETUP.md Documentation | complete |
| 7 | OpenSpec: Add Requirement to add-macos-packaging | complete |

---

### Task 1: Catalog SHA-256 Values
Status: deferred
Branch: feature/model-weight-mgmt (direct commit)
Done:
  - none — requires downloading ~3.5 GB of GGUF files from HuggingFace
Issues found:
  - SHA-256 computation requires network access to large files not available in sandbox
  - `download_with_bar` already skips checksum verification when `sha256 == ""`; safe to leave empty
  - Fields remain as empty string `""` in CATALOG; no behavioral regression
Contrarian verdict: deferred (user acknowledgement required for large download)

---

### Task 2: Extract llama-server Binary Path Helper
Status: complete
Branch: feature/model-weight-mgmt (already present in worktree)
Done:
  - `pub(crate) fn llama_server_bin_path()` already existed in src/dashboard/mod.rs (line 18)
  - Already used at line 479 replacing the hardcoded path
  - `pub(crate) type SidecarHandle` already `pub(crate)` at line 15
Issues found:
  - none — tasks 2 and 3.1 were pre-completed in the feature worktree
Contrarian verdict: approved

---

### Task 3: Hoist SidecarHandle — dashboard::run() Signature Change
Status: complete
Branch: feature/model-weight-mgmt
Done:
  - dashboard::run() signature extended with `sidecar: SidecarHandle` as last parameter
  - Internal `let sidecar: SidecarHandle = Arc::new(Mutex::new(None))` removed from run() body
  - cmd_start creates `sidecar: dashboard::SidecarHandle` and passes to dashboard::run()
  - run_tray_mode updated to create and pass sidecar (combined with Task 5)
Issues found:
  - none
Contrarian verdict: approved

---

### Task 4: ensure_slm_model Function
Status: complete
Branch: feature/model-weight-mgmt
Done:
  - `async fn ensure_slm_model(cfg: &mut Config, cfg_mgr: &Arc<ConfigManager>, sidecar: &dashboard::SidecarHandle)` added to src/main.rs
  - Early-return guard: tiers.slm=false → return; model_id set + file present → return
  - Downloads smollm2-135m via download_with_bar on missing model
  - Failure branch: disables T3 for session, returns
  - Success branch: sets model_id, persists to config, starts sidecar via spawn_blocking
  - cmd_start: `let mut cfg = cfg` shadow; ensure_slm_model called before build_pii_ctx
  - mockito added as dev-dependency
  - Test A (slm disabled): passes
  - Test B (model present): passes
  - Test C (download 503): passes via Box::leak for &'static str URL
Issues found:
  - Static lifetime of ModelInfo.url made Test C non-trivial; resolved with Box::leak pattern
  - Unused `make_cfg_slm_enabled_no_model` helper removed from final test module
Contrarian verdict: approved

---

### Task 5: Tray Path Reorder
Status: complete
Branch: feature/model-weight-mgmt
Done:
  - run_tray_mode reordered: rt built immediately after config_path
  - cfg_mgr = ConfigManager::new(cfg.clone(), Some(config_path.clone())) before load_proxy_resources
  - sidecar created; rt.block_on(ensure_slm_model(...)) called before load_proxy_resources
  - load_proxy_resources called after ensure_slm_model (logging init after auto-download)
  - sidecar passed to dashboard::run() (completing Task 3.4)
Issues found:
  - Original cfg_mgr was ConfigManager::new() returning Arc<Self> directly; extra Arc::new() wrap
    removed before compile check
Contrarian verdict: approved

---

### Task 6: PII-SETUP.md Documentation
Status: complete
Branch: feature/model-weight-mgmt
Done:
  - "## Choosing a Model" section inserted between Performance Notes and Vault Persistence
  - Table with 4 catalog entries (ID, Size, RAM, Latency, Quality)
  - Recommendation text with upgrade commands as fenced code block
  - Markdownlint warnings resolved: table column alignment fixed, indented block → fenced
Issues found:
  - Initial table used `qwen2.5-0.5b` with one fewer space than header column — fixed
  - Indented code block flagged by MD046 — changed to fenced ```sh block
Contrarian verdict: approved

---

### Task 7: OpenSpec: Add Requirement to add-macos-packaging
Status: complete
Branch: feature/model-weight-mgmt
Done:
  - "Auto-Download on First Run" requirement appended to add-macos-packaging/specs/model-management/spec.md
  - Three scenarios added: T3 enabled no model, auto-download fails, model already present
  - Appended after last existing Scenario block (Delete active model rejected)
Issues found:
  - none
Contrarian verdict: approved
