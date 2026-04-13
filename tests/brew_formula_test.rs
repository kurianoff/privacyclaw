/// §10.T1 + §10.T2: Validate that Homebrew formula and cask files exist and
/// contain the required structural elements. These tests run without `brew`
/// installed; they validate file content rather than running `brew audit`.

use std::path::PathBuf;

fn tap_root() -> PathBuf {
    // homebrew-privacyclaw/ lives at the repo root alongside Cargo.toml
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("homebrew-privacyclaw")
}

/// §10.T1: privacyclaw.rb formula exists and has required fields.
#[test]
fn formula_privacyclaw_rb_exists_and_valid() {
    let formula = tap_root().join("Formula/privacyclaw.rb");
    assert!(formula.exists(), "Formula/privacyclaw.rb must exist at {formula:?}");

    let content = std::fs::read_to_string(&formula).unwrap();
    assert!(content.contains("class Privacyclaw < Formula"), "must declare Formula class");
    assert!(content.contains("desc "), "must have desc field");
    assert!(content.contains("homepage "), "must have homepage field");
    assert!(content.contains("def install"), "must have install method");
    assert!(content.contains("service do"), "must have service block for brew services");
    assert!(content.contains("privacyclaw"), "must reference the privacyclaw binary");
    assert!(content.contains("test do"), "must have test block");

    // T4: source formula must not depend on llama.cpp
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("packaging/homebrew/privacyclaw.rb");
    let source_content = std::fs::read_to_string(&source).unwrap();
    assert!(
        !source_content.contains(r#"depends_on "llama.cpp""#),
        "source formula must not declare depends_on \"llama.cpp\""
    );
}

/// Tap formula must not have depends_on "llama.cpp" (bundled directly now).
#[test]
fn tap_formula_has_no_llama_cpp_dependency() {
    let formula = tap_root().join("Formula/privacyclaw.rb");
    let content = std::fs::read_to_string(&formula).unwrap();
    assert!(
        !content.contains(r#"depends_on "llama.cpp""#),
        "tap formula must not declare depends_on \"llama.cpp\""
    );
}

/// Tap formula must install llama-server directly from the tarball.
#[test]
fn tap_formula_installs_llama_server() {
    let formula = tap_root().join("Formula/privacyclaw.rb");
    let content = std::fs::read_to_string(&formula).unwrap();
    assert!(
        content.contains(r#"bin.install "llama-server""#),
        "tap formula must include bin.install \"llama-server\""
    );
}

/// §10.T2: privacyclaw-app.rb cask exists and has required fields.
#[test]
fn cask_privacyclaw_app_rb_exists_and_valid() {
    let cask = tap_root().join("Casks/privacyclaw-app.rb");
    assert!(cask.exists(), "Casks/privacyclaw-app.rb must exist at {cask:?}");

    let content = std::fs::read_to_string(&cask).unwrap();
    assert!(content.contains("cask \"privacyclaw-app\""), "must declare cask name");
    assert!(content.contains("app \"Privacyclaw.app\""), "must install Privacyclaw.app");
    assert!(content.contains("uninstall quit:"), "must have uninstall quit");
    assert!(content.contains("zap trash:"), "must have zap trash for cleanup");
    assert!(content.contains("com.privacyclaw.app"), "must reference bundle identifier");
}
