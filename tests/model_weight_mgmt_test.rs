/// Tests for the Model Weight Management feature (add-model-weight-mgmt).
///
/// Coverage areas:
///   A. models::catalog() — structure and completeness
///   B. models::is_downloaded() — gguf / onnx detection, missing paths
///   C. models::model_path() — gguf preferred over onnx
///   D. models::list_installed() — non-existent dir, empty dir, mixed extensions
///   E. models::DownloadTracker — full lifecycle: register/cancel/complete
///   F. models::get_api_entries — active flag, download_progress sentinel
///   G. models::download_with_bar — HTTP error → Err, 200 + body → file written,
///      checksum mismatch → Err, empty sha256 → no check, .tmp cleanup on failure
///
/// The tests in areas A–F are pure unit tests (no network).
/// Area G uses mockito to intercept HTTP calls.
use privacyclaw::models::{
    catalog, download_with_bar, get_api_entries, is_downloaded, list_installed, model_path,
    DownloadTracker, ModelInfo,
};
use std::path::PathBuf;
use tempfile::tempdir;

// ── A. Catalog structure ──────────────────────────────────────────────────────

#[test]
fn catalog_has_exactly_four_entries() {
    assert_eq!(catalog().len(), 4, "catalog must have exactly 4 entries");
}

#[test]
fn catalog_ids_are_unique() {
    let ids: Vec<_> = catalog().iter().map(|m| m.id).collect();
    let mut dedup = ids.clone();
    dedup.sort_unstable();
    dedup.dedup();
    assert_eq!(ids.len(), dedup.len(), "catalog IDs must be unique");
}

#[test]
fn catalog_required_ids_present() {
    let ids: Vec<_> = catalog().iter().map(|m| m.id).collect();
    for expected in &["smollm2-135m", "qwen2.5-0.5b", "llama-3.2-1b", "phi-3-mini-3.8b"] {
        assert!(ids.contains(expected), "catalog missing '{}'", expected);
    }
}

#[test]
fn catalog_no_empty_name_or_url() {
    for entry in catalog() {
        assert!(!entry.name.is_empty(), "model '{}' has empty name", entry.id);
        assert!(!entry.url.is_empty(), "model '{}' has empty URL", entry.id);
    }
}

#[test]
fn catalog_size_mb_positive() {
    for entry in catalog() {
        assert!(entry.size_mb > 0, "model '{}' has size_mb = 0", entry.id);
    }
}

#[test]
fn catalog_smollm2_is_index_zero() {
    // ensure_slm_model hard-codes catalog()[0] as the auto-download target
    assert_eq!(
        catalog()[0].id, "smollm2-135m",
        "smollm2-135m must be the first catalog entry (index 0)"
    );
}

// ── B. is_downloaded ──────────────────────────────────────────────────────────

#[test]
fn is_downloaded_returns_false_for_missing_dir() {
    let path = PathBuf::from("/nonexistent_dir_for_privacyclaw_tests");
    assert!(!is_downloaded(&path, "smollm2-135m"));
}

#[test]
fn is_downloaded_detects_gguf() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("smollm2-135m.gguf"), b"fake").unwrap();
    assert!(is_downloaded(dir, "smollm2-135m"));
}

#[test]
fn is_downloaded_detects_onnx() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("mymodel.onnx"), b"fake").unwrap();
    assert!(is_downloaded(dir, "mymodel"));
}

#[test]
fn is_downloaded_returns_false_when_no_matching_file() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    // unrelated file
    std::fs::write(dir.join("somethingelse.bin"), b"x").unwrap();
    assert!(!is_downloaded(dir, "smollm2-135m"));
}

#[test]
fn is_downloaded_requires_exact_id_match() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("smollm2-135m.gguf"), b"x").unwrap();
    // Similar but not equal ID
    assert!(!is_downloaded(dir, "smollm2-135"));
    assert!(!is_downloaded(dir, "smollm2-135m-extra"));
}

// ── C. model_path ─────────────────────────────────────────────────────────────

#[test]
fn model_path_prefers_gguf_over_onnx() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("mymodel.gguf"), b"g").unwrap();
    std::fs::write(dir.join("mymodel.onnx"), b"o").unwrap();
    let p = model_path(dir, "mymodel");
    assert!(
        p.extension().map(|e| e == "gguf").unwrap_or(false),
        "model_path should prefer .gguf when both exist"
    );
}

#[test]
fn model_path_falls_back_to_onnx() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    // Only .onnx present
    std::fs::write(dir.join("mymodel.onnx"), b"o").unwrap();
    let p = model_path(dir, "mymodel");
    assert!(
        p.extension().map(|e| e == "onnx").unwrap_or(false),
        "model_path should fall back to .onnx when .gguf absent"
    );
}

#[test]
fn model_path_returns_onnx_path_even_when_neither_exists() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    // Neither file exists → returns the .onnx path (caller checks existence)
    let p = model_path(dir, "ghost");
    assert!(p.to_string_lossy().ends_with("ghost.onnx"));
}

// ── D. list_installed ────────────────────────────────────────────────────────

#[test]
fn list_installed_returns_empty_for_nonexistent_dir() {
    let path = PathBuf::from("/nonexistent_dir_for_privacyclaw_list_tests");
    let result = list_installed(&path).unwrap();
    assert!(result.is_empty());
}

#[test]
fn list_installed_returns_empty_for_empty_dir() {
    let tmp = tempdir().unwrap();
    let result = list_installed(tmp.path()).unwrap();
    assert!(result.is_empty());
}

#[test]
fn list_installed_finds_gguf_and_onnx() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("smollm2-135m.gguf"), b"g").unwrap();
    std::fs::write(dir.join("other.onnx"), b"o").unwrap();
    let result = list_installed(dir).unwrap();
    assert_eq!(result.len(), 2);
    let names: Vec<_> = result.iter().map(|m| m.name.as_str()).collect();
    assert!(names.contains(&"smollm2-135m"));
    assert!(names.contains(&"other"));
}

#[test]
fn list_installed_ignores_non_model_extensions() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    std::fs::write(dir.join("readme.txt"), b"x").unwrap();
    std::fs::write(dir.join("weights.bin"), b"x").unwrap();
    std::fs::write(dir.join("model.gguf"), b"x").unwrap();
    let result = list_installed(dir).unwrap();
    assert_eq!(result.len(), 1, "only .gguf/.onnx files should be listed");
    assert_eq!(result[0].name, "model");
}

#[test]
fn list_installed_reports_correct_size() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    let content = b"hello world";
    std::fs::write(dir.join("m.gguf"), content).unwrap();
    let result = list_installed(dir).unwrap();
    assert_eq!(result[0].size_bytes, content.len() as u64);
}

// ── E. DownloadTracker lifecycle ──────────────────────────────────────────────

#[test]
fn download_tracker_is_not_downloading_initially() {
    let t = DownloadTracker::new();
    assert!(!t.is_downloading("any-model"));
}

#[test]
fn download_tracker_register_marks_downloading() {
    let t = DownloadTracker::new();
    let _rx = t.register("m1");
    assert!(t.is_downloading("m1"));
}

#[test]
fn download_tracker_cancel_removes_entry_and_closes_channel() {
    let t = DownloadTracker::new();
    let mut rx = t.register("m1");
    t.cancel("m1");
    assert!(!t.is_downloading("m1"));
    // Sender was dropped; receiver gets closed error
    assert!(rx.try_recv().is_err());
}

#[test]
fn download_tracker_complete_removes_entry() {
    let t = DownloadTracker::new();
    let _rx = t.register("m2");
    t.complete("m2");
    assert!(!t.is_downloading("m2"));
}

#[test]
fn download_tracker_cancel_noop_for_unknown_id() {
    let t = DownloadTracker::new();
    // should not panic
    t.cancel("nonexistent");
}

#[test]
fn download_tracker_multiple_models_independent() {
    let t = DownloadTracker::new();
    let _r1 = t.register("a");
    let _r2 = t.register("b");
    t.cancel("a");
    assert!(!t.is_downloading("a"));
    assert!(t.is_downloading("b"), "cancelling 'a' must not affect 'b'");
}

#[test]
fn download_tracker_re_register_after_complete() {
    let t = DownloadTracker::new();
    let _r = t.register("m");
    t.complete("m");
    // re-register the same model (e.g. retry download)
    let _r2 = t.register("m");
    assert!(t.is_downloading("m"));
}

// ── F. get_api_entries ────────────────────────────────────────────────────────

#[test]
fn get_api_entries_count_matches_catalog() {
    let dir = PathBuf::from("/nonexistent_dir_for_api_entries_test");
    let entries = get_api_entries(&dir, None, None);
    assert_eq!(entries.len(), catalog().len());
}

#[test]
fn get_api_entries_none_active_when_no_active_id() {
    let dir = PathBuf::from("/nonexistent");
    let entries = get_api_entries(&dir, None, None);
    assert!(entries.iter().all(|e| !e.active));
}

#[test]
fn get_api_entries_marks_correct_entry_active() {
    let dir = PathBuf::from("/nonexistent");
    let entries = get_api_entries(&dir, Some("qwen2.5-0.5b"), None);
    let active: Vec<_> = entries.iter().filter(|e| e.active).collect();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, "qwen2.5-0.5b");
}

#[test]
fn get_api_entries_unknown_active_id_marks_none() {
    let dir = PathBuf::from("/nonexistent");
    let entries = get_api_entries(&dir, Some("does-not-exist"), None);
    assert!(entries.iter().all(|e| !e.active));
}

#[test]
fn get_api_entries_marks_downloaded_when_gguf_exists() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    let id = catalog()[0].id;
    std::fs::write(dir.join(format!("{}.gguf", id)), b"x").unwrap();
    let entries = get_api_entries(dir, None, None);
    let first = entries.iter().find(|e| e.id == id).unwrap();
    assert!(first.downloaded);
    // others not downloaded
    assert!(entries.iter().filter(|e| e.id != id).all(|e| !e.downloaded));
}

#[test]
fn get_api_entries_in_progress_sentinel_minus_one() {
    let dir = PathBuf::from("/nonexistent");
    let tracker = DownloadTracker::new();
    let _rx = tracker.register("smollm2-135m");
    let entries = get_api_entries(&dir, None, Some(&tracker));
    let entry = entries.iter().find(|e| e.id == "smollm2-135m").unwrap();
    // not downloaded, in-progress => progress = Some(-1.0)
    assert_eq!(
        entry.download_progress,
        Some(-1.0),
        "in-progress not-downloaded entry should report -1.0"
    );
}

#[test]
fn get_api_entries_downloaded_entry_has_no_progress() {
    let tmp = tempdir().unwrap();
    let dir = tmp.path();
    let id = catalog()[0].id;
    std::fs::write(dir.join(format!("{}.gguf", id)), b"x").unwrap();
    let tracker = DownloadTracker::new();
    let _rx = tracker.register(id); // register even though file exists
    let entries = get_api_entries(dir, None, Some(&tracker));
    let entry = entries.iter().find(|e| e.id == id).unwrap();
    // already downloaded → no progress
    assert_eq!(entry.download_progress, None);
}

// ── G. download_with_bar ─────────────────────────────────────────────────────

#[tokio::test]
async fn download_with_bar_returns_err_on_http_503() {
    let tmp = tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/test.gguf")
        .with_status(503)
        .with_body("unavailable")
        .create_async()
        .await;
    let url: &'static str =
        Box::leak(format!("{}/test.gguf", server.url()).into_boxed_str());
    let info: &'static ModelInfo = Box::leak(Box::new(ModelInfo {
        id: "test-503",
        name: "Test 503",
        description: "test",
        url,
        sha256: "",
        size_mb: 1,
    }));
    let result = download_with_bar(info, tmp.path()).await;
    assert!(result.is_err(), "download_with_bar must return Err on HTTP 503");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("503") || err_msg.contains("download failed"),
        "error message should mention failure: {}", err_msg
    );
}

#[tokio::test]
async fn download_with_bar_returns_err_on_http_404() {
    let tmp = tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/missing.gguf")
        .with_status(404)
        .with_body("not found")
        .create_async()
        .await;
    let url: &'static str =
        Box::leak(format!("{}/missing.gguf", server.url()).into_boxed_str());
    let info: &'static ModelInfo = Box::leak(Box::new(ModelInfo {
        id: "test-404",
        name: "Test 404",
        description: "test",
        url,
        sha256: "",
        size_mb: 1,
    }));
    let result = download_with_bar(info, tmp.path()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn download_with_bar_writes_file_on_success() {
    let tmp = tempdir().unwrap();
    let body = b"fake gguf content for testing";
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/real.gguf")
        .with_status(200)
        .with_body(body.as_slice())
        .create_async()
        .await;
    let url: &'static str =
        Box::leak(format!("{}/real.gguf", server.url()).into_boxed_str());
    let info: &'static ModelInfo = Box::leak(Box::new(ModelInfo {
        id: "test-ok",
        name: "Test OK",
        description: "test",
        url,
        sha256: "",
        size_mb: 1,
    }));
    let result = download_with_bar(info, tmp.path()).await;
    assert!(result.is_ok(), "download_with_bar should succeed on 200: {:?}", result);
    // File should exist with the right content
    let dest = tmp.path().join("real.gguf");
    assert!(dest.exists(), "downloaded file must exist");
    let written = std::fs::read(&dest).unwrap();
    assert_eq!(written, body, "file content must match server response");
}

#[tokio::test]
async fn download_with_bar_no_tmp_file_on_failure() {
    // After a 503 error the .tmp staging file must be cleaned up.
    let tmp = tempdir().unwrap();
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/staged.gguf")
        .with_status(503)
        .with_body("")
        .create_async()
        .await;
    let url: &'static str =
        Box::leak(format!("{}/staged.gguf", server.url()).into_boxed_str());
    let info: &'static ModelInfo = Box::leak(Box::new(ModelInfo {
        id: "test-tmp-cleanup",
        name: "Test TMP Cleanup",
        description: "test",
        url,
        sha256: "",
        size_mb: 1,
    }));
    let _ = download_with_bar(info, tmp.path()).await;
    let tmp_file = tmp.path().join("staged.tmp");
    assert!(
        !tmp_file.exists(),
        ".tmp staging file must be removed after failure"
    );
}

#[tokio::test]
async fn download_with_bar_checksum_mismatch_returns_err() {
    let tmp = tempdir().unwrap();
    let body = b"data that won't match the hash";
    let wrong_hash = "0000000000000000000000000000000000000000000000000000000000000000";
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/hashcheck.gguf")
        .with_status(200)
        .with_body(body.as_slice())
        .create_async()
        .await;
    let url: &'static str =
        Box::leak(format!("{}/hashcheck.gguf", server.url()).into_boxed_str());
    let info: &'static ModelInfo = Box::leak(Box::new(ModelInfo {
        id: "test-hash",
        name: "Test Hash",
        description: "test",
        url,
        sha256: wrong_hash,
        size_mb: 1,
    }));
    let result = download_with_bar(info, tmp.path()).await;
    assert!(result.is_err(), "download_with_bar must return Err on checksum mismatch");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("checksum") || err_msg.contains("mismatch"),
        "error should mention checksum: {}", err_msg
    );
}

#[tokio::test]
async fn download_with_bar_empty_sha256_skips_checksum() {
    // When sha256 == "" verification is skipped and Ok is returned.
    let tmp = tempdir().unwrap();
    let body = b"arbitrary data no checksum required";
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/nocheck.gguf")
        .with_status(200)
        .with_body(body.as_slice())
        .create_async()
        .await;
    let url: &'static str =
        Box::leak(format!("{}/nocheck.gguf", server.url()).into_boxed_str());
    let info: &'static ModelInfo = Box::leak(Box::new(ModelInfo {
        id: "test-nocheck",
        name: "Test NoCheck",
        description: "test",
        url,
        sha256: "",
        size_mb: 1,
    }));
    let result = download_with_bar(info, tmp.path()).await;
    assert!(result.is_ok(), "empty sha256 should skip verification: {:?}", result);
}

#[tokio::test]
async fn download_with_bar_correct_sha256_passes() {
    // Compute the expected SHA-256 of a known payload and supply it.
    use sha2::{Digest, Sha256};
    let tmp = tempdir().unwrap();
    let body = b"exact hash payload";
    let hash = format!("{:x}", Sha256::digest(body));
    let mut server = mockito::Server::new_async().await;
    let _m = server
        .mock("GET", "/goodhash.gguf")
        .with_status(200)
        .with_body(body.as_slice())
        .create_async()
        .await;
    let url: &'static str =
        Box::leak(format!("{}/goodhash.gguf", server.url()).into_boxed_str());
    let hash_static: &'static str = Box::leak(hash.into_boxed_str());
    let info: &'static ModelInfo = Box::leak(Box::new(ModelInfo {
        id: "test-goodhash",
        name: "Test GoodHash",
        description: "test",
        url,
        sha256: hash_static,
        size_mb: 1,
    }));
    let result = download_with_bar(info, tmp.path()).await;
    assert!(result.is_ok(), "correct SHA-256 should pass verification: {:?}", result);
}
