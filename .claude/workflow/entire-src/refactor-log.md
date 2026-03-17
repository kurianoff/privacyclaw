# Refactor Log — entire-src

## Baseline
Branch: refactor/entire-src
Scope: src/ (entire codebase)
Boundaries: wire protocol (CONNECT tunnel, SSE wire format, WebSocket framing), public CLI interface (argument names and behavior), CA certificate DN parameters, database schema column names
Baseline tests: 374 passed; 2 failed (pre-existing: brew_formula_test — missing homebrew repo files)
Pre-existing failures: formula_privacyclaw_rb_exists_and_valid, cask_privacyclaw_app_rb_exists_and_valid

---
