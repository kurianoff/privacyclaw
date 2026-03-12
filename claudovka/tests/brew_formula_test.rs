/// §10.T1 + §10.T2: Validate that Homebrew formula and cask files exist and
/// contain the required structural elements. These tests run without `brew`
/// installed; they validate file content rather than running `brew audit`.

use std::path::PathBuf;

fn tap_root() -> PathBuf {
    // From claudovka/ we go up one level to find homebrew-claudovka/
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("homebrew-claudovka")
}

/// §10.T1: claudovka.rb formula exists and has required fields.
#[test]
fn formula_claudovka_rb_exists_and_valid() {
    let formula = tap_root().join("Formula/claudovka.rb");
    assert!(formula.exists(), "Formula/claudovka.rb must exist at {formula:?}");

    let content = std::fs::read_to_string(&formula).unwrap();
    assert!(content.contains("class Claudovka < Formula"), "must declare Formula class");
    assert!(content.contains("desc "), "must have desc field");
    assert!(content.contains("homepage "), "must have homepage field");
    assert!(content.contains("def install"), "must have install method");
    assert!(content.contains("service do"), "must have service block for brew services");
    assert!(content.contains("claudovka"), "must reference the claudovka binary");
    assert!(content.contains("test do"), "must have test block");
}

/// §10.T2: claudovka-app.rb cask exists and has required fields.
#[test]
fn cask_claudovka_app_rb_exists_and_valid() {
    let cask = tap_root().join("Casks/claudovka-app.rb");
    assert!(cask.exists(), "Casks/claudovka-app.rb must exist at {cask:?}");

    let content = std::fs::read_to_string(&cask).unwrap();
    assert!(content.contains("cask \"claudovka-app\""), "must declare cask name");
    assert!(content.contains("app \"Claudovka.app\""), "must install Claudovka.app");
    assert!(content.contains("uninstall quit:"), "must have uninstall quit");
    assert!(content.contains("zap trash:"), "must have zap trash for cleanup");
    assert!(content.contains("com.claudovka.app"), "must reference bundle identifier");
}
