# Privacyclaw CLI Reference

## Tray Icon States

When running with `privacyclaw start --tray`, a menu bar icon appears in the macOS status bar.

- **Green centre dot** — proxy is running; HTTP CONNECT listener is active and accepting connections.
- **Red centre dot** — proxy is stopped; the HTTP CONNECT listener has been shut down. The dashboard remains accessible.

## Start/Stop Proxy

The **Stop Proxy** / **Start Proxy** menu item toggles the HTTP CONNECT listener and network routing without exiting the application.

- **Stop Proxy**: aborts the HTTP CONNECT listener task; if network routing (pf/hosts) was active, disables it. The dashboard continues running.
- **Start Proxy**: spawns a new HTTP CONNECT listener on the configured port; updates the icon to green.

The tray launches in the running state (Start Proxy is called automatically on entry), so existing behaviour is preserved.

## HTTP Proxy Toggle

The **HTTP Proxy** checkbox controls the HTTP CONNECT listener independently of the Stop/Start lifecycle.

- Only available while the proxy is in the **running** state.
- Toggling off aborts the listener task (port released) without stopping network routing.
- Toggling back on spawns a new listener task.

This allows temporarily releasing the proxy port without a full Stop/Start cycle.

## Network Proxy Toggle

The **Network Proxy** checkbox controls the macOS pf redirect rules and `/etc/hosts` entries.

- Requires administrator credentials (presented via osascript dialog).
- Only available while the proxy is in the running state.

## Dashboard

The dashboard (`http://localhost:16443` by default) is always accessible while the tray process is running, regardless of proxy state. Use **Open Dashboard** from the menu to open it in the default browser.

## PII Protection

The **PII Protection** submenu sets the active protection level. The change takes effect immediately for new connections.

| Level | Description |
|-------|-------------|
| Off | No PII filtering |
| Detect only | Detect and log PII but do not replace |
| Tier 1 — Regex | Replace PII matched by regex patterns |
| Tier 2 — NER | Tier 1 plus named-entity recognition |
| Tier 3 — Full Pipeline | Tier 2 plus SLM-based detection |
| Intelligent (T3 only) | SLM detection without regex/NER |
