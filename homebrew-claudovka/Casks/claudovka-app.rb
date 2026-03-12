cask "claudovka-app" do
  version "0.2.0"

  # DMG URL — update when a GitHub Release is published.
  on_arm do
    url "https://github.com/kurianoff/kladovka/releases/download/v#{version}/Claudovka-#{version}-aarch64.dmg"
    sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  end
  on_intel do
    url "https://github.com/kurianoff/kladovka/releases/download/v#{version}/Claudovka-#{version}-x86_64.dmg"
    sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  end

  name "Claudovka"
  desc "Privacy proxy for LLM API traffic"
  homepage "https://github.com/kurianoff/kladovka"

  depends_on macos: ">= :ventura"

  app "Claudovka.app"

  # After installing the .app, link the CLI binary from the bundle.
  binary "#{appdir}/Claudovka.app/Contents/MacOS/claudovka"

  postflight do
    system_command "#{appdir}/Claudovka.app/Contents/MacOS/claudovka",
      args: ["init"],
      sudo: false
  end

  uninstall quit: "com.claudovka.app"

  zap trash: [
    "~/Library/Application Support/claudovka",
    "~/Library/Logs/claudovka",
  ]
end
