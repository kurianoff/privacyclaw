# typed: false
# frozen_string_literal: true

# SOURCE OF TRUTH: This file is the authoritative formula.
# The tap formula at homebrew-privacyclaw/Formula/privacyclaw.rb is
# GENERATED from this file via: make tap-sync-formula SHA256=<hash>
# Do not edit the tap formula directly.
#
# Homebrew formula for privacyclaw — a local MITM privacy proxy for LLM API traffic.
#
# To publish a real release:
#   1. Build universal binary + tarball:
#        make tarball   (produces /tmp/privacyclaw-VERSION-universal-apple-darwin.tar.gz)
#   2. Upload the tarball to GitHub Release v<VERSION>
#   3. Sync the tap formula with the real SHA-256:
#        make tap-sync-formula SHA256=<sha256-from-step-1>
#
# To install from this formula directly (before publishing):
#   brew install --formula ./packaging/homebrew/privacyclaw.rb
class Privacyclaw < Formula
  desc "Local MITM privacy proxy for LLM API traffic inspection and PII redaction"
  homepage "https://github.com/kurianoff/privacyclaw"
  version "0.2.0"

  # ── Pre-built universal binary (aarch64 + x86_64 via lipo) ────────────────
  #
  # The tarball bundles privacyclaw, llama-server, and privacyclaw-slm-sidecar.
  # Replace the placeholder SHA-256 via: make tap-sync-formula SHA256=<hash>
  #
  on_macos do
    url "https://github.com/kurianoff/kladovka/releases/download/v#{version}/privacyclaw-#{version}-universal-apple-darwin.tar.gz"
    sha256 "PLACEHOLDER_SHA256_REPLACE_BEFORE_PUBLISHING"
  end

  # ── Dependencies ──────────────────────────────────────────────────────────
  # llama-server is bundled directly in the tarball — no depends_on "llama.cpp" needed.
  depends_on "python@3.11"

  # ── Python sidecar pip resources ──────────────────────────────────────────
  # Pinned versions downloaded with:
  #   pip download --no-deps --python-version 3.11 --only-binary :all: \
  #     --platform macosx_11_0_universal2 <packages>

  resource "fastapi" do
    url "https://files.pythonhosted.org/packages/84/a4/5caa2de7f917a04ada20018eccf60d6cc6145b0199d55ca3711b0fc08312/fastapi-0.135.3-py3-none-any.whl"
    sha256 "9b0f590c813acd13d0ab43dd8494138eb58e484bfac405db1f3187cfc5810d98"
  end

  resource "uvicorn" do
    url "https://files.pythonhosted.org/packages/0a/89/f8827ccff89c1586027a105e5630ff6139a64da2515e24dafe860bd9ae4d/uvicorn-0.42.0-py3-none-any.whl"
    sha256 "96c30f5c7abe6f74ae8900a70e92b85ad6613b745d4879eb9b16ccad15645359"
  end

  resource "httpx" do
    url "https://files.pythonhosted.org/packages/2a/39/e50c7c3a983047577ee07d2a9e53faf5a69493943ec3f6a384bdc792deb2/httpx-0.28.1-py3-none-any.whl"
    sha256 "d909fcccc110f8c7faf814ca82a9a4d816bc5a6dbfea25d6591d6985b8ba59ad"
  end

  resource "pydantic" do
    url "https://files.pythonhosted.org/packages/5a/87/b70ad306ebb6f9b585f114d0ac2137d792b48be34d732d60e597c2f8465a/pydantic-2.12.5-py3-none-any.whl"
    sha256 "e561593fccf61e8a20fc46dfc2dfe075b8be7d0188df33f221ad1f0139180f9d"
  end

  resource "starlette" do
    url "https://files.pythonhosted.org/packages/0b/c9/584bc9651441b4ba60cc4d557d8a547b5aff901af35bda3a4ee30c819b82/starlette-1.0.0-py3-none-any.whl"
    sha256 "d3ec55e0bb321692d275455ddfd3df75fff145d009685eb40dc91fc66b03d38b"
  end

  resource "anyio" do
    url "https://files.pythonhosted.org/packages/da/42/e921fccf5015463e32a3cf6ee7f980a6ed0f395ceeaa45060b61d86486c2/anyio-4.13.0-py3-none-any.whl"
    sha256 "08b310f9e24a9594186fd75b4f73f4a4152069e3853f1ed8bfbf58369f4ad708"
  end

  resource "sniffio" do
    url "https://files.pythonhosted.org/packages/e9/44/75a9c9421471a6c4805dbf2356f7c181a29c1879239abab1ea2cc8f38b40/sniffio-1.3.1-py3-none-any.whl"
    sha256 "2f6da418d1f1e0fddd844478f41680e794e6051915791a034ff65e5f100525a2"
  end

  resource "httpcore" do
    url "https://files.pythonhosted.org/packages/7e/f5/f66802a942d491edb555dd61e3a9961140fd64c90bce1eafd741609d334d/httpcore-1.0.9-py3-none-any.whl"
    sha256 "2d400746a40668fc9dec9810239072b40b4484b640a8c38fd654a024c7a1bf55"
  end

  resource "h11" do
    url "https://files.pythonhosted.org/packages/04/4b/29cac41a4d98d144bf5f6d33995617b185d14b22401f75ca86f384e87ff1/h11-0.16.0-py3-none-any.whl"
    sha256 "63cf8bbe7522de3bf65932fda1d9c2772064ffb3dae62d55932da54b31cb6c86"
  end

  resource "certifi" do
    url "https://files.pythonhosted.org/packages/9a/3c/c17fb3ca2d9c3acff52e30b309f538586f9f5b9c9cf454f3845fc9af4881/certifi-2026.2.25-py3-none-any.whl"
    sha256 "027692e4402ad994f1c42e52a4997a9763c646b73e4096e4d5d6db8af1d6f0fa"
  end

  resource "idna" do
    url "https://files.pythonhosted.org/packages/0e/61/66938bbb5fc52dbdf84594873d5b51fb1f7c7794e9c0f5bd885f30bc507b/idna-3.11-py3-none-any.whl"
    sha256 "771a87f49d9defaf64091e6e6fe9c18d4833f140bd19464795bc32d966ca37ea"
  end

  resource "click" do
    url "https://files.pythonhosted.org/packages/98/78/01c019cdb5d6498122777c1a43056ebb3ebfeef2076d9d026bfe15583b2b/click-8.3.1-py3-none-any.whl"
    sha256 "981153a64e25f12d547d3426c367a4857371575ee7ad18df2a6183ab0545b2a6"
  end

  resource "annotated-types" do
    url "https://files.pythonhosted.org/packages/78/b6/6307fbef88d9b5ee7421e68d78a9f162e0da4900bc5f5793f6d3d0e34fb8/annotated_types-0.7.0-py3-none-any.whl"
    sha256 "1f02e8b43a8fbbc3f3e0d4f0f4bfc8131bcb4eebe8849b8e5c773f3a1c582a53"
  end

  # typing-extensions: single resource block (highest version satisfying fastapi>=4.0 and pydantic>=4.6).
  resource "typing-extensions" do
    url "https://files.pythonhosted.org/packages/18/67/36e9267722cc04a6b9f15c7f3441c2363321a3ea07da7ae0c0707beb2a9c/typing_extensions-4.15.0-py3-none-any.whl"
    sha256 "f0fa19c6845758ab08074a0cfa8b7aecb71c999ca73d62883bc25cc018c4e548"
  end

  # pydantic-core: platform-specific binary wheels (requires rustc to build from sdist).
  # x86_64 fallback: no macosx_11_0_x86_64 wheel available; using macosx_10_12_x86_64 (next lowest tag).
  on_macos do
    on_arm do
      resource "pydantic-core" do
        url "https://files.pythonhosted.org/packages/2f/b4/18092255f64392d1604cef8751a552f4c1fa0816a8b2f7120ad9896d2ecd/pydantic_core-2.45.0-cp311-cp311-macosx_11_0_arm64.whl"
        sha256 "2d4a9ad579a2a3c5f64f0a610fb2aa70a40abd9fffb63de5c6811bb276f1ed66"
      end
    end
    on_intel do
      resource "pydantic-core" do
        url "https://files.pythonhosted.org/packages/ca/a3/3b8822ca7abbaf829cea6c802f705d0f6cf0703cca2498255ef40906f064/pydantic_core-2.45.0-cp311-cp311-macosx_10_12_x86_64.whl"
        sha256 "13078af99248af0430ec752ac3f5fed1477ee8cd833b4f55563c458b9d72d9bf"
      end
    end
  end

  # ── Install ───────────────────────────────────────────────────────────────

  def install
    bin.install "privacyclaw"
    bin.install "llama-server"
    virtualenv_install_with_resources using: "python@3.11"
    bin.install "privacyclaw-slm-sidecar"
    inreplace bin/"privacyclaw-slm-sidecar",
              /\A#!.+\n/,
              "#!#{opt_libexec}/bin/python3\n"
  end

  # ── Post-install message ──────────────────────────────────────────────────

  def caveats
    <<~EOS
      To finish setup, generate the local CA certificate:

        privacyclaw init

      Then start the proxy:

        privacyclaw start

      The dashboard is available at: http://localhost:16443

      To configure your LLM clients, set:

        export HTTPS_PROXY=http://127.0.0.1:16440

      For transparent network-level interception (intercepts without proxy env var):

        privacyclaw network-enable   # requires admin credentials
        privacyclaw network-start

      To uninstall:

        privacyclaw uninstall
        brew uninstall privacyclaw

      T3 PII pipeline (SLM sidecar):

        On first run with T3 enabled, privacyclaw auto-downloads the smollm2-135m
        model (~135 MB). To manage models manually:

          privacyclaw models install <model-name>

        The Python sidecar is available for advanced or debugging use:

          privacyclaw-slm-sidecar --version
          SIDECAR_PORT=16442 privacyclaw-slm-sidecar
    EOS
  end

  # ── LaunchAgent (optional auto-start) ────────────────────────────────────

  service do
    run [opt_bin/"privacyclaw", "start"]
    keep_alive true
    log_path var/"log/privacyclaw.log"
    error_log_path var/"log/privacyclaw.log"
    environment_variables PATH: std_service_path_env
  end

  # ── Tests ─────────────────────────────────────────────────────────────────

  test do
    # Verify the binary runs and reports the expected version.
    assert_match version.to_s, shell_output("#{bin}/privacyclaw --version")

    # Verify PII detection works (no CA or network required).
    output = shell_output("#{bin}/privacyclaw test-pii 'Call me at 555-867-5309'")
    assert_match(/phone/i, output)

    # Verify init creates CA files.
    system "#{bin}/privacyclaw", "init"
    ca_dir = Pathname.new(ENV["HOME"]) / ".config/privacyclaw/ca"
    assert_predicate ca_dir / "ca.pem",     :exist?
    assert_predicate ca_dir / "ca.key.pem", :exist?

    # Verify ca-path returns a path to an existing .pem file.
    ca_path = shell_output("#{bin}/privacyclaw ca-path").strip
    assert_match(/\.pem$/, ca_path)
    assert_predicate Pathname.new(ca_path), :exist?

    # Verify sidecar script is installed and self-identifies.
    assert_match "privacyclaw-slm-sidecar 0.1.0",
                 shell_output("#{bin}/privacyclaw-slm-sidecar --version")
  end
end
