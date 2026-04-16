# Modernization: Python Sidecar — Dependency Refresh and Python Version

**Scope:** Update the Python sidecar's pinned pip resources in the Homebrew formula and evaluate upgrading the formula's Python version requirement from 3.11 to 3.12 or 3.13.

## What and Why

The Homebrew formula pins specific wheel URLs and SHA-256 hashes for every Python dependency of the sidecar (fastapi, uvicorn, httpx, pydantic, starlette, anyio, etc.). These pinned URLs are point-in-time snapshots and do not receive security updates automatically. As of April 2026, several pins are behind current releases. Additionally, the formula requires `python@3.11`, which will reach end-of-life in October 2027; Python 3.12 and 3.13 are already Homebrew stable, and moving to 3.12 now avoids a future forced migration. The version mismatch between the Cargo.toml version (0.3.0) and the formula's hardcoded `version "0.2.0"` is also a correctness issue in this area — the formula's version string must be kept in sync with Cargo.toml.

## Key Items

Verified current versions as of April 2026 vs formula pins:

- `fastapi`: formula has 0.135.3 — matches PyPI current (0.135.3 released April 1, 2026). No update needed.
- `uvicorn`: formula has 0.42.0 — current on PyPI is 0.44.0 (April 6, 2026). Update needed.
- `pydantic`: formula has 2.12.5 — current on PyPI is ~2.13 (April 15, 2026). Update needed.
- `httpx`: formula has 0.28.1 — matches PyPI current (0.28.1). No update needed.
- `starlette`, `anyio`, `sniffio`, `httpcore`, `pydantic-core`: audit against PyPI for current versions.
- Evaluate `depends_on "python@3.11"` → `"python@3.12"`; test sidecar startup under both
- Fix formula `version "0.2.0"` → `"0.3.0"` to match Cargo.toml (currently drifted)
- Verify `privacyclaw-slm-sidecar` `--version` output matches Cargo.toml version string
- Add a Makefile target or script to automate re-pinning (currently a manual `pip download` procedure)

## Risks

Low-to-medium. Python wheel updates for pure-Python packages (fastapi, uvicorn, starlette) are low risk. pydantic-core includes a compiled C extension; the wheel selection by platform must be verified carefully. Upgrading Python minor version could affect uvicorn startup behavior or anyio compatibility — manual testing of sidecar launch under the new version required.

## Dependencies on other blocks

None — fully independent of the Rust modernization blocks.
