# Review Add-On: Cycle 1

## Valid Findings

### RC1-1: Tap formula URL hardcodes version 0.3.0 while formula declares version 0.2.0
**Severity:** major
**File:** homebrew-privacyclaw/Formula/privacyclaw.rb:31
**Finding:** The tap formula declares `version "0.2.0"` at line 23 but the tarball URL at line 31 contains a hardcoded `privacyclaw-0.3.0-universal-apple-darwin.tar.gz`. Homebrew substitutes `v#{version}` in the download path prefix but the filename part is literal, resulting in a broken URL `releases/download/v0.2.0/privacyclaw-0.3.0-...`. Any `brew install` attempt will fetch a non-existent asset.
**Required fix:** In `homebrew-privacyclaw/Formula/privacyclaw.rb`, replace the hardcoded `0.3.0` in the URL with `#{version}` so the full URL becomes `privacyclaw-#{version}-universal-apple-darwin.tar.gz`, matching the source formula at `packaging/homebrew/privacyclaw.rb:31`.

### RC1-2: Tap formula contains source-of-truth header claiming it is the authoritative file
**Severity:** minor
**File:** homebrew-privacyclaw/Formula/privacyclaw.rb:1-19
**Finding:** The tap formula was synced from the source formula via `make tap-sync-formula` which copies the entire file including the header comment "SOURCE OF TRUTH: This file is the authoritative formula. The tap formula at homebrew-privacyclaw/Formula/privacyclaw.rb is GENERATED from this file…". This comment is self-referential and contradictory: the tap formula claims to be the source of truth while the same comment says it is generated from itself. Any developer reading the tap formula will be confused about which file to edit.
**Required fix:** In `homebrew-privacyclaw/Formula/privacyclaw.rb`, replace the header comment block (lines 3-8) with a generated-file notice: `# GENERATED FILE — do not edit directly.` / `# Source of truth: packaging/homebrew/privacyclaw.rb` / `# Regenerate via: make tap-sync-formula SHA256=<hash>`. The `tap-sync-formula` Makefile target must also be updated to perform this header substitution after the copy so future syncs produce the correct header automatically.

### RC1-3: Deprecated brew-package uses universal TARBALL variable and overwrites universal tarball with arm64-only content
**Severity:** minor
**File:** Makefile:149,192
**Finding:** The `TARBALL` variable at line 149 was updated to `/tmp/privacyclaw-$(VERSION)-universal-apple-darwin.tar.gz` for the new `tarball` target. The deprecated `brew-package` target at line 192 still uses `$(TARBALL)` and produces a tarball containing only the arm64 `privacyclaw` binary. Running `brew-package` after `tarball` overwrites the correctly-assembled universal tarball (all three artifacts) with a single-artifact arm64 file under the universal filename. The deprecation notice (line 188) does not prevent this silent corruption.
**Required fix:** In `Makefile`, define a separate variable `BREW_PACKAGE_TARBALL := /tmp/privacyclaw-$(VERSION)-arm64-apple-darwin.tar.gz` immediately before the `brew-package` target and replace all references to `$(TARBALL)` within `brew-package` with `$(BREW_PACKAGE_TARBALL)`. Update the echo lines inside `brew-package` accordingly. This restores the old arm64 filename for the deprecated target.
