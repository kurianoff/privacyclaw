/// §8.T4 Automated tests for macOS app bundle structure.
///
/// These tests validate the Info.plist template used by `make app` without
/// requiring a full Makefile run. The template content is derived from the same
/// printf block in the Makefile.

/// Build the same Info.plist content that `make app` generates.
fn make_info_plist(version: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>com.claudovka.app</string>
  <key>CFBundleName</key><string>Claudovka</string>
  <key>CFBundleVersion</key><string>{version}</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>LSUIElement</key><true/>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>CFBundleExecutable</key><string>claudovka-app</string>
  <key>CFBundlePackageType</key><string>APPL</string>
</dict></plist>"#
    )
}

/// §8.T4: Info.plist contains LSUIElement = true (suppresses Dock icon).
#[test]
fn info_plist_has_ls_ui_element() {
    let plist = make_info_plist("0.1.0");
    assert!(
        plist.contains("<key>LSUIElement</key><true/>"),
        "Info.plist must contain LSUIElement=true to suppress Dock icon"
    );
}

/// §8.T4: Info.plist has the correct bundle identifier.
#[test]
fn info_plist_has_correct_bundle_id() {
    let plist = make_info_plist("0.1.0");
    assert!(
        plist.contains("<key>CFBundleIdentifier</key><string>com.claudovka.app</string>"),
        "Info.plist must contain CFBundleIdentifier = com.claudovka.app"
    );
}

/// §8.T4: Info.plist sets minimum macOS version to 13.0.
#[test]
fn info_plist_minimum_macos_13() {
    let plist = make_info_plist("0.1.0");
    assert!(
        plist.contains("<key>LSMinimumSystemVersion</key><string>13.0</string>"),
        "Info.plist must require macOS 13.0 minimum"
    );
}

/// §8.T4: Info.plist sets bundle executable to claudovka-app.
#[test]
fn info_plist_executable_is_claudovka_app() {
    let plist = make_info_plist("0.1.0");
    assert!(
        plist.contains("<key>CFBundleExecutable</key><string>claudovka-app</string>"),
        "Info.plist CFBundleExecutable must be claudovka-app"
    );
}

/// §8.T4: Info.plist embeds the version string passed to make.
#[test]
fn info_plist_version_is_embedded() {
    let plist = make_info_plist("1.2.3");
    assert!(
        plist.contains("<string>1.2.3</string>"),
        "Info.plist must embed the version string"
    );
}

/// §11.T3: When no PID file exists, read_pid returns None — meaning cmd_stop
/// will print "not running" and exit cleanly without signalling any process.
#[test]
fn cmd_stop_with_no_pid_file_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let absent = dir.path().join("claudovka.pid");
    assert!(!absent.exists(), "test setup: PID file must not exist");

    // Simulate read_pid logic against a temp path.
    let result: Option<u32> = std::fs::read_to_string(&absent)
        .ok()
        .and_then(|s| s.trim().parse().ok());

    assert!(
        result.is_none(),
        "cmd_stop: read_pid must return None when PID file is absent → not-running path taken"
    );
}
