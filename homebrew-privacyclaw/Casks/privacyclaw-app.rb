cask "privacyclaw-app" do
  version "0.2.2"

  # DMG URL — update when a GitHub Release is published.
  on_arm do
    url "https://github.com/kurianoff/kladovka/releases/download/v#{version}/Privacyclaw-#{version}-aarch64.dmg"
    sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  end
  on_intel do
    url "https://github.com/kurianoff/kladovka/releases/download/v#{version}/Privacyclaw-#{version}-x86_64.dmg"
    sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  end

  name "Privacyclaw"
  desc "Privacy proxy for LLM API traffic"
  homepage "https://github.com/kurianoff/kladovka"

  depends_on macos: ">= :ventura"

  app "Privacyclaw.app"

  # After installing the .app, link the CLI binary from the bundle.
  binary "#{appdir}/Privacyclaw.app/Contents/MacOS/privacyclaw"

  postflight do
    system_command "#{appdir}/Privacyclaw.app/Contents/MacOS/privacyclaw",
      args: ["init"],
      sudo: false
  end

  uninstall quit: "com.privacyclaw.app"

  zap trash: [
    "~/Library/Application Support/privacyclaw",
    "~/Library/Logs/privacyclaw",
  ]
end
