VERSION := $(shell cargo metadata --no-deps --format-version 1 | python3 -c "import json,sys; print(json.load(sys.stdin)['packages'][0]['version'])" 2>/dev/null || echo "0.0.0")
ARCH    := $(shell uname -m)
DIST    := dist

# Pinned llama.cpp release to bundle with the tarball.
# To update: change this tag, then run: make tarball
LLAMA_CPP_TAG ?= b5000

# llama.cpp GitHub release asset base URL
LLAMA_RELEASE_BASE := https://github.com/ggerganov/llama.cpp/releases/download/$(LLAMA_CPP_TAG)

.PHONY: all build release test clean app pkg dmg tap-update-version tap-audit brew-package print-llama-tag

all: build

print-llama-tag:
	@echo $(LLAMA_CPP_TAG)

# ── Development build ─────────────────────────────────────────────────────────

build:
	cargo build

test:
	cargo test

# ── Release build ─────────────────────────────────────────────────────────────

release:
	cargo build --release --target aarch64-apple-darwin
	cargo build --release --target x86_64-apple-darwin
	mkdir -p $(DIST)
	lipo -create \
	  target/aarch64-apple-darwin/release/privacyclaw \
	  target/x86_64-apple-darwin/release/privacyclaw \
	  -output $(DIST)/privacyclaw
	@echo "Universal binary: $(DIST)/privacyclaw"

# App/pkg builds include the tray feature so the menu bar icon is compiled in.
release-app:
	cargo build --release --target aarch64-apple-darwin --features tray
	cargo build --release --target x86_64-apple-darwin --features tray
	mkdir -p $(DIST)
	lipo -create \
	  target/aarch64-apple-darwin/release/privacyclaw \
	  target/x86_64-apple-darwin/release/privacyclaw \
	  -output $(DIST)/privacyclaw
	@echo "Universal binary (tray): $(DIST)/privacyclaw"

# ── macOS .app bundle ─────────────────────────────────────────────────────────

APP_DIR := $(DIST)/Privacyclaw.app
APP_BIN := $(APP_DIR)/Contents/MacOS

app: release-app
	mkdir -p $(APP_BIN) $(APP_DIR)/Contents/Resources
	cp $(DIST)/privacyclaw $(APP_BIN)/privacyclaw
	# Launch script: start proxy in menu-bar (tray) mode — no Dock icon
	printf '#!/bin/bash\nexec "$$(dirname "$$0")/privacyclaw" start --tray\n' > $(APP_BIN)/privacyclaw-app
	chmod +x $(APP_BIN)/privacyclaw-app
	# Info.plist (printf avoids heredoc issues with make's parser)
	printf '%s\n' \
	  '<?xml version="1.0" encoding="UTF-8"?>' \
	  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
	  '<plist version="1.0"><dict>' \
	  '  <key>CFBundleIdentifier</key><string>com.privacyclaw.app</string>' \
	  '  <key>CFBundleName</key><string>Privacyclaw</string>' \
	  "  <key>CFBundleVersion</key><string>$(VERSION)</string>" \
	  "  <key>CFBundleShortVersionString</key><string>$(VERSION)</string>" \
	  '  <key>LSUIElement</key><true/>' \
	  '  <key>LSMinimumSystemVersion</key><string>13.0</string>' \
	  '  <key>CFBundleExecutable</key><string>privacyclaw-app</string>' \
	  '  <key>CFBundlePackageType</key><string>APPL</string>' \
	  '</dict></plist>' \
	  > $(APP_DIR)/Contents/Info.plist
	@echo "App bundle: $(APP_DIR)"

# ── macOS .pkg installer ──────────────────────────────────────────────────────

PKG_ROOT    := $(DIST)/pkg-root
PKG_SCRIPTS := $(DIST)/pkg-scripts
PKG_NAME    := privacyclaw-$(VERSION).pkg
SHARE_DIR   := $(PKG_ROOT)/usr/local/share/privacyclaw

# LLAMA_SERVER — path to the llama-server binary to bundle into the .pkg.
# Defaults to the Homebrew-installed copy; override on the command line:
#   make pkg LLAMA_SERVER=/path/to/llama-server
LLAMA_SERVER ?= $(shell brew --prefix llama.cpp 2>/dev/null)/bin/llama-server

# Shared pkg layout step (binary must already be at $(DIST)/privacyclaw,
# and $(APP_DIR) must already be built).
_pkg-layout:
	rm -rf $(PKG_ROOT) $(PKG_SCRIPTS) $(DIST)/$(PKG_NAME)
	mkdir -p $(PKG_ROOT)/usr/local/bin $(PKG_ROOT)/Applications $(SHARE_DIR) $(PKG_SCRIPTS)
	cp $(DIST)/privacyclaw $(PKG_ROOT)/usr/local/bin/privacyclaw
	cp -r $(APP_DIR) $(PKG_ROOT)/Applications/
	cp packaging/com.privacyclaw.proxy.plist $(SHARE_DIR)/
	cp packaging/com.privacyclaw.pf.plist    $(SHARE_DIR)/
	@if [ -f "$(LLAMA_SERVER)" ]; then \
	  cp "$(LLAMA_SERVER)" $(SHARE_DIR)/llama-server; \
	  echo "Bundled llama-server from $(LLAMA_SERVER)"; \
	else \
	  echo "WARN: llama-server not found at $(LLAMA_SERVER) — Tier 3 PII will not be bundled"; \
	fi
	cp packaging/postinstall $(PKG_SCRIPTS)/postinstall
	cp packaging/preremove   $(PKG_SCRIPTS)/preremove
	chmod +x $(PKG_SCRIPTS)/postinstall $(PKG_SCRIPTS)/preremove
	pkgbuild \
	  --root $(PKG_ROOT) \
	  --scripts $(PKG_SCRIPTS) \
	  --identifier com.privacyclaw.pkg \
	  --version $(VERSION) \
	  --install-location / \
	  $(DIST)/$(PKG_NAME)
	@echo "Package: $(DIST)/$(PKG_NAME)"

# Universal binary (arm64 + x86_64) for distribution.
# Builds .app bundle first so it is included in the pkg at /Applications/.
pkg: app
	$(MAKE) _pkg-layout

# Current-architecture release build — faster, for local testing.
pkg-local:
	cargo build --release --features tray
	mkdir -p $(DIST)
	cp target/release/privacyclaw $(DIST)/privacyclaw
	$(MAKE) _pkg-layout

# ── .dmg ─────────────────────────────────────────────────────────────────────

DMG_NAME := privacyclaw-$(VERSION).dmg

dmg: app
	hdiutil create -volname Privacyclaw \
	  -srcfolder $(APP_DIR) \
	  -ov -format UDZO \
	  $(DIST)/$(DMG_NAME)
	@echo "DMG: $(DIST)/$(DMG_NAME)"

# ── Homebrew tarball + SHA ────────────────────────────────────────────────────

TARBALL := /tmp/privacyclaw-$(VERSION)-universal-apple-darwin.tar.gz

.PHONY: tarball
tarball: release
	mkdir -p $(DIST)/llama-tmp
	curl -fL "$(LLAMA_RELEASE_BASE)/llama-$(LLAMA_CPP_TAG)-bin-macos-arm64.zip" \
	    -o $(DIST)/llama-tmp/llama-arm64.zip
	unzip -o $(DIST)/llama-tmp/llama-arm64.zip -d $(DIST)/llama-tmp/arm64/
	curl -fL "$(LLAMA_RELEASE_BASE)/llama-$(LLAMA_CPP_TAG)-bin-macos-x86_64.zip" \
	    -o $(DIST)/llama-tmp/llama-x86.zip
	unzip -o $(DIST)/llama-tmp/llama-x86.zip -d $(DIST)/llama-tmp/x86_64/
	@# Verify exactly one llama-server extracted per arch
	@ARM_COUNT=$$(find $(DIST)/llama-tmp/arm64 -name 'llama-server' -type f | wc -l | tr -d ' '); \
	 X86_COUNT=$$(find $(DIST)/llama-tmp/x86_64 -name 'llama-server' -type f | wc -l | tr -d ' '); \
	 [ "$$ARM_COUNT" = "1" ] || (echo "ERROR: expected 1 llama-server in arm64 zip, found $$ARM_COUNT"; exit 1); \
	 [ "$$X86_COUNT" = "1" ] || (echo "ERROR: expected 1 llama-server in x86_64 zip, found $$X86_COUNT"; exit 1)
	lipo -create \
	    $$(find $(DIST)/llama-tmp/arm64 -name 'llama-server' -type f) \
	    $$(find $(DIST)/llama-tmp/x86_64 -name 'llama-server' -type f) \
	    -output $(DIST)/llama-server
	@# Verify universal binary
	@file $(DIST)/llama-server | grep -q 'universal binary' || \
	    (echo "ERROR: llama-server is not a universal binary"; exit 1)
	chmod +x $(DIST)/llama-server
	cp packaging/privacyclaw-slm-sidecar $(DIST)/privacyclaw-slm-sidecar
	chmod +x $(DIST)/privacyclaw-slm-sidecar
	tar -czf $(TARBALL) -C $(DIST) privacyclaw llama-server privacyclaw-slm-sidecar
	@echo "Tarball: $(TARBALL)"
	@echo "SHA256:  $$(shasum -a 256 $(TARBALL) | awk '{print $$1}')"
	@echo ""
	@echo "Next steps:"
	@echo "  1. Upload $(TARBALL) to GitHub Release v$(VERSION)"
	@echo "  2. Run: make tap-sync-formula SHA256=<above>"

# DEPRECATED: use `make tarball` instead.
# brew-package produces a single-arch arm64-only tarball of privacyclaw only.
# It is retained for backward compatibility and will be removed in a future
# major version.
brew-package:
	@echo "DEPRECATION NOTICE: brew-package is deprecated. Use 'make tarball' instead."
	cargo build --release --target aarch64-apple-darwin
	mkdir -p $(DIST)
	cp target/aarch64-apple-darwin/release/privacyclaw $(DIST)/privacyclaw
	tar -czf $(TARBALL) -C $(DIST) privacyclaw
	@echo "Tarball: $(TARBALL)"
	@echo "SHA256:  $$(shasum -a 256 $(TARBALL) | awk '{print $$1}')"
	@echo "Update homebrew-privacyclaw/Formula/privacyclaw.rb with the SHA above."

# ── Homebrew tap files ────────────────────────────────────────────────────────

# Update version in tap cask (run after bumping Cargo.toml version).
# NOTE: The tap FORMULA is now fully regenerated by tap-sync-formula, not
# patched by this target. tap-update-version still patches the cask.
tap-update-version:
	sed -i '' 's/version "[0-9]*\.[0-9]*\.[0-9]*"/version "$(VERSION)"/' \
	  homebrew-privacyclaw/Casks/privacyclaw-app.rb

# Validate tap formula syntax (requires brew).
tap-audit:
	brew audit --strict homebrew-privacyclaw/Formula/privacyclaw.rb || true
	brew audit --cask homebrew-privacyclaw/Casks/privacyclaw-app.rb || true

# ── Clean ─────────────────────────────────────────────────────────────────────

clean:
	cargo clean
	rm -rf $(DIST)
