class Privacyclaw < Formula
  desc "Privacy proxy for LLM API traffic — inspect and redact sensitive data"
  homepage "https://github.com/kurianoff/kladovka"
  version "0.3.0"

  # Binary tarball URL — update when a GitHub Release is published.
  # See: https://github.com/kurianoff/kladovka/releases
  on_macos do
    if Hardware::CPU.arm?
      url "file:///tmp/privacyclaw-0.3.0-aarch64-apple-darwin.tar.gz"
      sha256 "dc679ef60ecf0791fd1b5409fbc1f9080f4df9f3de62962fc853d38848c332c8"
    else
      url "https://github.com/kurianoff/kladovka/releases/download/v#{version}/privacyclaw-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "3e60e292388ac91fb9e0c197fe40d20c4c7c5d88371a69f3b2f2218681e915cd"
    end
  end

  depends_on "llama.cpp"

  def install
    bin.install "privacyclaw"
  end

  service do
    run [opt_bin/"privacyclaw", "start"]
    keep_alive true
    log_path var/"log/privacyclaw.log"
    error_log_path var/"log/privacyclaw.log"
    working_dir var
    environment_variables PATH: std_service_path_env
  end

  def post_install
    # Generate CA certificate if not already present.
    system bin/"privacyclaw", "init" unless Pathname(ENV["HOME"]).join("Library/Application Support/privacyclaw/ca/ca.pem").exist?
  end

  def caveats
    <<~EOS
      ── First-time setup ──────────────────────────────────────────
      Generate and trust the local CA certificate:
        privacyclaw init
        privacyclaw init --install-ca   # adds to macOS keychain

      ── Tier 3 standalone PII protection ──────────────────────────
      llama-server (llama.cpp) is included with this install.
      You only need a GGUF model file. Recommended:
        Phi-3-mini-4k-instruct.Q4_K_M.gguf  (~2.2 GB, fast)
        Mistral-7B-Instruct-v0.3.Q4_K_M.gguf (~4.1 GB, more accurate)

      Download from https://huggingface.co or use:
        brew install huggingface-cli
        huggingface-cli download microsoft/Phi-3-mini-4k-instruct-gguf \
          Phi-3-mini-4k-instruct-q4.gguf --local-dir ~/Library/Application\ Support/privacyclaw/models/

      Then create ~/Library/Application Support/privacyclaw/config.toml:
        [pii]
        mode = "replace"

        [pii.tiers]
        regex = false
        ner   = false
        slm   = true

        [pii.slm]
        endpoint   = "http://127.0.0.1:16442"
        timeout_ms = 5000

      Start the SLM sidecar:
        llama-server --model ~/Library/Application\ Support/privacyclaw/models/Phi-3-mini-4k-instruct-q4.gguf \
          --port 16442 --ctx-size 2048

      Start the proxy:
        privacyclaw start
        # or as a background service:
        brew services start #{name}

      Point your LLM tool at the proxy:
        export HTTPS_PROXY=http://127.0.0.1:16440
        export HTTP_PROXY=http://127.0.0.1:16440
        export NODE_EXTRA_CA_CERTS="$HOME/Library/Application Support/privacyclaw/ca/ca.pem"
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/privacyclaw --version")
  end
end
