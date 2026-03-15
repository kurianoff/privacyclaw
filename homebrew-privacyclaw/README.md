# homebrew-privacyclaw

Homebrew tap for [Privacyclaw](https://github.com/kurianoff/privacyclaw) — a privacy proxy for LLM API traffic.

## Install CLI (brew services managed)

```sh
brew tap kurianoff/privacyclaw
brew install privacyclaw
brew services start privacyclaw
```

## Install Menu Bar App

```sh
brew tap kurianoff/privacyclaw
brew install --cask privacyclaw-app
```

## Development

The formula and cask contain placeholder SHA-256 checksums (`0000...`). Update them with real values when publishing a GitHub Release:

```sh
brew fetch --force privacyclaw
brew fetch --force --cask privacyclaw-app
```

Then update `sha256` in the formula/cask with the output checksums.
