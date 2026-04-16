# Modernization: Python Sidecar — Dependency Refresh and Python Version

**Scope:** Update the Python sidecar's pinned pip resources in the Homebrew formula and evaluate upgrading the formula's Python version requirement from 3.11 to 3.12 or 3.13.

## What and Why

The Homebrew formula pins specific wheel URLs and SHA-256 hashes for every Python dependency of the sidecar (fastapi, uvicorn, httpx, pydantic, starlette, anyio, etc.). These pinned URLs are point-in-time snapshots and do not receive security updates automatically. Additionally, the formula requires `python@3.11`, which is the current LTS but will reach end-of-life in October 2027; Python 3.12 and 3.13 are already Homebrew stable, and moving to 3.12 now avoids a future forced migration. The version mismatch between the Cargo.toml version (0.3.0) and the formula's hardcoded `version "0.2.0"` is also a correctness issue in this area — the formula's version string must be kept in sync with Cargo.toml.

## Key Items

- Audit each pinned Python wheel (fastapi, uvicorn, httpx, pydantic, starlette, anyio, sniffio, httpcore, pydantic-core) for CVE advisories and newer releases
- Re-pin all formula resources to the latest compatible wheel SHAs (re-run `pip download` procedure documented in the formula comment)
- Evaluate `depends_on "python@3.11"` → `"python@3.12"` (or 3.13); test sidecar startup under both
- Fix formula `version "0.2.0"` → `"0.3.0"` to match Cargo.toml (currently drifted)
- Verify `privacyclaw-slm-sidecar` `--version` output matches Cargo.toml version string
- Add a Makefile target or script to automate re-pinning (currently a manual `pip download` procedure)

## Risks

Low-to-medium. Python wheel updates for pure-Python packages (fastapi, uvicorn, starlette) are low risk. pydantic-core includes a compiled C extension; the wheel selection by platform must be verified carefully. Upgrading Python minor version could affect uvicorn startup behavior or anyio compatibility — manual testing of sidecar launch under the new version required.

## Dependencies on other blocks

None — fully independent of the Rust modernization blocks.
