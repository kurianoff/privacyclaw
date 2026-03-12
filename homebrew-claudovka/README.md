# homebrew-claudovka

Homebrew tap for [Claudovka](https://github.com/kurianoff/kladovka) — a privacy proxy for LLM API traffic.

## Install CLI (brew services managed)

```sh
brew tap kurianoff/claudovka
brew install claudovka
brew services start claudovka
```

## Install Menu Bar App

```sh
brew tap kurianoff/claudovka
brew install --cask claudovka-app
```

## Development

The formula and cask contain placeholder SHA-256 checksums (`0000...`). Update them with real values when publishing a GitHub Release:

```sh
brew fetch --force claudovka
brew fetch --force --cask claudovka-app
```

Then update `sha256` in the formula/cask with the output checksums.
