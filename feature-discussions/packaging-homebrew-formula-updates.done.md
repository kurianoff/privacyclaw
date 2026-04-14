# Feature: Homebrew Formula Updates for Sidecar and T3 Pipeline

**Context**: The current `packaging/homebrew/privacyclaw.rb` formula installs only the `privacyclaw` binary. It satisfies the T3 SLM requirement by declaring `depends_on "llama.cpp"` — meaning Homebrew pulls in the llama.cpp tap and the user gets `llama-server` as a transitive dependency. The `postinstall` script then copies `llama-server` from the share directory into `~/Library/Application Support/privacyclaw/bin/`.

This approach has gaps that need addressing as the sidecar grows into a first-class component.

---

## What needs to change

### 1. Install the Python sidecar script

The `install` block currently only installs the `privacyclaw` binary:

```ruby
def install
  bin.install "privacyclaw"
end
```

It must also install `privacyclaw-slm-sidecar` (the Python sidecar script). Options:

**Option A — Install as a script**: Place it in `libexec` (Homebrew convention for scripts not meant to be on PATH directly) and symlink via a wrapper in `bin/`:
```ruby
libexec.install "privacyclaw-slm-sidecar"
bin.write_exec_script libexec/"privacyclaw-slm-sidecar"
```

**Option B — Install directly to bin**: Simpler, but puts a Python script alongside the Rust binary in the user's PATH. Acceptable for a tool like this.

The choice affects whether users can invoke the sidecar manually (useful for debugging).

### 2. Python dependency

The sidecar is a Python script. The formula must declare a Python runtime dependency:

```ruby
depends_on "python@3.11"  # or whichever minimum version the sidecar requires
```

This ensures Homebrew pulls in Python if the user doesn't already have it. The sidecar's `pip` dependencies (e.g. `requests`, `fastapi`, `uvicorn`) need to either:
- Be vendored into the script (single-file, no imports beyond stdlib)
- Or listed as a `resource` block in the formula with pinned versions and SHA-256 checksums

Vendored single-file approach is strongly preferred — it avoids the Homebrew resource dance and makes the sidecar self-contained.

### 3. llama.cpp dependency strategy

Currently `depends_on "llama.cpp"` pulls the full llama.cpp tap. This means:
- The user gets the Homebrew-managed llama-server, which may drift in version
- The postinstall copies it to the privacyclaw data dir at install time — but if llama.cpp updates later, the copy goes stale

**Option A — Keep `depends_on "llama.cpp"`**: Simplest. Accept version drift risk. Suitable while llama-server's API is stable.

**Option B — Bundle a pinned llama-server binary**: Include a pre-built `llama-server` binary in the release tarball (arm64 + x86_64 via lipo). Remove `depends_on "llama.cpp"`. The formula installs it alongside the privacyclaw binary. Total control over the version; larger tarball (~15–30 MB).

**Recommendation**: Move to Option B when the `/replace` sidecar protocol matures enough that llama-server version matters for prompt behavior. For now, Option A is fine.

### 4. Caveats update

The `caveats` block should mention T3 setup — which model to download, how to activate it — since the formula is the entry point for new users:

```ruby
def caveats
  <<~EOS
    ...existing caveats...

    For Tier 3 PII protection (contextual detection):
      privacyclaw models list          # see available SLM models
      privacyclaw models download qwen2.5-0.5b
      privacyclaw models activate qwen2.5-0.5b
  EOS
end
```

### 5. Formula test block

The existing test block verifies the binary version, PII detection, CA init, and ca-path. Add a sidecar smoke test:

```ruby
test do
  # ...existing tests...
  assert_match "privacyclaw-slm-sidecar", shell_output("#{bin}/privacyclaw-slm-sidecar --version 2>&1")
end
```

---

## Files to change

| File | Change |
|---|---|
| `packaging/homebrew/privacyclaw.rb` | Add sidecar install, Python dependency, updated caveats, test block |
| Release tarball build script | Include sidecar script in tarball alongside binary |

---

## Dependencies on other features

- Requires the Python sidecar script to exist (`feature-discussions/pii-slm-sidecar-replace-endpoint.md`)
- Requires the sidecar to have a stable `--version` flag for the test block
- If bundling llama-server: requires a universal binary build step in the release pipeline
