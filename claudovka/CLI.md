# Claudovka CLI Reference

## Global flags

| Flag | Type | Description |
|---|---|---|
| `--config <PATH>` | path | Path to config file (default: `~/Library/Application Support/claudovka/config.toml`) |
| `--log-file <PATH>` | path | Override log file path; empty string disables file logging |

---

## Commands

### `claudovka init`

Generate the local CA certificate and print setup instructions.

| Flag | Description |
|---|---|
| `--install-ca` | Attempt to install the CA into the OS trust store (macOS Keychain) |

```bash
claudovka init
claudovka init --install-ca
```

---

### `claudovka start`

Start the proxy. Without `--mode`, respects `network_proxy.enabled` in config for the network proxy; dashboard and HTTP proxy always start.

| Flag | Values | Default | Description |
|---|---|---|---|
| `--mode` | `http` \| `network` \| `all` | *(config-driven)* | Which proxy mode to run |
| `--pii` | — | off | Enable PII replace mode (Tier 1 + Tier 2) |
| `--tray` | — | off | Show menu bar icon instead of blocking in terminal (macOS + tray feature) |

| `--mode` value | What starts |
|---|---|
| `http` | HTTP CONNECT proxy + dashboard |
| `network` | Transparent network proxy only |
| `all` | Both HTTP and network proxies |
| *(omitted)* | HTTP always; network only if `network_proxy.enabled = true` in config |

```bash
claudovka start                     # HTTP + dashboard (network if configured)
claudovka start --mode http         # HTTP CONNECT proxy + dashboard only
claudovka start --mode network      # Transparent network proxy only
claudovka start --mode all          # Both proxies simultaneously
claudovka start --pii               # Enable PII replace mode
claudovka start --tray              # macOS menu bar icon (tray build only)
```

---

### `claudovka stop`

Stop a running claudovka proxy by reading the PID file.

```bash
claudovka stop
```

---

### `claudovka config`

Manage claudovka configuration. With no flags or subcommands, launches the interactive wizard.

| Flag / Subcommand | Values | Description |
|---|---|---|
| *(none)* | — | Launch interactive configuration wizard |
| `show` | — | Print current config as TOML |
| `set <KEY=VALUE>` | dotted key | Set a single config key (type-inferred: bool / int / string) |
| `--protection-level <LEVEL>` | see below | Apply a preset tier configuration non-interactively |
| `--model <MODEL>` | catalog ID or file path | GGUF model to use for levels `3` and `intelligent` (default: phi3-mini) |

**Protection levels**

| Level | `pii.mode` | Tiers enabled | Model required? |
|---|---|---|---|
| `off` | `off` | none | No |
| `detect` | `detect-only` | T1 regex | No |
| `1` | `replace` | T1 regex | No |
| `2` | `replace` | T1 regex + T2 NER | No |
| `3` | `replace` | T1 + T2 + T3 SLM (full pipeline) | Yes (downloads if absent) |
| `intelligent` | `replace` | T3 SLM standalone only | Yes (downloads if absent) |

```bash
claudovka config                                   # interactive wizard
claudovka config show                              # print current config
claudovka config set pii.mode=replace              # set a key directly
claudovka config set pii.vault_ttl_hours=48
claudovka config --protection-level off            # disable PII
claudovka config --protection-level detect         # detect-only mode
claudovka config --protection-level 1              # regex replacement
claudovka config --protection-level 2              # regex + NER
claudovka config --protection-level 3              # full pipeline (downloads model)
claudovka config --protection-level intelligent    # T3 standalone (downloads model)
claudovka config --protection-level intelligent --model mistral-7b
claudovka config --protection-level 3 --model /path/to/model.gguf
```

After setting level `3` or `intelligent`, run `claudovka start` — it launches `llama-server` automatically using the configured model.

---

### `claudovka init`

Generate CA certificate and optionally install it in the system trust store.

*(See top of Commands section.)*

---

### `claudovka test-pii`

Test PII detection on a text string without starting the proxy.

| Argument / Flag | Description |
|---|---|
| `<TEXT>` | Text to analyze (positional, required) |
| `--locale <LOCALE>` | Locale for locale-specific patterns (e.g. `en-US`, `in-IN`, `br-BR`) |
| `--format <FORMAT>` | Output format: `text` (default) or `json` |

```bash
claudovka test-pii "My email is alice@example.com"
claudovka test-pii --locale en-GB "NI: AB 12 34 56 C"
claudovka test-pii --format json "SSN: 123-45-6789"
```

---

### `claudovka models`

Manage ML models for Tier 2 (GLiNER NER) and Tier 3 (SLM) PII detection.

| Subcommand | Arguments | Description |
|---|---|---|
| `install <NAME>` | model name | Download and install a model |
| `list` | — | List installed models |

```bash
claudovka models install gliner-pii-base
claudovka models list
```

---

### `claudovka benchmark`

Run PII detection benchmark against built-in fixtures.

| Flag | Description |
|---|---|
| `--tier <N>` | Only benchmark tier `1` or `2` (default: all tiers) |

```bash
claudovka benchmark
claudovka benchmark --tier 1
```

---

### `claudovka setup-network`

Print the `/etc/hosts` entries and macOS `pf` rules needed for transparent network proxy mode. Does not apply them.

```bash
claudovka setup-network
```

---

### `claudovka network-enable`

Write `/etc/hosts` entries and `pf` rules for network proxy mode. Requires admin privileges.

```bash
sudo claudovka network-enable
```

---

### `claudovka network-disable`

Revert `/etc/hosts` entries and `pf` rules written by `network-enable`.

```bash
sudo claudovka network-disable
```

---

### `claudovka ca-path`

Print the path to the active CA certificate (`.pem` file).

```bash
claudovka ca-path
# ~/Library/Application Support/claudovka/ca/ca.pem
```

---

### `claudovka reset-ca`

Delete the current CA certificate and generate a new one. Existing TLS certificates signed by the old CA will no longer be trusted.

```bash
claudovka reset-ca
```

---

### `claudovka export`

Export the conversation log.

| Flag | Default | Description |
|---|---|---|
| `--format <FORMAT>` | `json` | Export format |
| `--output <PATH>` | *(required)* | Output file path |

```bash
claudovka export --format json --output ~/conversations.json
```

---

### `claudovka uninstall`

Remove claudovka from the system (binary, LaunchAgent, CA from trust store).

| Flag | Description |
|---|---|
| `--purge` | Also delete all user data: logs, database, models, config, CA files |

```bash
claudovka uninstall
claudovka uninstall --purge
```

---

## Typical workflows

### First-time setup (HTTP proxy mode)

```bash
claudovka init --install-ca
claudovka start
export HTTPS_PROXY=http://127.0.0.1:16440
export NODE_EXTRA_CA_CERTS="$HOME/Library/Application Support/claudovka/ca/ca.pem"
```

### Enable T3 standalone PII protection

```bash
claudovka config --protection-level intelligent   # downloads phi3-mini by default
claudovka start                                   # starts proxy + llama-server
```

### Enable full PII pipeline (all tiers)

```bash
claudovka models install gliner-pii-base          # download NER model first
claudovka config --protection-level 3             # downloads SLM model
claudovka start
```

### Run as a background service (Homebrew)

```bash
brew services start claudovka
```
