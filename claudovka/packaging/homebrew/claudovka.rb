# typed: false
# frozen_string_literal: true

# Homebrew formula for claudovka — a local MITM privacy proxy for LLM API traffic.
#
# To publish a real release:
#   1. Build the universal binary:
#        make release   (produces dist/claudovka)
#   2. Package it:
#        tar czf claudovka-VERSION-universal-macos.tar.gz -C dist claudovka
#   3. Compute SHA-256:
#        shasum -a 256 claudovka-VERSION-universal-macos.tar.gz
#   4. Update the `url` and `sha256` fields below.
#   5. Submit to a tap or open a PR against homebrew-core.
#
# To install from this formula directly (before publishing):
#   brew install --formula ./packaging/homebrew/claudovka.rb
class Claudovka < Formula
  desc "Local MITM privacy proxy for LLM API traffic inspection and PII redaction"
  homepage "https://github.com/kurianoff/claudovka"
  version "0.2.0"

  # ── Pre-built universal binary (aarch64 + x86_64 via lipo) ────────────────
  #
  # Replace the placeholder URL and sha256 with the values from the actual
  # GitHub release once the binary is published.
  #
  on_macos do
    url "https://github.com/kurianoff/claudovka/releases/download/v#{version}/claudovka-#{version}-universal-macos.tar.gz"
    sha256 "PLACEHOLDER_SHA256_REPLACE_BEFORE_PUBLISHING"
  end

  # ── Dependencies ──────────────────────────────────────────────────────────
  # llama.cpp provides llama-server for Tier 3 standalone PII protection.
  depends_on "llama.cpp"

  # ── Install ───────────────────────────────────────────────────────────────

  def install
    bin.install "claudovka"
  end

  # ── Post-install message ──────────────────────────────────────────────────

  def caveats
    <<~EOS
      To finish setup, generate the local CA certificate:

        claudovka init

      Then start the proxy:

        claudovka start

      The dashboard is available at: http://localhost:16443

      To configure your LLM clients, set:

        export HTTPS_PROXY=http://127.0.0.1:16440

      For transparent network-level interception (intercepts without proxy env var):

        claudovka network-enable   # requires admin credentials
        claudovka network-start

      To uninstall:

        claudovka uninstall
        brew uninstall claudovka
    EOS
  end

  # ── LaunchAgent (optional auto-start) ────────────────────────────────────

  service do
    run [opt_bin/"claudovka", "start"]
    keep_alive true
    log_path var/"log/claudovka.log"
    error_log_path var/"log/claudovka.log"
    environment_variables PATH: std_service_path_env
  end

  # ── Tests ─────────────────────────────────────────────────────────────────

  test do
    # Verify the binary runs and reports the expected version.
    assert_match version.to_s, shell_output("#{bin}/claudovka --version")

    # Verify PII detection works (no CA or network required).
    output = shell_output("#{bin}/claudovka test-pii 'Call me at 555-867-5309'")
    assert_match(/phone/i, output)

    # Verify init creates CA files.
    system "#{bin}/claudovka", "init"
    ca_dir = Pathname.new(ENV["HOME"]) / ".config/claudovka/ca"
    assert_predicate ca_dir / "ca.pem",     :exist?
    assert_predicate ca_dir / "ca.key.pem", :exist?

    # Verify ca-path returns a path to an existing .pem file.
    ca_path = shell_output("#{bin}/claudovka ca-path").strip
    assert_match(/\.pem$/, ca_path)
    assert_predicate Pathname.new(ca_path), :exist?
  end
end
