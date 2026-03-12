class Claudovka < Formula
  desc "Privacy proxy for LLM API traffic — inspect and redact sensitive data"
  homepage "https://github.com/kurianoff/kladovka"
  version "0.2.0"

  # Binary tarball URL — update when a GitHub Release is published.
  # See: https://github.com/kurianoff/kladovka/releases
  on_macos do
    if Hardware::CPU.arm?
      url "file:///tmp/claudovka-0.2.0-arm64-apple-darwin.tar.gz"
      sha256 "3e60e292388ac91fb9e0c197fe40d20c4c7c5d88371a69f3b2f2218681e915cd"
    else
      url "https://github.com/kurianoff/kladovka/releases/download/v#{version}/claudovka-#{version}-x86_64-apple-darwin.tar.gz"
      sha256 "3e60e292388ac91fb9e0c197fe40d20c4c7c5d88371a69f3b2f2218681e915cd"
    end
  end

  def install
    bin.install "claudovka"
  end

  service do
    run [opt_bin/"claudovka", "start"]
    keep_alive true
    log_path var/"log/claudovka.log"
    error_log_path var/"log/claudovka.log"
    working_dir var
    environment_variables PATH: std_service_path_env
  end

  def post_install
    # Generate CA certificate if not already present.
    system bin/"claudovka", "init" unless Pathname(ENV["HOME"]).join("Library/Application Support/claudovka/ca/ca.pem").exist?
  end

  def caveats
    <<~EOS
      To initialize the CA certificate:
        claudovka init
        claudovka init --install-ca   # trust in macOS keychain

      To enable Tier 3 standalone PII protection, create:
        ~/Library/Application Support/claudovka/config.toml

      with contents:
        [pii]
        mode = "replace"

        [pii.tiers]
        regex = false
        ner   = false
        slm   = true

        [pii.slm]
        endpoint   = "http://127.0.0.1:16442"
        timeout_ms = 5000

      T3 standalone requires a running llama-server (llama.cpp) on port 16442:
        llama-server --model /path/to/model.gguf --port 16442 --ctx-size 2048

      Then start the proxy:
        claudovka start
        # or as a background service:
        brew services start #{name}
    EOS
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/claudovka --version")
  end
end
