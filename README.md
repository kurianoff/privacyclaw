# privacyclaw

Local MITM privacy proxy for LLM API traffic. Intercepts, logs, and displays conversations between your apps and LLM providers — without sending data anywhere.

## What it does

- Transparently intercepts HTTPS traffic to Anthropic, OpenAI, Google, Mistral, Groq
- Parses streaming SSE responses in real time
- Stores conversations locally in SQLite
- Shows a live web dashboard at `http://localhost:16443`

All data stays on your machine.

## Two proxy modes

### CONNECT mode (per-app)

Configure a single app to use privacyclaw as an HTTP proxy:

```sh
export HTTPS_PROXY=http://127.0.0.1:16440
```

Works with any app that respects the standard proxy environment variable.

### Network mode (system-wide)

Intercepts traffic at the network layer via DNS override + port redirect. No per-app configuration needed — any process whose DNS resolves LLM domains to `127.0.0.1` is intercepted.

## Install

```sh
cargo install --path .
```

Or build and run from source:

```sh
cargo build --release
./target/release/privacyclaw --help
```

## Quick start

### 1. Generate the CA

```sh
privacyclaw init --install-ca
```

This generates a local CA certificate and installs it into your OS trust store. On macOS this requires entering your password for `security add-trusted-cert`.

The CA lives at `~/Library/Application Support/privacyclaw/ca/ca.pem` (macOS) or `~/.config/privacyclaw/ca/ca.pem` (Linux).

### 2a. CONNECT mode

```sh
privacyclaw start
export HTTPS_PROXY=http://127.0.0.1:16440
```

Point any LLM SDK or CLI at the proxy. Open `http://localhost:16443` to watch conversations live.

### 2b. Network mode (macOS)

Print setup instructions:

```sh
privacyclaw setup-network
```

Apply them (requires sudo for `/etc/hosts` and `pfctl`):

```sh
# Add DNS overrides
sudo tee -a /etc/hosts <<'EOF'
127.0.0.1  api.anthropic.com
127.0.0.1  api.openai.com
127.0.0.1  generativelanguage.googleapis.com
127.0.0.1  api.mistral.ai
127.0.0.1  api.groq.com
EOF

# Create pf redirect: port 443 → 16441
sudo tee /etc/pf.anchors/privacyclaw <<'EOF'
rdr pass on lo0 proto tcp from any to 127.0.0.1 port 443 -> 127.0.0.1 port 16441
EOF

# Add anchor to pf.conf — rdr-anchor must go in the translation section,
# before the filter anchor "com.apple/*" line
sudo pfctl -ef /etc/pf.conf
```

For Node.js apps (including Claude Code CLI), also set:

```sh
export NODE_EXTRA_CA_CERTS="$HOME/Library/Application Support/privacyclaw/ca/ca.pem"
```

Node.js ignores the macOS system keychain and requires this variable explicitly.
Add it to `~/.zshrc` to make it permanent.

#### Claude Code in VSCode

The VSCode extension spawns Claude Code in a sandboxed environment that does not inherit shell variables. Add the CA via VSCode settings instead:

```json
// settings.json
"claudeCode.environmentVariables": [
  { "name": "NODE_EXTRA_CA_CERTS", "value": "/Users/<you>/Library/Application Support/privacyclaw/ca/ca.pem" },
  { "name": "SSL_CERT_FILE",        "value": "/Users/<you>/Library/Application Support/privacyclaw/ca/ca.pem" }
]
```

Replace `<you>` with your macOS username. The CA path is also printed by `privacyclaw init`.

### IPv6 — important caveat

`/etc/hosts` only overrides **IPv4** (A records). Many LLM API domains also have AAAA records (real IPv6 addresses). Apps that prefer IPv6 — including the native macOS Claude binary — will resolve the real IPv6 address and bypass both the hosts override and the pf redirect entirely.

To intercept IPv6 traffic as well, add loopback overrides and a second pf rule:

```sh
# /etc/hosts: add IPv6 loopback alongside IPv4
sudo tee -a /etc/hosts <<'EOF'
::1  api.anthropic.com
::1  api.openai.com
::1  generativelanguage.googleapis.com
::1  api.mistral.ai
::1  api.groq.com
EOF

# pf anchor: add IPv6 redirect alongside IPv4
sudo tee /etc/pf.anchors/privacyclaw <<'EOF'
rdr pass on lo0 inet  proto tcp from any to 127.0.0.1 port 443 -> 127.0.0.1 port 16441
rdr pass on lo0 inet6 proto tcp from any to ::1       port 443 -> ::1       port 16441
EOF

sudo pfctl -ef /etc/pf.conf
```

Then configure privacyclaw to listen on all interfaces (IPv4 + IPv6) by setting `listen = "[::]:16441"` in `config.toml` under `[network_proxy]`.

Then start the proxy:

```sh
privacyclaw start --mode network
```

Open `http://localhost:16443` for the live dashboard.

## Commands

| Command | Description |
| --- | --- |
| `privacyclaw init [--install-ca]` | Generate CA; optionally install into OS trust store |
| `privacyclaw start` | Start CONNECT proxy (`:16440`) + dashboard (`:16443`) |
| `privacyclaw start --mode network` | Start network proxy (`:16441`) + dashboard (`:16443`) |
| `privacyclaw start --mode all` | Start both proxies + dashboard |
| `privacyclaw setup-network` | Print `/etc/hosts` + pf rules for network mode |
| `privacyclaw ca-path` | Print path to CA certificate |
| `privacyclaw reset-ca` | Delete CA and generate a new one |
| `privacyclaw export --format json --output out.json` | Export conversation log |

## Configuration

Copy `config.example.toml` and pass it with `--config`:

```sh
privacyclaw --config config.toml start
```

```toml
[proxy]
listen = "127.0.0.1:16440"     # CONNECT proxy address
dashboard = "127.0.0.1:16443"  # Dashboard address

[intercept]
# Domains to MITM; all other HTTPS is passed through unchanged
domains = [
  "api.anthropic.com",
  "api.openai.com",
  "generativelanguage.googleapis.com",
  "api.mistral.ai",
  "api.groq.com",
]

[network_proxy]
listen = "127.0.0.1:16441"
enabled = false                 # Set true to auto-start with `privacyclaw start`

[storage]
# macOS: ~/Library/Application Support/privacyclaw/data.db
# Linux: ~/.config/privacyclaw/data.db
path = "~/.config/privacyclaw/data.db"
max_size_mb = 500
retention_days = 30

[logging]
level = "info"                  # trace | debug | info | warn | error
```

## Removing

To stop intercepting traffic and undo system changes:

```sh
# Remove /etc/hosts entries (delete the privacyclaw block)
sudo nano /etc/hosts

# Remove pf anchor
sudo rm /etc/pf.anchors/privacyclaw
# Remove the two privacyclaw lines from /etc/pf.conf, then reload:
sudo pfctl -ef /etc/pf.conf

# Remove CA from trust store
sudo security remove-trusted-cert -d "$HOME/Library/Application Support/privacyclaw/ca/ca.pem"
```

## How it works

**CONNECT mode**: Acts as an HTTP proxy. When a client sends `CONNECT api.anthropic.com:443`, privacyclaw generates a leaf certificate signed by its local CA, performs TLS termination, then opens a real TLS connection upstream and proxies the decrypted traffic. Intercepted domains get MITM'd; everything else is tunneled through unchanged.

**Network mode**: Uses DNS override (`/etc/hosts`) to redirect LLM API hostnames to `127.0.0.1`, and a `pf` redirect rule to forward port 443 → 16441. privacyclaw listens on `:16441`, reads the TLS SNI from the ClientHello to determine the target domain, then performs the same MITM logic.

Certificates are generated on demand and cached in memory. The CA private key never leaves your machine.

## Architecture

```text
src/
├── main.rs          CLI (clap), startup, task orchestration
├── config.rs        TOML config
├── ca/
│   ├── mod.rs       CA generation, disk I/O, OS trust store install
│   └── cert_gen.rs  Per-domain leaf cert generation, CertCache, SniCertResolver
├── proxy/
│   ├── mod.rs       CONNECT proxy listener
│   ├── connect.rs   HTTP CONNECT handler
│   ├── intercept.rs Request/response MITM + storage + WebSocket push
│   ├── network.rs   Network-mode listener (SNI-based)
│   └── passthrough.rs  Transparent tunnel for non-intercepted domains
├── parser/
│   ├── sse.rs       Server-Sent Events streaming parser
│   ├── anthropic.rs Anthropic message extraction
│   ├── openai.rs    OpenAI message extraction
│   └── google.rs    Google Gemini message extraction
├── storage/         SQLite store (rusqlite)
└── dashboard/       HTTP + WebSocket server, embedded assets
```

## Requirements

- Rust 1.75+
- macOS (network mode tested); Linux supported for CONNECT mode
- `sudo` access for CA trust store install and pf rules (network mode)
