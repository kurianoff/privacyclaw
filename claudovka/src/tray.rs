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
use tray_icon::{
    menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu},
    Icon, TrayIconBuilder,
};

// ── Menu item IDs ─────────────────────────────────────────────────────────────

struct Ids {
    open_dashboard: tray_icon::menu::MenuId,
    network_proxy:  tray_icon::menu::MenuId,
    pii_off:        tray_icon::menu::MenuId,
    pii_t1:         tray_icon::menu::MenuId,
    pii_t2:         tray_icon::menu::MenuId,
    pii_t3:         tray_icon::menu::MenuId,
    quit:           tray_icon::menu::MenuId,
}

// ── Pure helpers (testable without a display) ─────────────────────────────────

/// Returns the enabled/checked state for each PII submenu item based on
/// the current `pii_mode` string.  Used both by `build_menu` and unit tests.
pub fn pii_menu_states(pii_mode: &str) -> PiiMenuStates {
    PiiMenuStates {
        off_enabled:  pii_mode != "off",
        t1_enabled:   pii_mode != "replace",
        t2_enabled:   true,
        t3_enabled:   true,
    }
}

/// The enabled/checked booleans for the four PII submenu items.
#[derive(Debug, PartialEq, Eq)]
pub struct PiiMenuStates {
    /// "Off" item is enabled (clickable) when mode ≠ "off".
    pub off_enabled: bool,
    /// "Tier 1 – Regex" item enabled when mode ≠ "replace".
    pub t1_enabled: bool,
    /// "Tier 2 – NER" always enabled.
    pub t2_enabled: bool,
    /// "Tier 3 – SLM" always enabled.
    pub t3_enabled: bool,
}

// ── Menu construction ─────────────────────────────────────────────────────────

fn build_menu(network_proxy_on: bool, pii_mode: &str) -> (Menu, Ids) {
    let open_dashboard  = MenuItem::new("Open Dashboard", true, None);

    // HTTP proxy is always on while the `start` command is running.
    let http_proxy      = CheckMenuItem::new("HTTP Proxy", true, true, None);
    let network_proxy   = CheckMenuItem::new("Network Proxy", true, network_proxy_on, None);

    // PII submenu — checked state reflects current pii_mode from config.
    let states    = pii_menu_states(pii_mode);
    let pii_sub   = Submenu::new("PII Protection", true);
    let pii_off   = MenuItem::new("Off",              states.off_enabled, None);
    let pii_t1    = MenuItem::new("Tier 1 — Regex",   states.t1_enabled,  None);
    let pii_t2    = MenuItem::new("Tier 2 — NER",     states.t2_enabled,  None);
    let pii_t3    = MenuItem::new("Tier 3 — SLM",     states.t3_enabled,  None);
    let _ = pii_sub.append_items(&[&pii_off, &pii_t1, &pii_t2, &pii_t3]);

    let quit = MenuItem::new("Quit Claudovka", true, None);

    let ids = Ids {
        open_dashboard: open_dashboard.id().clone(),
        network_proxy:  network_proxy.id().clone(),
        pii_off:        pii_off.id().clone(),
        pii_t1:         pii_t1.id().clone(),
        pii_t2:         pii_t2.id().clone(),
        pii_t3:         pii_t3.id().clone(),
        quit:           quit.id().clone(),
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

/// Generate an RGBA pixel buffer for the Claudovka icon.
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

/// §7.T6: `pii_menu_states(pii_mode)` returns correct enabled/checked states.
///
/// These tests run on all platforms without a display because `pii_menu_states`
/// is a pure function that does not create any AppKit or tray-icon objects.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pii_menu_states_off_mode() {
        let s = pii_menu_states("off");
        assert!(!s.off_enabled,  "mode=off → Off item should be disabled (already off)");
        assert!(s.t1_enabled,    "mode=off → Tier 1 item should be enabled");
        assert!(s.t2_enabled);
        assert!(s.t3_enabled);
    }

    #[test]
    fn pii_menu_states_detect_only_mode() {
        let s = pii_menu_states("detect-only");
        assert!(s.off_enabled,   "mode=detect-only → Off item should be enabled");
        assert!(s.t1_enabled,    "mode=detect-only → Tier 1 item should be enabled");
        assert!(s.t2_enabled);
        assert!(s.t3_enabled);
    }

    #[test]
    fn pii_menu_states_replace_mode() {
        let s = pii_menu_states("replace");
        assert!(s.off_enabled,   "mode=replace → Off item should be enabled");
        assert!(!s.t1_enabled,   "mode=replace → Tier 1 item should be disabled (active)");
        assert!(s.t2_enabled);
        assert!(s.t3_enabled);
    }

    #[test]
    fn pii_menu_states_unknown_mode_is_noop() {
        // Unknown modes are treated the same as "detect-only" — everything enabled.
        let s = pii_menu_states("something-else");
        assert!(s.off_enabled);
        assert!(s.t1_enabled);
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

/// Pump the macOS main-thread CFRunLoop for up to `secs` seconds.
///
/// AppKit delivers menu-click callbacks via the main-thread run loop.  Without
/// pumping it the `MenuEvent` channel would never receive events.  We link
/// against `CoreFoundation` (always present on macOS) and call
/// `CFRunLoopRunInMode` directly to avoid pulling in an extra Rust crate.
fn pump_run_loop(secs: f64) {
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        static kCFRunLoopDefaultMode: *const std::ffi::c_void;
        fn CFRunLoopRunInMode(
            mode: *const std::ffi::c_void,
            seconds: f64,
            return_after_source_handled: u8,
        ) -> i32;
    }
    unsafe {
        CFRunLoopRunInMode(kCFRunLoopDefaultMode, secs, 0);
    }
}

// ── Live config polling ───────────────────────────────────────────────────────

/// Poll `GET /api/config` and return `(network_proxy_on, pii_mode)` if successful.
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
    let pii_mode   = v["pii"]["mode"].as_str().unwrap_or("off").to_string();

    Some((network_on, pii_mode))
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Run the menu bar tray icon on the **main thread**.
///
/// Blocks until the user clicks "Quit Claudovka", at which point it calls
/// `shutdown.notify_waiters()` and returns.  The tokio runtime (running in
/// background threads) listens on the same `Notify` and shuts down.
///
/// The tray polls `GET /api/config` every ~5 s (100 × 50 ms run-loop pumps)
/// and rebuilds the menu if `network_proxy_on` or `pii_mode` have changed,
/// so check-marks stay in sync with live config edits from the dashboard.
pub fn run(
    dashboard_url:     String,
    network_proxy_on:  bool,
    pii_mode:          String,
    shutdown:          Arc<Notify>,
) {
    let (menu, mut ids) = build_menu(network_proxy_on, &pii_mode);
    let icon = make_icon();

    // TrayIconBuilder on macOS initialises NSApplication internally (accessory
    // mode: no Dock icon).  The icon appears in the system status bar.
    let mut tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Claudovka Privacy Proxy")
        .with_icon(icon)
        .build()
        .expect("failed to create tray icon");

    let rx = MenuEvent::receiver();

    let mut current_network_on = network_proxy_on;
    let mut current_pii_mode   = pii_mode.clone();
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
                if new_net != current_network_on || new_pii != current_pii_mode {
                    current_network_on = new_net;
                    current_pii_mode   = new_pii.clone();
                    let (new_menu, new_ids) = build_menu(current_network_on, &current_pii_mode);
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
                // Toggle network proxy via subprocess so it gets the admin dialog.
                let enabled = crate::network_helper::is_enabled();
                let arg = if enabled { "network-disable" } else { "network-enable" };
                let exe = std::env::current_exe().unwrap_or_default();
                let _ = std::process::Command::new(exe).arg(arg).spawn();

            } else if ev.id == ids.pii_off || ev.id == ids.pii_t1
                   || ev.id == ids.pii_t2 || ev.id == ids.pii_t3 {
                // Open dashboard Settings panel for PII config changes.
                let url = format!("{}/settings", dashboard_url);
                let _ = std::process::Command::new("open").arg(&url).spawn();

            } else if ev.id == ids.quit {
                shutdown.notify_waiters();
                return;
            }
        }
    }
}
