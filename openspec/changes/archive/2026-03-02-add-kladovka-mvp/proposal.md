# Change: Add Kladovka MVP — MITM LLM Traffic Inspector

## Why

Developers using AI coding agents have no visibility into what data is sent to commercial LLMs. Kladovka provides a local MITM proxy that intercepts, decrypts, and displays all LLM API traffic in a real-time dashboard — giving developers full observability over their AI interactions.

## What Changes

- Add CA certificate management (generate, store, install into OS trust store)
- Add MITM TLS proxy using HTTP CONNECT tunneling
- Add LLM protocol parser for Anthropic, OpenAI, and Google Gemini (including SSE streaming)
- Add SQLite-backed persistent storage for intercepted conversations
- Add real-time web dashboard with WebSocket streaming
- Add CLI interface (`init`, `start`, `ca-path`, `reset-ca`, `export` subcommands)

## Impact

- Affected specs: ca-management, mitm-proxy, llm-parser, storage, dashboard, cli (all new)
- Affected code: entire project (greenfield)
