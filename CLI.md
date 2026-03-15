# Privacyclaw CLI Reference

## Global flags

| Flag | Type | Description |
|---|---|---|
| `--config <PATH>` | path | Path to config file (default: `~/Library/Application Support/privacyclaw/config.toml`) |
| `--log-file <PATH>` | path | Override log file path; empty string disables file logging |

---

## Commands

### `privacyclaw init`

Generate the local CA certificate and print setup instructions.

| Flag | Description |
|---|---|
| `--install-ca` | Attempt to install the CA into the OS trust store (macOS Keychain) |
| `--network` | Also configure pf rules and `/etc/hosts` for network proxy mode (requires admin) |

```bash
privacyclaw init
privacyclaw init --install-ca
privacyclaw init --install-ca --network   # full first-time setup including network proxy
```

---

### `privacyclaw start`

Start the proxy. Without `--mode`, respects `network_proxy.enabled` in config for the network proxy; dashboard and HTTP proxy always start.

| Flag | Values | Default | Description |
|---|---|---|---|
| `--mode` | `http` \| `network` \| `all` | *(config-driven)* | Which proxy mode to run |
| `--pii` | — | off | Enable PII replace mode (Tier 1 + Tier 2) |
| `--tray` | — | off | Show menu bar icon instead of blocking in terminal (macOS + tray feature) |

| `--mode` value | What starts |
|---|---|
| `http` | HTTP CONNECT proxy + dashboard |
| `network` | Transparent network proxy + dashboard |
| `all` | Both HTTP and network proxies + dashboard |
| *(omitted)* | HTTP always + dashboard; network only if `network_proxy.enabled = true` in config |

> **Note:** The dashboard (`http://127.0.0.1:16443`) always starts regardless of mode.

```bash
privacyclaw start                     # HTTP + dashboard (network if configured)
privacyclaw start --mode http         # HTTP CONNECT proxy + dashboard only
privacyclaw start --mode network      # Transparent network proxy only
privacyclaw start --mode all          # Both proxies simultaneously
privacyclaw start --pii               # Enable PII replace mode
privacyclaw start --tray              # macOS menu bar icon (tray build only)
```

---

### `privacyclaw stop`

Stop a running privacyclaw proxy by reading the PID file.

```bash
privacyclaw stop
```

---

### `privacyclaw config`

Manage privacyclaw configuration. With no flags or subcommands, launches the interactive wizard.

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
privacyclaw config                                   # interactive wizard
privacyclaw config show                              # print current config
privacyclaw config set pii.mode=replace              # set a key directly
privacyclaw config set pii.vault_ttl_hours=48
privacyclaw config --protection-level off            # disable PII
privacyclaw config --protection-level detect         # detect-only mode
privacyclaw config --protection-level 1              # regex replacement
privacyclaw config --protection-level 2              # regex + NER
privacyclaw config --protection-level 3              # full pipeline (downloads model)
privacyclaw config --protection-level intelligent    # T3 standalone (downloads model)
privacyclaw config --protection-level intelligent --model mistral-7b
privacyclaw config --protection-level 3 --model /path/to/model.gguf
```

After setting level `3` or `intelligent`, run `privacyclaw start` — it launches `llama-server` automatically using the configured model.

---

### `privacyclaw init`

Generate CA certificate and optionally install it in the system trust store.

*(See top of Commands section.)*

---

### `privacyclaw test-pii`

Test PII detection on a text string without starting the proxy.

| Argument / Flag | Description |
|---|---|
| `<TEXT>` | Text to analyze (positional, required) |
| `--locale <LOCALE>` | Locale for locale-specific patterns (e.g. `en-US`, `in-IN`, `br-BR`) |
| `--format <FORMAT>` | Output format: `text` (default) or `json` |

```bash
privacyclaw test-pii "My email is alice@example.com"
privacyclaw test-pii --locale en-GB "NI: AB 12 34 56 C"
privacyclaw test-pii --format json "SSN: 123-45-6789"
```

---

### `privacyclaw models`

Manage ML models for Tier 2 (GLiNER NER) and Tier 3 (SLM) PII detection.

| Subcommand | Arguments | Description |
|---|---|---|
| `install <NAME>` | model name | Download and install a model |
| `list` | — | List installed models |

```bash
privacyclaw models install gliner-pii-base
privacyclaw models list
```

---

### `privacyclaw benchmark`

Run PII detection benchmark against built-in fixtures.

| Flag | Description |
|---|---|
| `--tier <N>` | Only benchmark tier `1` or `2` (default: all tiers) |

```bash
privacyclaw benchmark
privacyclaw benchmark --tier 1
```

---

### `privacyclaw setup-network`

Print the `/etc/hosts` entries and macOS `pf` rules needed for transparent network proxy mode. Does not apply them.

```bash
privacyclaw setup-network
```

---

### `privacyclaw network-enable`

Write `/etc/hosts` entries and `pf` rules for network proxy mode. Requires admin privileges.

```bash
sudo privacyclaw network-enable
```

---

### `privacyclaw network-disable`

Revert `/etc/hosts` entries and `pf` rules written by `network-enable`.

```bash
sudo privacyclaw network-disable
```

---

### `privacyclaw ca-path`

Print the path to the active CA certificate (`.pem` file).

```bash
privacyclaw ca-path
# ~/Library/Application Support/privacyclaw/ca/ca.pem
```

---

### `privacyclaw reset-ca`

Delete the current CA certificate and generate a new one. Existing TLS certificates signed by the old CA will no longer be trusted.

```bash
privacyclaw reset-ca
```

---

### `privacyclaw export`

Export the conversation log.

| Flag | Default | Description |
|---|---|---|
| `--format <FORMAT>` | `json` | Export format |
| `--output <PATH>` | *(required)* | Output file path |

```bash
privacyclaw export --format json --output ~/conversations.json
```

---

### `privacyclaw uninstall`

Remove privacyclaw from the system (binary, LaunchAgent, CA from trust store).

| Flag | Description |
|---|---|
| `--purge` | Also delete all user data: logs, database, models, config, CA files |

```bash
privacyclaw uninstall
privacyclaw uninstall --purge
```

---

## Typical workflows

### First-time setup (HTTP proxy mode)

```bash
privacyclaw init --install-ca
privacyclaw start
export HTTPS_PROXY=http://127.0.0.1:16440
export NODE_EXTRA_CA_CERTS="$HOME/Library/Application Support/privacyclaw/ca/ca.pem"
```

### Enable T3 standalone PII protection

```bash
privacyclaw config --protection-level intelligent   # downloads phi3-mini by default
privacyclaw start                                   # starts proxy + llama-server
```

### Enable full PII pipeline (all tiers)

```bash
privacyclaw models install gliner-pii-base          # download NER model first
privacyclaw config --protection-level 3             # downloads SLM model
privacyclaw start
```

### Run as a background service (Homebrew)

```bash
brew services start privacyclaw
```
