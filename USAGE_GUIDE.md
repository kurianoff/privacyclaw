# Privacyclaw Usage Guide

Privacyclaw is a local MITM proxy that sits between your AI coding tools (Claude Code, Cursor, etc.) and LLM APIs. It lets you inspect all AI traffic in a dashboard, and optionally detect and replace PII before it ever leaves your machine.

---

## Installation

```bash
# Build the binary
cd privacyclaw
cargo build

# Or install globally
cargo install --path .
```

---

## First-time setup

```bash
# 1. Generate the local CA certificate
privacyclaw init

# 2. Trust it in your system keychain (macOS)
privacyclaw init --install-ca

# 3. Start the proxy and dashboard
privacyclaw start
```

The proxy starts at `http://127.0.0.1:16440` (CONNECT proxy).
The dashboard opens at `http://127.0.0.1:16443`.

### Configure your tools to use the proxy

**Claude Code / any terminal tool:**
```bash
export HTTPS_PROXY=http://127.0.0.1:16440
export HTTP_PROXY=http://127.0.0.1:16440
```

**Node.js (Claude Code specifically):**
```bash
export HTTPS_PROXY=http://127.0.0.1:16440
export NODE_EXTRA_CA_CERTS="$HOME/.config/privacyclaw/ca/ca.pem"
```

**Cursor / VS Code extensions:** Set proxy in the app's network settings to `http://127.0.0.1:16440`.

Once configured, all LLM API traffic (Anthropic, OpenAI, Google Gemini) flows through the proxy. Open the dashboard to see conversations in real time.

---

## PII Protection

By default the proxy is read-only — it records and displays traffic but does not modify it. PII protection is opt-in.

When enabled, the proxy:
1. Detects PII in outbound requests (emails, API keys, SSNs, phone numbers, etc.)
2. Replaces each value with a realistic synthetic equivalent before forwarding to the LLM
3. Transparently reverses the replacements in the LLM's streamed response, so your tool sees the original values

The LLM never sees the real PII. Your tool never knows anything changed.

### Enable PII protection

Edit (or create) `~/.config/privacyclaw/config.toml`:

```toml
[pii]
mode = "replace"   # "off" | "detect-only" | "replace"
locale = "en-US"   # affects locale-specific pattern packs

[pii.tiers]
regex = true       # Tier 1: fast regex (<1ms). Enable this first.
ner   = false      # Tier 2: GLiNER NER model. Requires model install.
slm   = false      # Tier 3: SLM sidecar. Requires sidecar running.
```

Restart the proxy after editing the config.

**Modes explained:**

| Mode | What it does |
|---|---|
| `off` | Passthrough. No detection, no modification. Default. |
| `detect-only` | Scans traffic, logs detections at INFO level. Does not modify requests. Good for auditing. |
| `replace` | Full bidirectional replacement. Outbound PII → synthetic. Inbound synthetic → original. |

### What Tier 1 (regex) detects

Tier 1 is always fast (<1ms per request) and covers:

| Type | Example |
|---|---|
| Email | `john@acme.com` |
| US Phone | `+1 555-867-5309` |
| US SSN | `123-45-6789` |
| Credit card | `4532-0151-1283-0366` (Luhn-validated) |
| IPv4 / IPv6 | `192.168.1.1`, `2001:db8::1` |
| OpenAI API key | `sk-proj-...` |
| AWS access key | `AKIA...` |
| AWS secret key | `aws_secret_access_key = ...` |
| GitHub PAT | `ghp_...` |
| Bearer token | `Authorization: Bearer eyJ...` |
| SSH private key | `-----BEGIN ... PRIVATE KEY-----` |
| DB connection string | `postgres://user:pass@host/db` |

### Verify detection is working

```bash
privacyclaw test-pii "My email is john@example.com and SSN is 123-45-6789"
```

Output:
```
Type                 Original                   Synthetic              Tier   Confidence
----------------------------------------------------------------------------------------
EMAIL                john@example.com           jackpatel@example.com  1      1.00
SSN                  123-45-6789                456-78-2301            1      1.00
```

JSON output:
```bash
privacyclaw test-pii --format json "email: john@example.com"
```

Test with a specific locale:
```bash
privacyclaw test-pii --locale en-GB "NI number: AB 12 34 56 C"
```

---

## Tier 2: Named Entity Recognition (GLiNER)

Tier 1 only catches structured PII (things with a fixed format). Tier 2 adds a local NER model that catches unstructured PII like person names, organisation names, and addresses.

```bash
# Install the model (~260 MB)
privacyclaw models install gliner-pii-base

# Check installed models
privacyclaw models list

# Enable in config
```

```toml
[pii.tiers]
regex = true
ner   = true

[pii.ner]
confidence_threshold = 0.5   # 0.0–1.0; lower = more detections, more false positives
timeout_ms = 500             # skip NER and fall back to Tier 1 if model takes too long
```

Tier 2 adds roughly 20–50ms to each outbound request. If the model takes longer than `timeout_ms`, it is silently skipped and Tier 1 results are used.

---

## Tracking what gets replaced

### Dashboard vault view

Open the dashboard at `http://127.0.0.1:16443` and click on any conversation. The **Vault** tab shows all original → synthetic mappings made for that conversation:

```
[EMAIL]   →  tarasmith@example.com
[PHONE]   →  +1 555-3681-5659
[SSN]     →  456-78-2301
```

Originals are shown masked by type to avoid re-displaying PII in the UI.

### REST API

```bash
curl http://127.0.0.1:16443/api/conversations/<conv-id>/vault
```

Response:
```json
[
  { "type": "EMAIL",  "original_masked": "[EMAIL]",  "synthetic": "tarasmith@example.com" },
  { "type": "PHONE",  "original_masked": "[PHONE]",  "synthetic": "+1 555-3681-5659" }
]
```

### Logs

With `mode = "detect-only"` or `mode = "replace"`, detection events are logged at INFO level:

```
INFO pii: detected entity_type=EMAIL tier=1 confidence=1.0 conv_id=abc123
INFO pii: detected entity_type=SSN tier=1 confidence=1.0 conv_id=abc123
```

Original values are **never** logged. Only the type, tier, and confidence appear.

Set `RUST_LOG=privacyclaw=debug` for more verbose output including per-chunk replacement buffer activity.

---

## Vault persistence

Per-conversation PII mappings survive proxy restarts. If you stop and restart privacyclaw mid-conversation, the same synthetic tokens are used for the same originals — the LLM context remains consistent.

Vaults are stored alongside conversation files in the storage directory (default `~/.config/privacyclaw/`). Each vault is a single line appended to the conversation's NDJSON file, tagged `"type":"vault"`.

Vaults expire from memory after `pii.vault_ttl_hours` (default 24h) of inactivity. The on-disk record is kept indefinitely with the conversation.

---

## Run a detection benchmark

```bash
privacyclaw benchmark
```

Output:
```
Running Tier 1 benchmark...

[PASS] EMAIL → john@acme.com
[PASS] SSN → 123-45-6789
[PASS] CREDIT_CARD → 4532015112830366
[PASS] OPENAI_API_KEY → sk-abcdefghijklmnopqrstuvwxyz12345678901234
[PASS] AWS_ACCESS_KEY → AKIAIOSFODNN7EXAMPLE

Results: 5/5 detected (100%)
```

Benchmark only Tier 1:
```bash
privacyclaw benchmark --tier 1
```

---

## Network-level proxy (no manual proxy setting required)

If you can't set `HTTPS_PROXY` (e.g., GUI apps, background daemons), use the network proxy mode. It intercepts all TCP port 443 traffic via a pf redirect.

```bash
# Print the /etc/hosts and pf rules needed
privacyclaw setup-network

# Apply them (requires sudo for pf and /etc/hosts)
# Then start the network proxy
privacyclaw network-start
```

Note: Node.js still requires `NODE_EXTRA_CA_CERTS` because it ignores the macOS system keychain:
```bash
export NODE_EXTRA_CA_CERTS="$HOME/.config/privacyclaw/ca/ca.pem"
```

---

## Configuration reference

Full config file with all options and defaults:

```toml
[proxy]
listen    = "127.0.0.1:16440"
dashboard = "127.0.0.1:16443"

[intercept]
domains = [
  "api.anthropic.com",
  "api.openai.com",
  "generativelanguage.googleapis.com",
  "api.mistral.ai",
  "api.groq.com",
]

[storage]
path             = "~/.config/privacyclaw/data.db"
max_size_mb      = 500
retention_days   = 30

[logging]
level = "info"   # trace | debug | info | warn | error

[pii]
mode            = "off"       # off | detect-only | replace
locale          = "en-US"
vault_ttl_hours = 24
models_dir      = "~/.config/privacyclaw/models"

[pii.tiers]
regex = true
ner   = false
slm   = false

[pii.ner]
model_path           = "~/.config/privacyclaw/models"
confidence_threshold = 0.5
timeout_ms           = 500

[pii.slm]
endpoint   = "http://127.0.0.1:16442"
timeout_ms = 5000
```

---

## Known limitations

- **Tier 1 only catches structured PII.** Person names, organisation names, and free-form addresses require Tier 2 (GLiNER model).
- **Images and binary content are not inspected.** Only text fields in JSON message bodies are processed.
- **The local conversation log contains original PII.** Only outbound LLM traffic is sanitised. Your local storage (`~/.config/privacyclaw/`) is not encrypted — treat it accordingly.
- **Locale-specific patterns** are currently available for en-US and en-GB. Additional locales (de-DE, fr-FR, in-IN, ko-KR, br-BR) are defined but patterns are not yet implemented.
- **Dashboard UI** shows vault data via the REST API; a visual diff panel is not yet built.
