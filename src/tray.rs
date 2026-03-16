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

// ── Menu item IDs ─────────────────────────────────────────────────────────────

struct Ids {
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

fn build_menu(network_proxy_on: bool, pii_level: &str) -> (Menu, Ids) {
    let open_dashboard  = MenuItem::new("Open Dashboard", true, None);

    // HTTP proxy is always on while the `start` command is running.
    let http_proxy      = CheckMenuItem::new("HTTP Proxy", true, true, None);
    let network_proxy   = CheckMenuItem::new("Network Proxy", true, network_proxy_on, None);

    // PII submenu — checked state reflects current pii_level derived from config.
    let states = pii_menu_states(pii_level);
    let pii_sub = Submenu::new("PII Protection", true);
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

    let quit = MenuItem::new("Quit Privacyclaw", true, None);

    let ids = Ids {
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
/// teal centre dot.  This is a purely procedural icon — no external assets
/// required.  At 32×32 the ring and dot are clearly visible in the menu bar;
/// at 512×512 the same function produces a high-resolution version suitable
/// for the app bundle.
pub fn generate_icon_rgba(size: u32) -> Vec<u8> {
    let mut pixels = vec![0u8; (size * size * 4) as usize];
    let cx = size / 2;
    let cy = size / 2;
    let r_outer = size * 45 / 100; // outer radius of ring
    let ring_width = (size / 16).max(1);
    let r_inner_dot = size * 15 / 100; // solid teal centre dot

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

            // Teal centre dot
            if dist2 <= r_inner_dot * r_inner_dot {
                pixels[i]     = 100;
                pixels[i + 1] = 220;
                pixels[i + 2] = 200;
            }
        }
    }
    pixels
}

/// Build the tray icon (32×32, procedurally generated).
fn make_icon() -> Icon {
    const N: u32 = 32;
    let rgba = generate_icon_rgba(N);
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
        let buf = generate_icon_rgba(size);
        assert_eq!(buf.len(), (size * size * 4) as usize);
    }

    #[test]
    fn generate_icon_rgba_background_is_navy() {
        // Top-left corner pixel is background (well outside any ring/dot).
        let buf = generate_icon_rgba(32);
        // Pixel (0,0): index 0
        assert_eq!(buf[0], 26,  "background R");
        assert_eq!(buf[1], 35,  "background G");
        assert_eq!(buf[2], 50,  "background B");
        assert_eq!(buf[3], 255, "background A");
    }

    #[test]
    fn generate_icon_rgba_centre_is_teal() {
        let size = 32u32;
        let buf = generate_icon_rgba(size);
        // Centre pixel (16, 16) is inside the teal dot.
        let cx = 16usize;
        let cy = 16usize;
        let i = (cy * size as usize + cx) * 4;
        assert_eq!(buf[i],     100, "teal R");
        assert_eq!(buf[i + 1], 220, "teal G");
        assert_eq!(buf[i + 2], 200, "teal B");
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

// ── Public API ────────────────────────────────────────────────────────────────

/// Run the menu bar tray icon on the **main thread**.
///
/// Blocks until the user clicks "Quit Privacyclaw", at which point it calls
/// `shutdown.notify_waiters()` and returns.  The tokio runtime (running in
/// background threads) listens on the same `Notify` and shuts down.
///
/// The tray polls `GET /api/config` every ~5 s (100 × 50 ms run-loop pumps)
/// and rebuilds the menu if `network_proxy_on` or `pii_level` have changed,
/// so check-marks stay in sync with live config edits from the dashboard.
pub fn run(
    dashboard_url:     String,
    network_proxy_on:  bool,
    pii_mode:          String,
    shutdown:          Arc<Notify>,
    domains:           Vec<String>,
    proxy_port:        u16,
) {
    // Derive the initial pii_level from the mode string alone (no tiers at
    // startup — the first poll will refine it within 5 s).
    let initial_pii_level = derive_pii_level(&pii_mode, true, false, false);
    let (menu, mut ids) = build_menu(network_proxy_on, initial_pii_level);
    let icon = make_icon();

    // TrayIconBuilder on macOS initialises NSApplication internally (accessory
    // mode: no Dock icon).  The icon appears in the system status bar.
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Privacyclaw Privacy Proxy")
        .with_icon(icon)
        .build()
        .expect("failed to create tray icon");

    let rx = MenuEvent::receiver();

    let mut current_network_on  = network_proxy_on;
    let mut current_pii_level   = initial_pii_level.to_string();
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
                if new_net != current_network_on || new_pii != current_pii_level {
                    current_network_on = new_net;
                    current_pii_level  = new_pii.clone();
                    let (new_menu, new_ids) = build_menu(current_network_on, &current_pii_level);
                    tray.set_menu(Some(Box::new(new_menu)));
                    ids = new_ids;
                }
            }
        }

        // Drain all pending menu events.
        while let Ok(ev) = rx.try_recv() {
            if ev.id == ids.open_dashboard {
                let _ = std::process::Command::new("open")
                    .arg(&dashboard_url)
                    .spawn();

            } else if ev.id == ids.network_proxy {
                // Toggle network proxy in-process on a background thread.
                // Running in-process ensures the tray's window-server connection
                // is inherited by osascript, so the admin dialog appears correctly.
                let domains2 = domains.clone();
                let port2 = proxy_port;
                std::thread::spawn(move || {
                    let enabled = crate::network_helper::is_enabled();
                    let result = if enabled {
                        let r = crate::network_helper::disable();
                        if r.is_ok() { crate::launchctl_unset_node_ca(); }
                        r
                    } else {
                        let d: Vec<&str> = domains2.iter().map(|s| s.as_str()).collect();
                        let r = crate::network_helper::enable(&d, port2);
                        if r.is_ok() {
                            let ca_pem = crate::ca::ca_cert_path(&default_ca_dir());
                            crate::launchctl_set_node_ca(&ca_pem);
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
                shutdown.notify_waiters();
                return;
            }
        }
    }
}
