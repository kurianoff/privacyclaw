# PII Protection Setup

privacyclaw can detect and replace personally identifiable information (PII) in
LLM API traffic before it leaves the machine. The LLM never sees real email
addresses, SSNs, API keys, or other sensitive data. Responses from the model
are reverse-mapped on the way back, so the calling application receives the
original values transparently.

---

## Quick Start

1. Open (or create) `~/.config/privacyclaw/config.toml` and add:

   ```toml
   [pii]
   mode = "replace"
   locale = "en-US"
   ```

2. Start the proxy normally:

   ```sh
   privacyclaw start
   # or, for network mode:
   privacyclaw network-start
   ```

3. Confirm PII is being scrubbed by testing a sample string:

   ```sh
   privacyclaw test-pii "My SSN is 123-45-6789 and email is john@acme.com"
   ```

That is all that is required for Tier 1 (regex) protection. No model
installation or extra dependencies are needed.

---

## Configuration Reference

All PII settings live under the `[pii]` table in the config file.

### `[pii]`

| Field | Type | Default | Description |
|---|---|---|---|
| `mode` | string | `"off"` | Master switch. See [Modes](#modes). |
| `locale` | string | `"en-US"` | Locale for pattern selection. See [Locale support](#locale-support). |
| `vault_ttl_hours` | integer | `24` | Hours to keep a conversation vault in memory after last use. |
| `models_dir` | string | `~/.config/privacyclaw/models` | Directory for ML model files (Tier 2/3). |

### `[pii.tiers]`

Controls which detection tiers are active. Tiers are applied in order; a span
matched by an earlier tier is not re-evaluated by later tiers.

| Field | Type | Default | Description |
|---|---|---|---|
| `regex` | bool | `true` | Tier 1 regex detector. Zero-dependency, sub-millisecond. |
| `ner` | bool | `false` | Tier 2 GLiNER NER model. Requires model install. |
| `slm` | bool | `false` | Tier 3 SLM sidecar. Requires a running local model server. |

### `[pii.ner]`

Settings for the Tier 2 GLiNER model (only used when `pii.tiers.ner = true`).

| Field | Type | Default | Description |
|---|---|---|---|
| `model_path` | string | `~/.config/privacyclaw/models` | Path to the ONNX model directory. |
| `confidence_threshold` | float | `0.5` | Minimum score (0.0–1.0) to accept a span as PII. |
| `timeout_ms` | integer | `500` | Skip inference if it takes longer than this. |

### `[pii.slm]`

Settings for the Tier 3 SLM sidecar (only used when `pii.tiers.slm = true`).

| Field | Type | Default | Description |
|---|---|---|---|
| `endpoint` | string | `http://127.0.0.1:8080` | HTTP base URL of the sidecar. |
| `timeout_ms` | integer | `5000` | Request timeout in milliseconds. |

---

## Modes

### `off` (default)

PII processing is completely disabled. All traffic passes through unchanged.
This is the Phase 1 passthrough behaviour and has zero overhead.

```toml
[pii]
mode = "off"
```

### `detect-only`

The proxy scans each request body and logs every detected span at `INFO` level.
Request and response bodies are **not modified**. Use this mode to audit what
PII is present in your traffic before enabling full replacement.

```toml
[pii]
mode = "detect-only"
```

Each logged detection includes the entity type, byte range within the text,
confidence score, and detection tier. The original value is never included in
the log.

```
INFO pii: detected span conv_id="…" entity_type="EMAIL" start=10 end=24 confidence=1.0 tier=1
```

### `replace`

Full bidirectional PII replacement:

- **Outbound (request):** PII values in message content are replaced with
  realistic synthetic equivalents before the request is forwarded to the LLM.
- **Inbound (response):** Synthetic tokens appearing in the LLM's reply are
  replaced back with the originals before the response is returned to the
  caller.

```toml
[pii]
mode = "replace"
```

The `--pii` CLI flag also activates replace mode without editing the config:

```sh
privacyclaw start --pii
privacyclaw network-start --pii
```

---

## CLI: test-pii

Run the detector against a text string without starting the proxy. Useful for
verifying pattern coverage or testing a specific input.

```sh
privacyclaw test-pii "Contact me at john@acme.com, SSN 123-45-6789"
```

Example output (default `text` format):

```
Type                 Original                                 Synthetic                      Tier   Confidence
---------------------------------------------------------------------------------------------------------
EMAIL                john@acme.com                            alice.davis@example.com        1      1.00
SSN                  123-45-6789                              214-37-8821                    1      1.00
```

### Flags

| Flag | Default | Description |
|---|---|---|
| `--locale <LOCALE>` | (from config or `en-US`) | Override locale for this run. |
| `--format text\|json` | `text` | Output format. |

JSON output includes `type`, `original`, `synthetic`, `tier`, and `confidence`
fields for each detection.

```sh
privacyclaw test-pii "sk-abcdefghijklmnopqrstuvwxyz12345678901234" --format json
```

---

## Supported PII Types

The table below lists all entity types detected by Tier 1 (regex). Tier 2
(GLiNER) additionally detects named entities such as person names, organisation
names, addresses, and dates of birth once a model is installed.

| Label | Description | Example |
|---|---|---|
| `EMAIL` | Email address | `john@acme.com` |
| `PHONE` | US phone number | `415-555-1234` |
| `SSN` | US Social Security Number | `123-45-6789` |
| `CREDIT_CARD` | Credit card number (Luhn-validated) | `4532 0151 1283 0366` |
| `IP_V4` | IPv4 address | `192.168.1.100` |
| `IP_V6` | IPv6 address | `2001:db8::1` |
| `OPENAI_API_KEY` | OpenAI secret key (`sk-...`) | `sk-abcde...` |
| `AWS_ACCESS_KEY` | AWS Access Key ID (`AKIA...`) | `AKIAIOSFODNN7EXAMPLE` |
| `AWS_SECRET_KEY` | AWS Secret Access Key | `wJalrXUtnFEMI/K7MDENG/...` |
| `GITHUB_PAT` | GitHub Personal Access Token | `ghp_ABCdef...` |
| `BEARER_TOKEN` | HTTP Bearer token | `Bearer eyJhbGci...` |
| `SSH_PRIVATE_KEY` | SSH / PGP private key block header | `-----BEGIN RSA PRIVATE KEY-----` |
| `DB_CONNECTION_STRING` | Database connection URI with credentials | `postgres://user:pass@host/db` |
| `URL_WITH_CREDS` | Any URL with embedded username:password | `https://user:pass@example.com` |

Credit cards require a valid Luhn checksum; all other Tier 1 patterns are
regex-only and report confidence `1.0`.

---

## Locale Support

The `locale` setting selects locale-specific pattern packs. All 7 supported
locales share the universal Tier 1 patterns listed above. Locale-specific
additions (e.g. Aadhaar for `in-IN`, CPF for `br-BR`) are applied on top.

| Locale code | Region |
|---|---|
| `en-US` | United States (default) |
| `en-GB` | United Kingdom |
| `de-DE` | Germany |
| `fr-FR` | France |
| `in-IN` | India (Aadhaar support) |
| `ko-KR` | South Korea |
| `br-BR` | Brazil (CPF support) |

Set locale in config:

```toml
[pii]
locale = "in-IN"
```

Or override per `test-pii` run:

```sh
privacyclaw test-pii "Aadhaar 2345 6789 0123" --locale in-IN
```

Locale matching is case-insensitive and accepts underscores (`en_US`) as well
as hyphens (`en-US`).

---

## Performance Notes

- **Tier 1 (regex):** Pure regex scan over message content fields. Typically
  under 1 ms per request. No external dependencies; always safe to enable.

- **Tier 2 (GLiNER NER):** Runs an ONNX model in-process. Adds 50–200 ms per
  request depending on text length and hardware. A configurable timeout
  (`pii.ner.timeout_ms`, default 500 ms) skips inference if the model is too
  slow and falls back to Tier 1 results only.

  Install the model before enabling:

  ```sh
  privacyclaw models install gliner-small
  privacyclaw models list
  ```

- **Tier 3 (SLM sidecar):** Delegates to an external HTTP server running a
  small language model. This tier is a stub in the current release; the
  endpoint and timeout are accepted but inference is not yet implemented.

---

## Choosing a Model

When Tier 3 is enabled, privacyclaw uses a locally-running GGUF model. The
four catalog models differ in size, memory usage, and detection quality:

| Model ID         | Size   | RAM    | Latency (typical) | Quality |
|------------------|--------|--------|-------------------|---------|
| smollm2-135m     | 90 MB  | 300 MB | ~100 ms/turn      | Good    |
| qwen2.5-0.5b     | 400 MB | 800 MB | ~250 ms/turn      | Better  |
| llama-3.2-1b     | 700 MB | 1.2 GB | ~500 ms/turn      | Better+ |
| phi-3-mini-3.8b  | 2.3 GB | 3.5 GB | ~1–2 s/turn       | Best    |

**Recommendation:** Start with `smollm2-135m`. It is auto-downloaded on first
run when T3 is enabled and no model is active. If you need higher accuracy for
edge-case PII, upgrade to `qwen2.5-0.5b` with:

```sh
privacyclaw models install qwen2.5-0.5b
privacyclaw models activate qwen2.5-0.5b
```

Latency figures assume Apple M-series hardware. Intel/AMD CPUs will be 2–3x
slower; the timeout (`pii.slm.timeout_ms`, default 5000 ms) controls fail-open
behavior if the model is too slow.

---

## Vault Persistence

Each conversation gets its own `PiiVault` — a bidirectional mapping table
between original PII values and their synthetic tokens. The vault is:

- **Keyed by conversation ID** (extracted from `X-Conversation-Id` or the
  request body, depending on the provider).
- **Held in memory** for `vault_ttl_hours` hours after the last access, then
  evicted automatically.
- **Saved to an NDJSON file** in the logs directory alongside the conversation
  record. On proxy restart the vault is reloaded from disk, so synthetic tokens
  remain consistent across restarts for the same conversation.

This means that if a request in turn 3 of a conversation contains the same
email address as turn 1, the proxy returns the same synthetic token and the LLM
sees a consistent identity throughout the session.

---

## Security Notes

- **Original PII is never logged.** The `detect-only` and `replace` modes log
  entity type, byte range, confidence, and conversation ID only. The raw text
  of a detected span is not written to any log file.

- **Synthetic tokens are deterministic per conversation.** The vault seeds its
  RNG from `sha1(conversation_id)`, so the same original value always maps to
  the same synthetic within a conversation. Different conversations produce
  different synthetics for the same input.

- **Synthetic values are realistic but safe.** Emails map to `@example.com`
  addresses, IPv4 addresses to RFC 1918 `10.x.x.x` ranges, credit cards to
  Luhn-valid numbers with the same brand prefix, and so on. The LLM receives
  structurally valid data and can reason about it without seeing real values.

- **Authorization headers are always redacted in logs** regardless of PII mode,
  via the `fmt_headers` helper used throughout the proxy.
