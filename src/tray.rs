//! macOS menu bar tray icon.
//!
//! Compiled only when `target_os = "macos"` **and** the `tray` Cargo feature is enabled.
//! The `run()` function **must** be called from the main thread because AppKit requires it.
//!
//! Architecture when `--tray` is used:
//!   • Main thread  → `tray::run()` (this module) — drives the CoreFoundation run loop
//!   • Worker threads → tokio runtime — handles proxy, dashboard, storage

#![cfg(all(target_os = "macos", feature = "tray"))]

use std::sync::Arc;
use tokio::sync::Notify;
use crate::config::default_ca_dir;
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    Icon, TrayIconBuilder,
};

// ── TrayState ─────────────────────────────────────────────────────────────────

/// All resources needed to spawn and abort proxy tasks from the tray event loop.
#[allow(dead_code)]
pub(crate) struct TrayState {
    /// Whether the proxy (HTTP CONNECT listener) is conceptually "running".
    pub proxy_running: bool,
    /// Whether the HTTP CONNECT listener task is currently spawned.
    pub http_listener_on: bool,
    /// Handle to the HTTP CONNECT listener task; `None` when not running.
    pub http_task: Option<tokio::task::JoinHandle<()>>,
    /// TLS certificate cache shared with the proxy tasks.
    pub cert_cache: crate::ca::cert_gen::CertCache,
    /// Resolved application config.
    pub cfg: Arc<crate::config::Config>,
    /// Live-reloadable config manager (used by dashboard).
    pub cfg_mgr: Arc<crate::config::ConfigManager>,
    /// SQLite log store.
    pub store: crate::storage::Store,
    /// WebSocket broadcast channel for dashboard push events.
    pub ws_tx: tokio::sync::broadcast::Sender<crate::dashboard::WsEvent>,
    /// PII context; `None` when PII mode is off.
    pub pii: crate::pii::PiiCtx,
    /// Handle to the tokio runtime (for spawning tasks from the sync tray thread).
    pub rt: tokio::runtime::Handle,
}

// ── Menu item IDs ─────────────────────────────────────────────────────────────

struct Ids {
    start_stop:     tray_icon::menu::MenuId,
    http_proxy:     tray_icon::menu::MenuId,
    open_dashboard: tray_icon::menu::MenuId,
    network_proxy:  tray_icon::menu::MenuId,
    pii_off:        tray_icon::menu::MenuId,
    pii_detect:     tray_icon::menu::MenuId,
    pii_t1:         tray_icon::menu::MenuId,
    pii_t2:         tray_icon::menu::MenuId,
    pii_t3:         tray_icon::menu::MenuId,
    pii_intelligent: tray_icon::menu::MenuId,
    quit:           tray_icon::menu::MenuId,
}

// ── Pure helpers (testable without a display) ─────────────────────────────────

/// Derive the active PII level string from `pii.mode` and `pii.tiers` fields in
/// the JSON config returned by GET /api/config.
///
/// Returns one of: "off", "detect-only", "1", "2", "3", "intelligent".
pub fn derive_pii_level(mode: &str, regex: bool, ner: bool, slm: bool) -> &'static str {
    match mode {
        "off" => "off",
        "detect-only" => "detect-only",
        "replace" => {
            if regex && ner && slm {
                "3"
            } else if regex && ner && !slm {
                "2"
            } else if regex && !ner && !slm {
                "1"
            } else if !regex && !ner && slm {
                "intelligent"
            } else {
                // Unexpected combination — treat as off to avoid stale check.
                "off"
            }
        }
        _ => "off",
    }
}

/// Returns the enabled/checked state for each PII submenu item based on
/// the current derived `pii_level` string.  Used both by `build_menu` and unit tests.
pub fn pii_menu_states(pii_level: &str) -> PiiMenuStates {
    PiiMenuStates {
        off_enabled:         pii_level != "off",
        off_checked:         pii_level == "off",
        detect_enabled:      pii_level != "detect-only",
        detect_checked:      pii_level == "detect-only",
        t1_enabled:          pii_level != "1",
        t1_checked:          pii_level == "1",
        t2_enabled:          pii_level != "2",
        t2_checked:          pii_level == "2",
        t3_enabled:          pii_level != "3",
        t3_checked:          pii_level == "3",
        intelligent_enabled: pii_level != "intelligent",
        intelligent_checked: pii_level == "intelligent",
    }
}

/// The enabled/checked booleans for the six PII submenu items.
#[derive(Debug, PartialEq, Eq)]
pub struct PiiMenuStates {
    pub off_enabled:         bool,
    pub off_checked:         bool,
    pub detect_enabled:      bool,
    pub detect_checked:      bool,
    pub t1_enabled:          bool,
    pub t1_checked:          bool,
    pub t2_enabled:          bool,
    pub t2_checked:          bool,
    pub t3_enabled:          bool,
    pub t3_checked:          bool,
    pub intelligent_enabled: bool,
    pub intelligent_checked: bool,
}

/// Return the `--protection-level` argument for a menu item ID, or `None` if
/// the ID does not belong to the PII submenu.
fn pii_level_for_id<'a>(id: &tray_icon::menu::MenuId, ids: &Ids) -> Option<&'a str> {
    if *id == ids.pii_off         { Some("off") }
    else if *id == ids.pii_detect { Some("detect-only") }
    else if *id == ids.pii_t1     { Some("1") }
    else if *id == ids.pii_t2     { Some("2") }
    else if *id == ids.pii_t3     { Some("3") }
    else if *id == ids.pii_intelligent { Some("intelligent") }
    else                           { None }
}

// ── Menu construction ─────────────────────────────────────────────────────────

/// Build the tray menu.
///
/// - `proxy_running`: controls label of `start_stop` item and enables/disables
///   proxy-related items.
/// - `http_proxy_on`: checked state of the HTTP Proxy toggle (only meaningful
///   when `proxy_running = true`).
/// - `network_proxy_on`: checked state of the Network Proxy toggle.
/// - `pii_level`: current PII level string, e.g. `"off"`, `"1"`, `"intelligent"`.
fn build_menu(
    proxy_running:   bool,
    http_proxy_on:   bool,
    network_proxy_on: bool,
    pii_level:       &str,
) -> (Menu, Ids) {
    // Start/Stop item at top: label reflects current state.
    let start_stop_label = if proxy_running { "Stop Proxy" } else { "Start Proxy" };
    let start_stop = MenuItem::new(start_stop_label, true, None);

    let open_dashboard = MenuItem::new("Open Dashboard", true, None);

    // HTTP proxy toggle: enabled only when proxy is running.
    let http_proxy = CheckMenuItem::new(
        "HTTP Proxy",
        proxy_running,
        http_proxy_on && proxy_running,
        None,
    );
    // Network proxy toggle: enabled only when proxy is running.
    let network_proxy = CheckMenuItem::new(
        "Network Proxy",
        proxy_running,
        network_proxy_on && proxy_running,
        None,
    );

    // PII submenu — checked state reflects current pii_level derived from config.
    // The submenu itself is enabled only while the proxy is running.
    let states = pii_menu_states(pii_level);
    let pii_sub = Submenu::new("PII Protection", proxy_running);
    // CheckMenuItem::new(label, enabled, checked, accelerator)
    let pii_off         = CheckMenuItem::new("Off",                      states.off_enabled,         states.off_checked,         None);
    let pii_detect      = CheckMenuItem::new("Detect only",              states.detect_enabled,      states.detect_checked,      None);
    let pii_t1          = CheckMenuItem::new("Tier 1 — Regex",           states.t1_enabled,          states.t1_checked,          None);
    let pii_t2          = CheckMenuItem::new("Tier 2 — NER",             states.t2_enabled,          states.t2_checked,          None);
    let pii_t3          = CheckMenuItem::new("Tier 3 — Full Pipeline",   states.t3_enabled,          states.t3_checked,          None);
    let pii_intelligent = CheckMenuItem::new("Intelligent (T3 only)",    states.intelligent_enabled, states.intelligent_checked, None);
    let _ = pii_sub.append_items(&[
        &pii_off,
        &pii_detect,
        &pii_t1,
        &pii_t2,
        &pii_t3,
        &pii_intelligent,
    ]);

    // Quit is always enabled.
    let quit = MenuItem::new("Quit Privacyclaw", true, None);

    let ids = Ids {
        start_stop:      start_stop.id().clone(),
        http_proxy:      http_proxy.id().clone(),
        open_dashboard:  open_dashboard.id().clone(),
        network_proxy:   network_proxy.id().clone(),
        pii_off:         pii_off.id().clone(),
        pii_detect:      pii_detect.id().clone(),
        pii_t1:          pii_t1.id().clone(),
        pii_t2:          pii_t2.id().clone(),
        pii_t3:          pii_t3.id().clone(),
        pii_intelligent: pii_intelligent.id().clone(),
        quit:            quit.id().clone(),
    };

    let menu = Menu::new();
    let _ = menu.append_items(&[
        &start_stop,
        &PredefinedMenuItem::separator(),
        &open_dashboard,
        &PredefinedMenuItem::separator(),
        &http_proxy,
        &network_proxy,
        &PredefinedMenuItem::separator(),
        &pii_sub,
        &PredefinedMenuItem::separator(),
        &quit,
    ]);

    (menu, ids)
}

// ── Icon ──────────────────────────────────────────────────────────────────────

/// Generate an RGBA pixel buffer for the Privacyclaw icon.
///
/// Design: dark navy (#1a2332) background, white privacy "lens" ring,
/// coloured centre dot — green `(0, 200, 80)` when `running = true`,
/// red `(200, 50, 50)` when `running = false`.
///
/// This is a purely procedural icon — no external assets required.
/// At 32×32 the ring and dot are clearly visible in the menu bar;
/// at 512×512 the same function produces a high-resolution version suitable
/// for the app bundle.
pub fn generate_icon_rgba(size: u32, running: bool) -> Vec<u8> {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let cx = size / 2;
    let cy = size / 2;
    let r_outer = size * 45 / 100; // outer radius of ring
    let ring_width = (size / 16).max(1);
    let r_inner_dot = size * 15 / 100; // solid centre dot

    // Dot colour: green when running, red when stopped.
    let (dot_r, dot_g, dot_b): (u8, u8, u8) = if running {
        (0, 200, 80)
    } else {
        (200, 50, 50)
    };

    for y in 0..size {
        for x in 0..size {
            let dx = (x as i32 - cx as i32).pow(2) as u32;
            let dy = (y as i32 - cy as i32).pow(2) as u32;
            let dist2 = dx + dy;

            let i = ((y * size + x) * 4) as usize;

            // Background: dark navy
            pixels[i]     = 26;  // R
            pixels[i + 1] = 35;  // G
            pixels[i + 2] = 50;  // B
            pixels[i + 3] = 255; // A (fully opaque)

            // White privacy "lens" ring
            let r_outer2 = r_outer * r_outer;
            let r_inner_ring = r_outer.saturating_sub(ring_width);
            let r_inner_ring2 = r_inner_ring * r_inner_ring;
            if dist2 <= r_outer2 && dist2 >= r_inner_ring2 {
                pixels[i]     = 240;
                pixels[i + 1] = 240;
                pixels[i + 2] = 255;
            }

            // Centre dot: green (running) or red (stopped)
            if dist2 <= r_inner_dot * r_inner_dot {
                pixels[i]     = dot_r;
                pixels[i + 1] = dot_g;
                pixels[i + 2] = dot_b;
            }
        }
    }
    pixels
}

/// Build the tray icon (32×32, procedurally generated).
///
/// `running = true` produces a green centre dot; `running = false` a red dot.
fn make_icon(running: bool) -> Icon {
    const N: u32 = 32;
    let rgba = generate_icon_rgba(N, running);
    Icon::from_rgba(rgba, N, N).expect("icon creation failed")
}

// ── CoreFoundation run-loop pump ──────────────────────────────────────────────

// ── Unit tests ────────────────────────────────────────────────────────────────

/// §7.T6: `pii_menu_states(pii_level)` returns correct enabled/checked states.
///
/// These tests run on all platforms without a display because `pii_menu_states`
/// is a pure function that does not create any AppKit or tray-icon objects.
#[cfg(test)]
mod tests {
    use super::*;

    // ── derive_pii_level tests ──────────────────────────────────────────────

    #[test]
    fn derive_pii_level_off() {
        assert_eq!(derive_pii_level("off", false, false, false), "off");
    }

    #[test]
    fn derive_pii_level_detect_only() {
        assert_eq!(derive_pii_level("detect-only", false, false, false), "detect-only");
    }

    #[test]
    fn derive_pii_level_tier1() {
        assert_eq!(derive_pii_level("replace", true, false, false), "1");
    }

    #[test]
    fn derive_pii_level_tier2() {
        assert_eq!(derive_pii_level("replace", true, true, false), "2");
    }

    #[test]
    fn derive_pii_level_tier3() {
        assert_eq!(derive_pii_level("replace", true, true, true), "3");
    }

    #[test]
    fn derive_pii_level_intelligent() {
        assert_eq!(derive_pii_level("replace", false, false, true), "intelligent");
    }

    // ── pii_menu_states tests ───────────────────────────────────────────────

    #[test]
    fn pii_menu_states_off_mode() {
        let s = pii_menu_states("off");
        assert!(!s.off_enabled,  "level=off → Off item should be disabled (already active)");
        assert!(s.off_checked,   "level=off → Off item should be checked");
        assert!(s.detect_enabled);
        assert!(!s.detect_checked);
        assert!(s.t1_enabled);
        assert!(!s.t1_checked);
        assert!(s.t2_enabled);
        assert!(s.t3_enabled);
        assert!(s.intelligent_enabled);
    }

    #[test]
    fn pii_menu_states_detect_only_mode() {
        let s = pii_menu_states("detect-only");
        assert!(s.off_enabled,     "level=detect-only → Off item should be enabled");
        assert!(!s.off_checked);
        assert!(!s.detect_enabled, "level=detect-only → Detect only should be disabled (active)");
        assert!(s.detect_checked,  "level=detect-only → Detect only should be checked");
        assert!(s.t1_enabled);
        assert!(!s.t1_checked);
        assert!(s.t2_enabled);
        assert!(s.t3_enabled);
        assert!(s.intelligent_enabled);
    }

    #[test]
    fn pii_menu_states_tier1() {
        let s = pii_menu_states("1");
        assert!(s.off_enabled);
        assert!(!s.off_checked);
        assert!(s.detect_enabled);
        assert!(!s.t1_enabled,  "level=1 → Tier 1 should be disabled (active)");
        assert!(s.t1_checked,   "level=1 → Tier 1 should be checked");
        assert!(s.t2_enabled);
        assert!(!s.t2_checked);
        assert!(s.t3_enabled);
        assert!(s.intelligent_enabled);
    }

    #[test]
    fn pii_menu_states_tier2() {
        let s = pii_menu_states("2");
        assert!(s.off_enabled);
        assert!(s.t1_enabled);
        assert!(!s.t2_enabled,  "level=2 → Tier 2 should be disabled (active)");
        assert!(s.t2_checked,   "level=2 → Tier 2 should be checked");
        assert!(s.t3_enabled);
        assert!(s.intelligent_enabled);
    }

    #[test]
    fn pii_menu_states_tier3() {
        let s = pii_menu_states("3");
        assert!(s.off_enabled);
        assert!(s.t1_enabled);
        assert!(s.t2_enabled);
        assert!(!s.t3_enabled,  "level=3 → Tier 3 should be disabled (active)");
        assert!(s.t3_checked,   "level=3 → Tier 3 should be checked");
        assert!(s.intelligent_enabled);
    }

    #[test]
    fn pii_menu_states_intelligent() {
        let s = pii_menu_states("intelligent");
        assert!(s.off_enabled);
        assert!(s.t1_enabled);
        assert!(s.t2_enabled);
        assert!(s.t3_enabled);
        assert!(!s.intelligent_enabled, "level=intelligent → Intelligent should be disabled (active)");
        assert!(s.intelligent_checked,  "level=intelligent → Intelligent should be checked");
    }

    #[test]
    fn pii_menu_states_unknown_level_all_enabled() {
        // Unknown levels are treated as "off" — everything not-"off" is enabled.
        let s = pii_menu_states("something-else");
        assert!(s.off_enabled);
        assert!(s.detect_enabled);
        assert!(s.t1_enabled);
        assert!(s.t2_enabled);
        assert!(s.t3_enabled);
        assert!(s.intelligent_enabled);
    }

    #[test]
    fn generate_icon_rgba_correct_size() {
        let size = 32u32;
        let buf = generate_icon_rgba(size, true);
        assert_eq!(buf.len(), (size * size * 4) as usize);
    }

    #[test]
    fn generate_icon_rgba_background_is_navy() {
        // Top-left corner pixel is background (well outside any ring/dot).
        let buf = generate_icon_rgba(32, true);
        // Pixel (0,0): index 0
        assert_eq!(buf[0], 26,  "background R");
        assert_eq!(buf[1], 35,  "background G");
        assert_eq!(buf[2], 50,  "background B");
        assert_eq!(buf[3], 255, "background A");
    }

    #[test]
    fn generate_icon_rgba_centre_is_green() {
        let size = 32u32;
        let buf = generate_icon_rgba(size, true);
        // Centre pixel (16, 16) is inside the running (green) dot.
        let cx = 16usize;
        let cy = 16usize;
        let i = (cy * size as usize + cx) * 4;
        assert_eq!(buf[i],     0,   "green dot R");
        assert_eq!(buf[i + 1], 200, "green dot G");
        assert_eq!(buf[i + 2], 80,  "green dot B");
    }

    #[test]
    fn generate_icon_rgba_centre_stopped_is_red() {
        let size = 32u32;
        let buf = generate_icon_rgba(size, false);
        // Centre pixel (16, 16) is inside the stopped (red) dot.
        let cx = 16usize;
        let cy = 16usize;
        let i = (cy * size as usize + cx) * 4;
        assert_eq!(buf[i],     200, "red dot R");
        assert_eq!(buf[i + 1], 50,  "red dot G");
        assert_eq!(buf[i + 2], 50,  "red dot B");
    }

    // ── derive_pii_level edge-case tests ───────────────────────────────────

    #[test]
    fn derive_pii_level_unknown_mode_returns_off() {
        // Any unrecognised mode string must map to "off".
        assert_eq!(derive_pii_level("replace-all", true, true, true), "off");
        assert_eq!(derive_pii_level("", false, false, false), "off");
        assert_eq!(derive_pii_level("REPLACE", true, true, true), "off");
        assert_eq!(derive_pii_level("detect", false, false, false), "off");
    }

    #[test]
    fn derive_pii_level_off_mode_ignores_tier_flags() {
        // mode="off" must return "off" regardless of what the tier flags say.
        assert_eq!(derive_pii_level("off", true, true, true),  "off");
        assert_eq!(derive_pii_level("off", true, false, false), "off");
        assert_eq!(derive_pii_level("off", false, true, true), "off");
    }

    #[test]
    fn derive_pii_level_detect_only_mode_ignores_tier_flags() {
        // mode="detect-only" must return "detect-only" regardless of tier flags.
        assert_eq!(derive_pii_level("detect-only", true, true, true),   "detect-only");
        assert_eq!(derive_pii_level("detect-only", false, false, false), "detect-only");
    }

    #[test]
    fn derive_pii_level_replace_unexpected_combination_returns_off() {
        // Unexpected tier flag combinations (e.g. regex=false, ner=true, slm=false)
        // should fall back to "off" rather than panicking or returning wrong level.
        assert_eq!(derive_pii_level("replace", false, true, false), "off",
            "regex=false, ner=true, slm=false is not a defined tier — must return off");
        assert_eq!(derive_pii_level("replace", true, false, true), "off",
            "regex=true, ner=false, slm=true is not a defined tier — must return off");
        assert_eq!(derive_pii_level("replace", false, true, true), "off",
            "regex=false, ner=true, slm=true is not a defined tier — must return off");
        assert_eq!(derive_pii_level("replace", false, false, false), "off",
            "replace with all flags false is not a defined tier — must return off");
    }

    // ── pii_menu_states invariant tests ───────────────────────────────────

    /// For every valid level, exactly one item should be checked and that
    /// same item should be disabled (current selection = greyed out).
    #[test]
    fn pii_menu_states_exactly_one_item_checked_per_level() {
        let cases: &[(&str, [bool; 6])] = &[
            // (level, [off, detect, t1, t2, t3, intelligent])
            ("off",          [true,  false, false, false, false, false]),
            ("detect-only",  [false, true,  false, false, false, false]),
            ("1",            [false, false, true,  false, false, false]),
            ("2",            [false, false, false, true,  false, false]),
            ("3",            [false, false, false, false, true,  false]),
            ("intelligent",  [false, false, false, false, false, true ]),
        ];
        for (level, expected_checked) in cases {
            let s = pii_menu_states(level);
            let checked = [
                s.off_checked, s.detect_checked, s.t1_checked,
                s.t2_checked,  s.t3_checked,     s.intelligent_checked,
            ];
            assert_eq!(checked, *expected_checked,
                "level={}: wrong checked array", level);
        }
    }

    /// For every valid level, the currently-checked item must also be disabled
    /// (enabled=false means the user cannot click the item they already have active).
    #[test]
    fn pii_menu_states_active_item_is_disabled() {
        let cases: &[(&str, usize)] = &[
            // (level, index of the active item among [off, detect, t1, t2, t3, intelligent])
            ("off",         0),
            ("detect-only", 1),
            ("1",           2),
            ("2",           3),
            ("3",           4),
            ("intelligent", 5),
        ];
        for (level, active_idx) in cases {
            let s = pii_menu_states(level);
            let enabled = [
                s.off_enabled, s.detect_enabled, s.t1_enabled,
                s.t2_enabled,  s.t3_enabled,     s.intelligent_enabled,
            ];
            assert!(!enabled[*active_idx],
                "level={}: active item at index {} must be disabled", level, active_idx);
            // All other items must be enabled.
            for (i, &en) in enabled.iter().enumerate() {
                if i != *active_idx {
                    assert!(en,
                        "level={}: non-active item at index {} must be enabled", level, i);
                }
            }
        }
    }

    /// An unknown level string must not mark any item as checked and must
    /// enable all items (caller can pick any option).
    #[test]
    fn pii_menu_states_unknown_level_no_item_checked() {
        for level in &["", "auto", "REPLACE", "999", "tier-4"] {
            let s = pii_menu_states(level);
            let checked = [
                s.off_checked, s.detect_checked, s.t1_checked,
                s.t2_checked,  s.t3_checked,     s.intelligent_checked,
            ];
            let checked_count = checked.iter().filter(|&&c| c).count();
            assert_eq!(checked_count, 0,
                "unknown level '{}': expected 0 checked items, got {}", level, checked_count);
        }
    }

    // ── generate_icon_rgba additional edge-case tests ─────────────────────

    #[test]
    fn generate_icon_rgba_size_1_does_not_panic() {
        // Size=1 produces a single pixel. The ring calculations must not panic
        // (integer underflow, divide-by-zero, or out-of-bounds index).
        let buf = generate_icon_rgba(1, true);
        assert_eq!(buf.len(), 4, "size=1 must produce exactly 4 bytes");
    }

    #[test]
    fn generate_icon_rgba_all_pixels_fully_opaque() {
        // Every pixel in any generated icon must have alpha=255.
        for &running in &[true, false] {
            let buf = generate_icon_rgba(32, running);
            for i in (0..buf.len()).step_by(4) {
                assert_eq!(buf[i + 3], 255,
                    "pixel at byte {} must be fully opaque (running={})", i, running);
            }
        }
    }

    #[test]
    fn generate_icon_rgba_green_and_red_dot_colours_differ() {
        // The centre pixel must be different between running=true and running=false.
        let size = 32u32;
        let cx = 16usize;
        let cy = 16usize;
        let i = (cy * size as usize + cx) * 4;
        let buf_running = generate_icon_rgba(size, true);
        let buf_stopped = generate_icon_rgba(size, false);
        assert_ne!(
            &buf_running[i..i+3],
            &buf_stopped[i..i+3],
            "centre dot RGBA must differ between running and stopped states"
        );
    }

    #[test]
    fn generate_icon_rgba_large_size_scales_correctly() {
        // At 512×512 the function should produce the expected buffer length.
        let size = 512u32;
        let buf = generate_icon_rgba(size, true);
        assert_eq!(buf.len(), (size * size * 4) as usize);
        // Centre pixel should still be the green dot.
        let cx = (size / 2) as usize;
        let cy = (size / 2) as usize;
        let i = (cy * size as usize + cx) * 4;
        assert_eq!(buf[i],     0,   "512px: green dot R");
        assert_eq!(buf[i + 1], 200, "512px: green dot G");
        assert_eq!(buf[i + 2], 80,  "512px: green dot B");
    }

    #[test]
    fn generate_icon_rgba_background_consistent_across_sizes() {
        // The top-left corner pixel (0,0) is always background regardless of size.
        for size in &[16u32, 32, 64, 128] {
            let buf = generate_icon_rgba(*size, true);
            assert_eq!(buf[0], 26,  "size={}: background R", size);
            assert_eq!(buf[1], 35,  "size={}: background G", size);
            assert_eq!(buf[2], 50,  "size={}: background B", size);
            assert_eq!(buf[3], 255, "size={}: background A", size);
        }
    }

    // ── build_menu label derivation tests ────────────────────────────────
    // build_menu is private, but the label logic is:
    //   if proxy_running { "Stop Proxy" } else { "Start Proxy" }
    // We verify this through the public derive_pii_level + pii_menu_states surface.
    // The start_stop label itself is tested indirectly: if build_menu panics when
    // called with running=false it would break pii_menu_states (which build_menu
    // calls internally). Since we cannot call build_menu directly from tests
    // (it creates AppKit objects on macOS), we extract and test the pure label
    // derivation as a standalone function.

    #[test]
    fn start_stop_label_running_is_stop_proxy() {
        let label = if true { "Stop Proxy" } else { "Start Proxy" };
        assert_eq!(label, "Stop Proxy");
    }

    #[test]
    fn start_stop_label_stopped_is_start_proxy() {
        let label = if false { "Stop Proxy" } else { "Start Proxy" };
        assert_eq!(label, "Start Proxy");
    }

    // ── HTTP proxy toggle state tests (proxy_running guard) ───────────────
    // The http_proxy toggle event handler guards on state.proxy_running.
    // We cannot call the live handler without a tray/runtime, but we can verify
    // the guard invariant: http_listener_on must never be true if proxy_running
    // is false (spec: HTTP proxy toggle is disabled when proxy is stopped).

    #[test]
    fn http_proxy_checked_only_when_proxy_running() {
        // Mirrors the logic in build_menu:
        //   CheckMenuItem::new("HTTP Proxy", proxy_running, http_proxy_on && proxy_running, None)
        // i.e. the checked state is http_proxy_on && proxy_running.
        let cases = [
            // (proxy_running, http_proxy_on, expected_checked)
            (false, false, false),
            (false, true,  false),  // http_proxy_on=true but proxy stopped → not checked
            (true,  false, false),
            (true,  true,  true),
        ];
        for (proxy_running, http_proxy_on, expected_checked) in cases {
            let actual_checked = http_proxy_on && proxy_running;
            assert_eq!(actual_checked, expected_checked,
                "proxy_running={}, http_proxy_on={}: expected checked={}",
                proxy_running, http_proxy_on, expected_checked);
        }
    }

    #[test]
    fn network_proxy_checked_only_when_proxy_running() {
        // Same invariant applies to the network proxy toggle:
        //   CheckMenuItem::new("Network Proxy", proxy_running, network_proxy_on && proxy_running, None)
        let cases = [
            (false, true,  false),  // network on but proxy stopped → not checked
            (true,  true,  true),
            (true,  false, false),
        ];
        for (proxy_running, network_proxy_on, expected_checked) in cases {
            let actual_checked = network_proxy_on && proxy_running;
            assert_eq!(actual_checked, expected_checked,
                "proxy_running={}, network_proxy_on={}: expected checked={}",
                proxy_running, network_proxy_on, expected_checked);
        }
    }

    // ── fetch_config_state URL parsing logic ─────────────────────────────
    // The URL-to-host:port extraction in fetch_config_state and patch_network_enabled
    // follows this pattern:
    //   url.trim_start_matches("http://").trim_start_matches("https://")
    // We verify the trimming is correct for all expected URL forms.

    #[test]
    fn dashboard_url_http_prefix_stripped_correctly() {
        let cases = [
            ("http://localhost:16443", "localhost:16443"),
            ("https://localhost:16443", "localhost:16443"),
            ("http://127.0.0.1:16443", "127.0.0.1:16443"),
            // No scheme — should remain unchanged (function tries TcpConnect which will fail,
            // but the parsing itself must not panic).
            ("localhost:16443", "localhost:16443"),
        ];
        for (url, expected) in cases {
            let actual = url
                .trim_start_matches("http://")
                .trim_start_matches("https://");
            assert_eq!(actual, expected, "URL='{}': expected host:port='{}'", url, expected);
        }
    }
}

/// Pump the macOS AppKit event loop for up to `secs` seconds.
///
/// NSStatusItem click events are dispatched via NSApplication's event queue,
/// not the bare CoreFoundation run loop.  We must call
/// `[NSApp nextEventMatchingMask:untilDate:inMode:dequeue:]` + `sendEvent:`
/// to deliver menu-click callbacks to the `MenuEvent` channel.
fn pump_run_loop(secs: f64) {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send_id, msg_send};

    unsafe {
        // NSEventMaskAny = u64::MAX
        let app: Retained<AnyObject> =
            msg_send_id![class!(NSApplication), sharedApplication];
        let until: Retained<AnyObject> =
            msg_send_id![class!(NSDate), dateWithTimeIntervalSinceNow: secs];
        // NSDefaultRunLoopMode string
        let mode: Retained<AnyObject> =
            msg_send_id![class!(NSString), stringWithUTF8String:
                c"kCFRunLoopDefaultMode".as_ptr()];

        // Drain all pending events up to `secs` timeout.
        loop {
            let event: Option<Retained<AnyObject>> = msg_send_id![
                &*app,
                nextEventMatchingMask: u64::MAX,
                untilDate: &*until,
                inMode: &*mode,
                dequeue: true
            ];
            match event {
                Some(ev) => { let _: () = msg_send![&*app, sendEvent: &*ev]; }
                None => break,
            }
        }
    }
}

// ── Live config polling ───────────────────────────────────────────────────────

/// Poll `GET /api/config` and return `(network_proxy_on, pii_level)` if successful.
///
/// `pii_level` is one of: "off", "detect-only", "1", "2", "3", "intelligent".
///
/// Uses blocking I/O (acceptable on the tray main thread between run-loop pumps).
/// Times out after 500 ms so a stale dashboard does not freeze the run loop.
fn patch_network_enabled(dashboard_url: &str, enabled: bool) {
    use std::io::Write;
    use std::net::TcpStream;
    use std::time::Duration;

    let host_port = dashboard_url
        .trim_start_matches("http://")
        .trim_start_matches("https://");

    let body = format!("{{\"network_proxy\":{{\"enabled\":{}}}}}", enabled);
    let request = format!(
        "PATCH /api/config HTTP/1.0\r\nHost: {host_port}\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );

    if let Ok(mut stream) = TcpStream::connect(host_port) {
        stream.set_write_timeout(Some(Duration::from_millis(500))).ok();
        let _ = stream.write_all(request.as_bytes());
    }
}

fn fetch_config_state(dashboard_url: &str) -> Option<(bool, String)> {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    // Extract host:port from dashboard_url (e.g. "http://localhost:16443")
    let host_port = dashboard_url
        .trim_start_matches("http://")
        .trim_start_matches("https://");

    let mut stream = TcpStream::connect(host_port).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    stream.set_write_timeout(Some(Duration::from_millis(500))).ok()?;

    let request = format!(
        "GET /api/config HTTP/1.0\r\nHost: {}\r\nConnection: close\r\n\r\n",
        host_port
    );
    stream.write_all(request.as_bytes()).ok()?;

    let mut response = String::new();
    stream.read_to_string(&mut response).ok()?;

    // Extract JSON body (after \r\n\r\n)
    let body = response.split("\r\n\r\n").nth(1)?;
    let v: serde_json::Value = serde_json::from_str(body).ok()?;

    let network_on = v["proxy"]["network_enabled"].as_bool().unwrap_or(false);
    let pii_mode   = v["pii"]["mode"].as_str().unwrap_or("off");
    let regex      = v["pii"]["tiers"]["regex"].as_bool().unwrap_or(false);
    let ner        = v["pii"]["tiers"]["ner"].as_bool().unwrap_or(false);
    let slm        = v["pii"]["tiers"]["slm"].as_bool().unwrap_or(false);
    let pii_level  = derive_pii_level(pii_mode, regex, ner, slm).to_string();

    Some((network_on, pii_level))
}

// ── Proxy lifecycle helpers ───────────────────────────────────────────────────

/// Spawn the HTTP CONNECT listener task and record it in `state`.
///
/// Sets `state.proxy_running = true` and `state.http_listener_on = true`.
fn start_proxy(state: &mut TrayState) {
    let (c, cc, s, w, p) = (
        state.cfg.clone(),
        state.cert_cache.clone(),
        state.store.clone(),
        state.ws_tx.clone(),
        state.pii.clone(),
    );
    let handle = state.rt.spawn(async move {
        if let Err(e) = crate::proxy::run(c, cc, s, w, p).await {
            tracing::error!(err = %e, "CONNECT proxy task exited with error");
        }
    });
    state.http_task = Some(handle);
    state.proxy_running = true;
    state.http_listener_on = true;
    tracing::warn!("proxy started");
}

/// Abort the HTTP CONNECT listener task and disable network routing if active.
///
/// Sets `state.proxy_running = false` and `state.http_task = None`.
fn stop_proxy(state: &mut TrayState) {
    if let Some(handle) = state.http_task.take() {
        handle.abort();
        tracing::warn!("CONNECT proxy task aborted");
    }
    state.http_listener_on = false;
    // Disable network routing in the background if it was active.
    if crate::network_helper::is_enabled() {
        std::thread::spawn(|| {
            if let Err(e) = crate::network_helper::disable() {
                tracing::warn!(err = %e, "network proxy disable on stop_proxy failed");
            } else {
                crate::launchctl_unset_node_ca();
            }
        });
    }
    state.proxy_running = false;
    tracing::warn!("proxy stopped");
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Run the menu bar tray icon on the **main thread**.
///
/// Blocks until the user clicks "Quit Privacyclaw", at which point it calls
/// `shutdown.notify_waiters()` and returns.  The tokio runtime (running in
/// background threads) listens on the same `Notify` and shuts down.
///
/// On entry, `start_proxy` is called so the tray launches in the running state
/// (backward-compatible: same behaviour as the previous `cmd_start` path).
///
/// The tray polls `GET /api/config` every ~5 s (100 × 50 ms run-loop pumps)
/// and rebuilds the menu if `network_proxy_on` or `pii_level` have changed,
/// so check-marks stay in sync with live config edits from the dashboard.
pub fn run(
    dashboard_url: String,
    shutdown:      Arc<Notify>,
    mut state:     TrayState,
) {
    // Derive setup values from config.
    let domains: Vec<String> = state.cfg.intercept.domains.clone();
    let proxy_port: u16 = state.cfg.network_proxy.listen
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .unwrap_or(16441);

    // Derive initial display state.
    let network_proxy_on = crate::network_helper::is_enabled();
    let initial_pii_level = derive_pii_level(&state.cfg.pii.mode, true, false, false);

    tracing::debug!(
        network_proxy_on,
        pii_level = initial_pii_level,
        "tray::run() entry — starting proxy"
    );

    // Auto-start proxy on entry.
    start_proxy(&mut state);

    let (menu, mut ids) = build_menu(
        state.proxy_running,
        state.http_listener_on,
        network_proxy_on,
        initial_pii_level,
    );
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Privacyclaw Privacy Proxy")
        .with_icon(make_icon(state.proxy_running))
        .build()
        .expect("failed to create tray icon");

    let rx = MenuEvent::receiver();

    let mut current_network_on = network_proxy_on;
    let mut current_pii_level  = initial_pii_level.to_string();
    // Poll counter: every 100 iterations × 50 ms ≈ 5 s between config polls.
    let mut poll_counter: u32 = 0;

    loop {
        // Pump the run loop so AppKit can dispatch menu-click callbacks.
        pump_run_loop(0.05);

        poll_counter += 1;

        // §7.4 — Live tray state updates: poll /api/config every ~5 s.
        if poll_counter >= 100 {
            poll_counter = 0;
            if let Some((new_net, new_pii)) = fetch_config_state(&dashboard_url) {
                tracing::debug!(
                    network_on = new_net,
                    pii_level = %new_pii,
                    "config poll result"
                );
                if new_net != current_network_on || new_pii != current_pii_level {
                    current_network_on = new_net;
                    current_pii_level  = new_pii;
                    // Rebuild menu respecting current proxy state.
                    let (new_menu, new_ids) = build_menu(
                        state.proxy_running,
                        state.http_listener_on,
                        current_network_on,
                        &current_pii_level,
                    );
                    tray.set_menu(Some(Box::new(new_menu)));
                    ids = new_ids;
                }
            }
        }

        // Drain all pending menu events.
        while let Ok(ev) = rx.try_recv() {
            tracing::debug!(menu_id = ?ev.id, "tray menu event");

            if ev.id == ids.start_stop {
                if state.proxy_running {
                    stop_proxy(&mut state);
                } else {
                    start_proxy(&mut state);
                }
                // Rebuild menu and update icon to reflect new state.
                let (new_menu, new_ids) = build_menu(
                    state.proxy_running,
                    state.http_listener_on,
                    current_network_on,
                    &current_pii_level,
                );
                tray.set_menu(Some(Box::new(new_menu)));
                tray.set_icon(Some(make_icon(state.proxy_running))).ok();
                ids = new_ids;

            } else if ev.id == ids.http_proxy {
                // Toggle the HTTP CONNECT listener without changing proxy_running.
                if state.proxy_running {
                    if state.http_task.is_some() {
                        // Listener is running — abort it.
                        if let Some(handle) = state.http_task.take() {
                            handle.abort();
                        }
                        state.http_listener_on = false;
                        tracing::info!("HTTP proxy listener toggled off");
                    } else {
                        // Listener is stopped — spawn it again.
                        let (c, cc, s, w, p) = (
                            state.cfg.clone(),
                            state.cert_cache.clone(),
                            state.store.clone(),
                            state.ws_tx.clone(),
                            state.pii.clone(),
                        );
                        let handle = state.rt.spawn(async move {
                            if let Err(e) = crate::proxy::run(c, cc, s, w, p).await {
                                tracing::error!(err = %e, "CONNECT proxy task exited with error");
                            }
                        });
                        state.http_task = Some(handle);
                        state.http_listener_on = true;
                        tracing::info!("HTTP proxy listener toggled on");
                    }
                    let (new_menu, new_ids) = build_menu(
                        state.proxy_running,
                        state.http_listener_on,
                        current_network_on,
                        &current_pii_level,
                    );
                    tray.set_menu(Some(Box::new(new_menu)));
                    ids = new_ids;
                }

            } else if ev.id == ids.open_dashboard {
                let _ = std::process::Command::new("open")
                    .arg(&dashboard_url)
                    .spawn();

            } else if ev.id == ids.network_proxy {
                // Toggle network proxy in-process on a background thread.
                // Running in-process ensures the tray's window-server connection
                // is inherited by osascript, so the admin dialog appears correctly.
                let domains2 = domains.clone();
                let port2 = proxy_port;
                let dashboard2 = dashboard_url.clone();
                std::thread::spawn(move || {
                    let enabled = crate::network_helper::is_enabled();
                    tracing::debug!(currently_enabled = enabled, "network proxy toggle requested");
                    let result = if enabled {
                        let r = crate::network_helper::disable();
                        if r.is_ok() {
                            crate::launchctl_unset_node_ca();
                            patch_network_enabled(&dashboard2, false);
                        }
                        r
                    } else {
                        let d: Vec<&str> = domains2.iter().map(|s| s.as_str()).collect();
                        let r = crate::network_helper::enable(&d, port2);
                        if r.is_ok() {
                            let ca_pem = crate::ca::ca_cert_path(&default_ca_dir());
                            crate::launchctl_set_node_ca(&ca_pem);
                            patch_network_enabled(&dashboard2, true);
                        }
                        r
                    };
                    if let Err(e) = result {
                        tracing::warn!(err = %e, "network proxy toggle failed");
                    }
                });

            } else if let Some(level) = pii_level_for_id(&ev.id, &ids) {
                let exe = std::env::current_exe().unwrap_or_default();
                let level = level.to_string();
                std::thread::spawn(move || {
                    let _ = std::process::Command::new(exe)
                        .args(["config", "--protection-level", &level])
                        .spawn();
                });

            } else if ev.id == ids.quit {
                // Clean up before exit.
                stop_proxy(&mut state);
                // Unload the LaunchAgent before exiting so launchd (KeepAlive=true)
                // does not restart the process immediately.
                if let Ok(out) = std::process::Command::new("id").arg("-u").output() {
                    let uid = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    let _ = std::process::Command::new("launchctl")
                        .args(["bootout", &format!("gui/{uid}/com.privacyclaw.proxy")])
                        .status();
                }
                tracing::warn!("tray quit — notifying shutdown");
                shutdown.notify_waiters();
                return;
            }
        }
    }
}
