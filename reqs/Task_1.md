# TASK: Privacy Proxy MVP — MITM LLM Traffic Inspector

## Objective

Build a local MITM proxy in Rust that intercepts HTTPS traffic between AI coding agents (Claude Code, OpenClaw, Cursor, Codex, etc.) and commercial LLM APIs. The proxy decrypts, parses, and displays all request/response traffic — including streaming SSE responses — in a real-time web dashboard served on localhost.

This is **Phase 1 (observation mode only)** — no PII redaction or modification yet. The goal is to prove the core architecture: generate a trusted CA → intercept TLS → parse LLM-specific protocols → display streaming conversations in real time → log everything.

---

## Architecture Overview

```
┌──────────────┐        ┌─────────────────────────────────┐        ┌──────────────────┐
│  Claude Code  │        │        Privacy Proxy             │        │  api.anthropic.com│
│  OpenClaw     │──HTTPS──▶  ┌───────────┐  ┌───────────┐ ──HTTPS──▶  api.openai.com   │
│  Cursor       │        │  │ TLS MITM  │──▶│  Parser   │ │        │  generativelang.. │
│  Codex        │        │  │ (rustls)  │  │ (SSE/JSON)│ │        │  (Google)         │
│  Any client   │        │  └───────────┘  └─────┬─────┘ │        └──────────────────┘
└──────────────┘        │                        │       │
                         │                  ┌─────▼─────┐ │
                         │                  │  Storage   │ │
                         │                  │ (SQLite)   │ │
                         │                  └─────┬─────┘ │
                         │                        │       │
                         │                  ┌─────▼─────┐ │
                         │                  │  Web UI    │ │
                         │                  │ (WebSocket)│ │
                         │                  └───────────┘ │
                         └─────────────────────────────────┘
                              localhost:8080 (proxy)
                              localhost:8443 (dashboard)
```

---

## Functional Requirements

### 1. CA Certificate Management

On first run (`privacy-proxy init`), the proxy must:

- Generate a root CA key pair (ECDSA P-256 or Ed25519) and self-signed root certificate.
- Store the CA key and certificate in a platform-specific secure location:
  - macOS: `~/Library/Application Support/privacy-proxy/ca/`
  - Linux: `~/.config/privacy-proxy/ca/`
  - Windows: `%APPDATA%\privacy-proxy\ca\`
- Offer to install the CA certificate into the OS trust store (with user confirmation):
  - macOS: `security add-trusted-cert -d -r trustRoot -k /Library/Keychains/System.keychain ca.pem`
  - Linux: copy to `/usr/local/share/ca-certificates/` and run `update-ca-certificates`
  - Windows: `certutil -addstore -user Root ca.pem`
- Print clear instructions for manual installation if automatic installation fails or is declined.
- On subsequent runs, load the existing CA. Never regenerate unless explicitly asked (`privacy-proxy reset-ca`).

### 2. MITM TLS Proxy

The proxy listens on `localhost:8080` (configurable) and operates as an HTTP CONNECT proxy:

- Client sends `CONNECT api.anthropic.com:443 HTTP/1.1`.
- Proxy establishes a TLS connection to the real upstream server (api.anthropic.com).
- Proxy dynamically generates a leaf certificate for `api.anthropic.com` signed by the local CA.
- Proxy presents this leaf certificate to the client, completing the TLS handshake.
- All traffic is now decrypted in both directions.

Implementation details:

- Use `rcgen` crate for dynamic certificate generation. Cache generated leaf certificates in memory (HashMap keyed by domain) so we don't regenerate on every connection.
- Use `rustls` for both client-side (proxy → upstream) and server-side (client → proxy) TLS.
- Use `tokio` for async I/O. Each CONNECT tunnel is a spawned task.
- Use `hyper` for HTTP parsing of the outer CONNECT request and the inner HTTP requests/responses.

Domain filtering:

- By default, only intercept traffic to a configurable allowlist of domains:
  ```toml
  [proxy]
  listen = "127.0.0.1:8080"

  [intercept]
  domains = [
    "api.anthropic.com",
    "api.openai.com",
    "generativelanguage.googleapis.com",
    "api.mistral.ai",
    "api.groq.com",
  ]
  ```
- Traffic to domains NOT in the allowlist must be tunneled transparently (pure TCP passthrough, no MITM). This is critical — we must not break non-LLM HTTPS traffic.

### 3. LLM Protocol Parsing

After decrypting, parse the HTTP request/response according to LLM API formats.

**Request parsing** — extract and structure:

- Provider (Anthropic / OpenAI / Google / other) — inferred from domain.
- Model name (from request body: `model` field).
- Messages array (system prompt, user messages, assistant messages).
- Tools / function definitions if present.
- Metadata: timestamp, request ID, token counts if available.

**Response parsing** — handle two modes:

- **Non-streaming**: Standard JSON response. Parse `content` / `choices` array.
- **Streaming (SSE)**: This is the critical path. Most LLM clients use streaming.
  - Anthropic format: `event: content_block_delta` + `data: {"type":"content_block_delta","delta":{"type":"text_delta","text":"..."}}`
  - OpenAI format: `data: {"choices":[{"delta":{"content":"..."}}]}`
  - Buffer SSE chunks, extract text deltas, accumulate into a complete response.
  - Forward each chunk to the web UI via WebSocket in real time (so the dashboard shows tokens appearing live, just like in the chat UI).
  - Simultaneously forward the original unmodified chunk to the client with zero delay. The proxy must NOT add latency — buffer a copy for display, but pass through immediately.

**Important**: In Phase 1, the proxy is read-only. It must never modify request or response bytes. The client must receive bit-identical responses to what the upstream server sent.

### 4. Storage

Use SQLite (via `rusqlite` crate) for persistent storage of intercepted conversations.

Schema:

```sql
CREATE TABLE conversations (
  id TEXT PRIMARY KEY,           -- UUID
  started_at TEXT NOT NULL,      -- ISO 8601
  provider TEXT NOT NULL,        -- "anthropic" | "openai" | "google"
  model TEXT,                    -- "claude-sonnet-4-20250514" etc.
  client_hint TEXT               -- User-Agent or other heuristic for identifying the client
);

CREATE TABLE messages (
  id TEXT PRIMARY KEY,           -- UUID
  conversation_id TEXT NOT NULL REFERENCES conversations(id),
  direction TEXT NOT NULL,       -- "request" | "response"
  timestamp TEXT NOT NULL,       -- ISO 8601
  role TEXT,                     -- "system" | "user" | "assistant" | "tool"
  content TEXT NOT NULL,         -- Full message text (accumulated from SSE chunks)
  raw_http BLOB,                -- Compressed raw HTTP bytes (gzip) for forensic review
  tokens_in INTEGER,            -- From API response headers/body if available
  tokens_out INTEGER
);

CREATE INDEX idx_messages_conversation ON messages(conversation_id, timestamp);
```

Conversation grouping heuristic: requests to the same domain within the same TCP connection (or within a short time window from the same client IP:port) belong to the same conversation. This can be refined later.

### 5. Real-Time Web Dashboard

Serve a web UI on `localhost:8443` (configurable).

Technology: embedded static assets (HTML + CSS + JS) served by the Rust binary. No build step, no npm, no framework dependencies. Keep it simple — vanilla JS + WebSocket.

The dashboard must show:

**Left panel: Conversation list**
- List of intercepted conversations, most recent first.
- Each entry shows: timestamp, provider icon/name, model, message count, client hint.
- Live indicator (green dot) for active/streaming conversations.
- Click to select and view in the main panel.

**Main panel: Conversation detail**
- Chat-style display showing the full request → response exchange.
- System prompt displayed in a collapsible section (often very long).
- User messages and assistant responses styled differently (like a chat UI).
- **Live streaming**: when a response is currently streaming, tokens appear in real time via WebSocket push. Use a monospace font and show a blinking cursor.
- Tool use / function calls displayed in a distinct style (collapsible code block).
- Show metadata: model, token counts, latency, timestamps.

**Top bar:**
- Proxy status indicator (running / stopped / error).
- Total requests intercepted counter.
- Filter/search across conversations.

WebSocket protocol (simple JSON messages):

```json
// Server → Client: new conversation started
{"type": "conversation_start", "id": "uuid", "provider": "anthropic", "model": "claude-sonnet-4-20250514", "timestamp": "..."}

// Server → Client: streaming text delta
{"type": "text_delta", "conversation_id": "uuid", "text": "Hello", "timestamp": "..."}

// Server → Client: response complete
{"type": "response_complete", "conversation_id": "uuid", "tokens_in": 1500, "tokens_out": 800}

// Server → Client: full message (for non-streaming or page load)
{"type": "message", "conversation_id": "uuid", "direction": "request", "role": "user", "content": "...", "timestamp": "..."}
```

### 6. CLI Interface

```bash
# First-time setup: generate CA, offer to install
privacy-proxy init

# Start the proxy + dashboard
privacy-proxy start
# Output:
#   🔒 MITM proxy listening on 127.0.0.1:8080
#   📊 Dashboard at http://localhost:8443
#   📋 Intercepting: api.anthropic.com, api.openai.com, ...
#   
#   Configure your system/terminal:
#     export HTTPS_PROXY=http://127.0.0.1:8080

# Start with custom config
privacy-proxy start --config ./config.toml

# Show CA certificate path (for manual trust installation)
privacy-proxy ca-path

# Remove CA from trust store and delete keys
privacy-proxy reset-ca

# Export conversation log
privacy-proxy export --format json --output conversations.json
privacy-proxy export --format html --output report.html
```

Use `clap` crate for argument parsing. Use `tracing` crate for structured logging.

---

## Configuration File

Default location: `~/.config/privacy-proxy/config.toml` (XDG on Linux, platform-appropriate elsewhere).

```toml
[proxy]
listen = "127.0.0.1:8080"      # Proxy listen address
dashboard = "127.0.0.1:8443"   # Web dashboard address

[intercept]
# Only MITM these domains. All other HTTPS is passed through transparently.
domains = [
  "api.anthropic.com",
  "api.openai.com",
  "generativelanguage.googleapis.com",
  "api.mistral.ai",
  "api.groq.com",
]

[storage]
path = "~/.config/privacy-proxy/data.db"  # SQLite database
max_size_mb = 500                          # Auto-prune oldest conversations
retention_days = 30                        # Auto-delete after N days

[logging]
level = "info"                             # trace | debug | info | warn | error
```

---

## Non-Functional Requirements

- **Zero-latency passthrough**: The proxy must forward bytes to the client as soon as they arrive from upstream. Parsing and storage happen on a copied byte stream, never on the critical path. Use `tokio::io::copy_bidirectional` or a split approach where one task copies bytes through and another task parses the copy.
- **No panics on malformed input**: The proxy handles unexpected/malformed HTTP, incomplete SSE chunks, binary payloads, and connection drops gracefully. If parsing fails, log a warning and pass through raw bytes.
- **Memory-bounded**: SSE accumulation buffers must have a max size (e.g., 10MB per response). If exceeded, stop accumulating but keep forwarding.
- **Cross-platform**: Must compile and run on macOS (ARM64 + x86_64), Linux (x86_64 + ARM64 for Raspberry Pi), and Windows (x86_64). Use conditional compilation (`#[cfg(target_os = "...")]`) for CA installation commands only.
- **Single binary**: The final artifact is one static binary with the web UI assets embedded (use `include_str!` / `include_bytes!` or `rust-embed` crate).

---

## Crate Dependencies

```toml
[dependencies]
tokio = { version = "1", features = ["full"] }
hyper = { version = "1", features = ["full"] }
hyper-util = "0.1"
http-body-util = "0.1"
rustls = "0.23"
tokio-rustls = "0.26"
rcgen = "0.13"                    # Dynamic certificate generation
webpki-roots = "0.26"             # Mozilla root CA bundle for upstream TLS
rusqlite = { version = "0.32", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"                      # Config parsing
clap = { version = "4", features = ["derive"] }
tracing = "0.1"
tracing-subscriber = "0.3"
uuid = { version = "1", features = ["v4"] }
chrono = "0.4"
flate2 = "1"                      # Gzip compression for raw HTTP storage
tokio-tungstenite = "0.24"        # WebSocket for dashboard
rust-embed = "8"                  # Embed static web assets
dirs = "6"                        # Platform-specific directories
```

---

## Project Structure

```
privacy-proxy/
├── Cargo.toml
├── config.example.toml
├── README.md
├── ARCHITECTURE.md                # Data flow diagram, security model, trust boundaries
├── src/
│   ├── main.rs                    # CLI entrypoint (clap)
│   ├── config.rs                  # Config loading, defaults, validation
│   ├── ca/
│   │   ├── mod.rs                 # CA management: generate, load, install
│   │   └── cert_gen.rs            # Dynamic leaf certificate generation + cache
│   ├── proxy/
│   │   ├── mod.rs                 # Main proxy server (accept loop)
│   │   ├── connect.rs             # CONNECT handler: tunnel setup, TLS handshake
│   │   ├── intercept.rs           # Bidirectional copy with tee (copy bytes + parse)
│   │   └── passthrough.rs         # Transparent tunnel for non-intercepted domains
│   ├── parser/
│   │   ├── mod.rs                 # Dispatcher: detect provider, route to parser
│   │   ├── anthropic.rs           # Anthropic Messages API: request + SSE response
│   │   ├── openai.rs              # OpenAI Chat Completions: request + SSE response
│   │   ├── google.rs              # Google Gemini API: request + SSE response
│   │   └── sse.rs                 # Generic SSE stream parser (shared logic)
│   ├── storage/
│   │   ├── mod.rs                 # SQLite operations: insert, query, prune
│   │   └── schema.sql             # Embedded schema for auto-migration
│   ├── dashboard/
│   │   ├── mod.rs                 # HTTP server for web UI + WebSocket handler
│   │   └── assets/                # Embedded static files
│   │       ├── index.html         # Single-page dashboard
│   │       ├── style.css
│   │       └── app.js             # WebSocket client, conversation renderer
│   └── util.rs                    # Shared helpers: timestamps, UUID, compression
└── tests/
    ├── integration/
    │   ├── proxy_connect_test.rs   # Test CONNECT tunnel establishment
    │   ├── anthropic_sse_test.rs   # Replay captured Anthropic SSE stream
    │   └── openai_sse_test.rs      # Replay captured OpenAI SSE stream
    └── fixtures/
        ├── anthropic_stream.bin    # Captured SSE bytes for testing
        └── openai_stream.bin
```

---

## Implementation Order

Build and verify in this sequence. Each step should result in a working (partial) binary.

### Step 1: CLI skeleton + config
- Set up clap CLI with `init`, `start`, `ca-path`, `reset-ca` subcommands.
- Implement config loading from TOML with sensible defaults.
- Implement platform-specific directory resolution via `dirs` crate.
- **Verify**: `cargo run -- start` prints config and exits.

### Step 2: CA management
- Implement `init` subcommand: generate ECDSA P-256 CA key + self-signed cert with `rcgen`.
- Save to disk. Load on subsequent runs.
- Implement trust store installation for macOS / Linux / Windows (behind `--install-ca` flag).
- Implement `reset-ca` to remove and regenerate.
- **Verify**: `privacy-proxy init` creates CA files. `openssl x509 -in ca.pem -text` shows valid cert.

### Step 3: Basic CONNECT proxy with passthrough
- Implement the TCP listener on configured port.
- Parse `CONNECT host:port` requests.
- For ALL domains (no MITM yet): establish upstream TCP connection, bidirectional byte copy.
- **Verify**: `HTTPS_PROXY=http://localhost:8080 curl https://api.anthropic.com/v1/messages` — connection succeeds (with auth error from Anthropic, which is expected).

### Step 4: MITM for allowlisted domains
- On CONNECT to an allowlisted domain: generate leaf cert, complete TLS handshake with client, establish TLS to upstream.
- Bidirectional decrypted byte forwarding.
- For non-allowlisted domains: continue with passthrough from Step 3.
- **Verify**: `HTTPS_PROXY=http://localhost:8080 curl https://api.anthropic.com/v1/messages -d '...'` — returns valid API response. `curl https://google.com` — also works (passthrough). Check with `openssl s_client` that the presented cert is signed by our CA.

### Step 5: HTTP parsing + request logging
- Parse decrypted bytes as HTTP (request line, headers, body).
- For LLM API requests: extract and log provider, model, messages.
- Store in SQLite.
- **Verify**: Make a Claude Code request through proxy. Check SQLite database has the conversation with correct content.

### Step 6: SSE response parsing
- Detect `content-type: text/event-stream` responses.
- Implement SSE chunk parser that handles partial chunks, multi-line data fields, and buffering.
- Accumulate text deltas into complete response text.
- Store complete response in SQLite.
- **Verify**: Make a streaming Claude Code request. Database contains full accumulated response.

### Step 7: Web dashboard (static)
- Serve embedded HTML/CSS/JS on dashboard port.
- On page load: fetch conversation list and selected conversation from SQLite via REST endpoints (`GET /api/conversations`, `GET /api/conversations/:id`).
- Render chat-style UI.
- **Verify**: Open browser to dashboard, see past conversations.

### Step 8: Live streaming via WebSocket
- Add WebSocket endpoint on dashboard server.
- When SSE chunks arrive in the parser, broadcast text deltas to connected WebSocket clients.
- Dashboard JS: append tokens to current conversation in real time.
- **Verify**: Start Claude Code session, open dashboard — see tokens appearing live as Claude responds.

---

## Testing Strategy

- **Unit tests**: SSE parser with edge cases (partial chunks, empty events, multi-line data, `[DONE]` sentinel).
- **Integration tests**: Replay captured HTTP/SSE byte streams through the parser pipeline. Use recorded fixtures from real Anthropic and OpenAI API responses.
- **End-to-end**: Script that starts proxy, makes a real API call (requires API key), and asserts the dashboard shows the conversation. Use this sparingly (costs money, requires credentials).
- **Passthrough safety**: Verify that non-intercepted HTTPS traffic (e.g., to github.com) passes through with zero modification and the original server certificate is presented to the client.

---

## What This MVP Does NOT Include (Future Phases)

- PII detection or redaction (Phase 2)
- Edge LLM integration for smart PII detection (Phase 3)
- PII re-injection into streaming responses (Phase 3)
- MCP / tool call traffic interception (Phase 2)
- Multi-language / locale-specific PII packs (Phase 3)
- Audit logging and compliance reporting (Phase 4)
- Visual / image content handling (Phase 4)
- Policy engine with per-tenant rules (Phase 4)

---

## Success Criteria for Phase 1

1. A developer installs the proxy in under 60 seconds (`cargo install` or download binary + `privacy-proxy init`).
2. Sets `HTTPS_PROXY=http://localhost:8080` and uses Claude Code normally.
3. Opens `http://localhost:8443` in a browser and sees every message sent to and received from the LLM — including streaming responses appearing in real time.
4. Claude Code works identically to without the proxy — no errors, no latency, no broken features.
5. Non-LLM HTTPS traffic (git push, npm install, brew update) passes through unaffected.
6. Works on macOS, Linux, and Windows.
