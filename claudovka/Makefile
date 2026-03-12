VERSION := $(shell cargo metadata --no-deps --format-version 1 | python3 -c "import json,sys; print(json.load(sys.stdin)['packages'][0]['version'])" 2>/dev/null || echo "0.0.0")
ARCH    := $(shell uname -m)
DIST    := dist

.PHONY: all build release test clean app pkg dmg tap-update-version tap-audit

all: build

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
	  target/aarch64-apple-darwin/release/claudovka \
	  target/x86_64-apple-darwin/release/claudovka \
	  -output $(DIST)/claudovka
	@echo "Universal binary: $(DIST)/claudovka"

# App/pkg builds include the tray feature so the menu bar icon is compiled in.
release-app:
	cargo build --release --target aarch64-apple-darwin --features tray
	cargo build --release --target x86_64-apple-darwin --features tray
	mkdir -p $(DIST)
	lipo -create \
	  target/aarch64-apple-darwin/release/claudovka \
	  target/x86_64-apple-darwin/release/claudovka \
	  -output $(DIST)/claudovka
	@echo "Universal binary (tray): $(DIST)/claudovka"

# ── macOS .app bundle ─────────────────────────────────────────────────────────

APP_DIR := $(DIST)/Claudovka.app
APP_BIN := $(APP_DIR)/Contents/MacOS

app: release-app
	mkdir -p $(APP_BIN) $(APP_DIR)/Contents/Resources
	cp $(DIST)/claudovka $(APP_BIN)/claudovka
	# Launch script: start proxy in menu-bar (tray) mode — no Dock icon
	printf '#!/bin/bash\nexec "$$(dirname "$$0")/claudovka" start --tray\n' > $(APP_BIN)/claudovka-app
	chmod +x $(APP_BIN)/claudovka-app
	# Info.plist (printf avoids heredoc issues with make's parser)
	printf '%s\n' \
	  '<?xml version="1.0" encoding="UTF-8"?>' \
	  '<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
	  '<plist version="1.0"><dict>' \
	  '  <key>CFBundleIdentifier</key><string>com.claudovka.app</string>' \
	  '  <key>CFBundleName</key><string>Claudovka</string>' \
	  "  <key>CFBundleVersion</key><string>$(VERSION)</string>" \
	  "  <key>CFBundleShortVersionString</key><string>$(VERSION)</string>" \
	  '  <key>LSUIElement</key><true/>' \
	  '  <key>LSMinimumSystemVersion</key><string>13.0</string>' \
	  '  <key>CFBundleExecutable</key><string>claudovka-app</string>' \
	  '  <key>CFBundlePackageType</key><string>APPL</string>' \
	  '</dict></plist>' \
	  > $(APP_DIR)/Contents/Info.plist
	@echo "App bundle: $(APP_DIR)"

# ── macOS .pkg installer ──────────────────────────────────────────────────────

PKG_ROOT    := $(DIST)/pkg-root
PKG_SCRIPTS := $(DIST)/pkg-scripts
PKG_NAME    := claudovka-$(VERSION).pkg
SHARE_DIR   := $(PKG_ROOT)/usr/local/share/claudovka

# Shared pkg layout step (binary must already be at $(DIST)/claudovka).
_pkg-layout:
	rm -rf $(PKG_ROOT) $(PKG_SCRIPTS) $(DIST)/$(PKG_NAME)
	mkdir -p $(PKG_ROOT)/usr/local/bin $(SHARE_DIR) $(PKG_SCRIPTS)
	cp $(DIST)/claudovka $(PKG_ROOT)/usr/local/bin/claudovka
	cp packaging/com.claudovka.proxy.plist $(SHARE_DIR)/
	cp packaging/com.claudovka.pf.plist    $(SHARE_DIR)/
	cp packaging/postinstall $(PKG_SCRIPTS)/postinstall
	cp packaging/preremove   $(PKG_SCRIPTS)/preremove
	chmod +x $(PKG_SCRIPTS)/postinstall $(PKG_SCRIPTS)/preremove
	pkgbuild \
	  --root $(PKG_ROOT) \
	  --scripts $(PKG_SCRIPTS) \
	  --identifier com.claudovka.pkg \
	  --version $(VERSION) \
	  --install-location / \
	  $(DIST)/$(PKG_NAME)
	@echo "Package: $(DIST)/$(PKG_NAME)"

# Universal binary (arm64 + x86_64) for distribution.
pkg: release-app
	$(MAKE) _pkg-layout

# Current-architecture release build — faster, for local testing.
pkg-local:
	cargo build --release --features tray
	mkdir -p $(DIST)
	cp target/release/claudovka $(DIST)/claudovka
	$(MAKE) _pkg-layout

# ── .dmg ─────────────────────────────────────────────────────────────────────

DMG_NAME := claudovka-$(VERSION).dmg

dmg: app
	hdiutil create -volname Claudovka \
	  -srcfolder $(APP_DIR) \
	  -ov -format UDZO \
	  $(DIST)/$(DMG_NAME)
	@echo "DMG: $(DIST)/$(DMG_NAME)"

# ── Homebrew tap files ────────────────────────────────────────────────────────

# Update version placeholder in tap files (run after bumping Cargo.toml version).
tap-update-version:
	sed -i '' 's/version "[0-9]*\.[0-9]*\.[0-9]*"/version "$(VERSION)"/' \
	  ../homebrew-claudovka/Formula/claudovka.rb \
	  ../homebrew-claudovka/Casks/claudovka-app.rb

# Validate tap formula syntax (requires brew).
tap-audit:
	brew audit --strict ../homebrew-claudovka/Formula/claudovka.rb || true
	brew audit --cask ../homebrew-claudovka/Casks/claudovka-app.rb || true

# ── Clean ─────────────────────────────────────────────────────────────────────

clean:
	cargo clean
	rm -rf $(DIST)
