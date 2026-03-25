# Feature: Binary Bundling Strategy for Sidecar and llama-server

**Context**: Privacyclaw currently ships only the `privacyclaw` Rust binary in its release tarball. The T3 SLM requirement is satisfied indirectly via `depends_on "llama.cpp"` in the Homebrew formula. As the sidecar evolves into a first-class component with a custom protocol (`/replace`, `/disambiguate`), the current approach creates version coupling risks and a poor first-run experience.

---

## Problem statement

Three separate binaries/scripts must land on the user's machine for T3 to work:

1. `privacyclaw` — the Rust proxy (currently shipped)
2. `llama-server` — the GGUF model inference backend (currently via Homebrew dep)
3. `privacyclaw-slm-sidecar` — the Python wrapper that translates between the proxy API and llama-server (currently not shipped at all)

The user experience today for T3: install privacyclaw → notice T3 doesn't work → read docs → `brew install llama.cpp` → download a GGUF model manually → configure the path. This is too many manual steps.

---

## Options

### Option A — Homebrew dependency (current approach, extended)

Keep `depends_on "llama.cpp"`. Add `depends_on "python@3.11"`. Add the sidecar script to the formula's install block.

**Pros**: Small tarball. Homebrew manages llama-server updates.
**Cons**: llama-server version is uncontrolled (may break sidecar prompt behavior on updates). Python version management via Homebrew is finicky. Requires Homebrew — doesn't work for `.pkg` installer users.

### Option B — Bundle llama-server in release tarball

Build a universal (arm64 + x86_64) `llama-server` binary and include it in the release tarball. The `postinstall` script already has the copy logic (`LLAMA_SRC="$SHARE_DIR/llama-server"` → `LLAMA_DEST=".../bin/llama-server"`). Only the tarball build step changes.

**Pros**: Pinned version — no drift. Works for both Homebrew and `.pkg` installer. Existing postinstall already handles the copy.
**Cons**: Tarball grows by ~15–30 MB. Must maintain a build step to produce the universal binary on each privacyclaw release.

**Build step**: `lipo -create llama-server-arm64 llama-server-x86_64 -output llama-server-universal`

### Option C — PyInstaller-packaged sidecar

Instead of shipping a Python script + requiring a Python runtime, compile the sidecar into a self-contained binary using PyInstaller. The result is a single executable `privacyclaw-slm-sidecar` with Python and all dependencies embedded.

**Pros**: No Python runtime required on the user's machine. Single binary installable like any other. No Homebrew Python dependency.
**Cons**: PyInstaller binaries are ~10–30 MB. Startup is slightly slower (first-run unpacking). macOS Gatekeeper notarization is required. CI build step needed per architecture.

**Recommendation for near-term**: Option B (bundle llama-server) + ship sidecar as a Python script (requires Python 3.x, which macOS ships by default since Ventura). Move to Option C later if Python runtime management becomes a support burden.

---

## Practical impact on postinstall

The existing `postinstall` script already handles the llama-server copy pattern. Extending it for the sidecar is a small addition:

```bash
SIDECAR_SRC="$SHARE_DIR/privacyclaw-slm-sidecar"
SIDECAR_DEST="$USER_HOME/Library/Application Support/privacyclaw/bin/privacyclaw-slm-sidecar"

if [ -f "$SIDECAR_SRC" ]; then
    cp "$SIDECAR_SRC" "$SIDECAR_DEST"
    chmod 755 "$SIDECAR_DEST"
    echo "[privacyclaw] sidecar installed to $SIDECAR_DEST"
else
    echo "[privacyclaw] WARNING: sidecar not bundled — Tier 3 /replace requires privacyclaw-slm-sidecar"
fi
```

This is the only postinstall change in scope for the current T3-first pipeline feature. Everything else in this document is follow-up work.

---

## Release pipeline changes

Whichever option is chosen, the release tarball build process (currently undocumented beyond the Makefile comment in the formula) needs to be formalized:

| Artifact | Source | Build step |
|---|---|---|
| `privacyclaw` | `cargo build --release --target aarch64-apple-darwin` + x86 + lipo | Existing |
| `llama-server` | llama.cpp release download or local build | New (Option B) |
| `privacyclaw-slm-sidecar` | Python script from repo | New (copy) or PyInstaller (Option C) |

A `Makefile` target or CI workflow (GitHub Actions) should produce the tarball with all three artifacts and compute the SHA-256 for the Homebrew formula update.

---

## Dependencies on other features

- Requires the Python sidecar script (`feature-discussions/pii-slm-sidecar-replace-endpoint.md`)
- Requires Homebrew formula updates (`feature-discussions/packaging-homebrew-formula-updates.md`)
- If Option C: requires a notarization step in the release pipeline
