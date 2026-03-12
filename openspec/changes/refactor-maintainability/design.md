# Design: Phased maintainability refactoring

## Context

The codebase has grown from a Phase 1 MVP to a full PII-aware intercepting proxy (~8,600 LOC). The two largest files (`intercept.rs` at 888 lines, `storage/mod.rs` at 787 lines) accumulated duplication and wide function signatures incrementally across multiple feature branches. All changes here are purely internal restructuring — external behavior, CLI surface, storage format, and network protocol are unchanged.

Constraint: every phase must leave `cargo test` green and `cargo clippy -- -D warnings` clean before moving to the next.

## Goals / Non-Goals

- **Goals**: reduce argument counts to ≤5 per function, eliminate copy-pasted state machines, make storage writes O(1), remove all `#[allow(clippy::too_many_arguments)]` suppressions, give magic protocol constants readable names
- **Non-Goals**: change any network behavior, alter the SQLite schema, add new CLI commands, change config file format, improve test coverage beyond what's needed to validate the refactor

## Decisions

### Phase 1: `InterceptContext` struct

**Decision**: Wrap the 6 shared fields (`shared_conv_id`, `shared_vault`, `store`, `ws_tx`, `pii`, `provider`) in a single struct rather than using a trait or builder.

Rationale: All six fields are always needed together; there is no partial use case. A plain struct with `Clone`/`Arc` fields has zero overhead vs passing individually. A trait would require boxing or generics that ripple through the async spawn boundaries. A builder adds indirection with no benefit for a fixed set of fields.

```rust
// src/proxy/intercept.rs
struct InterceptContext {
    shared_conv_id: Arc<Mutex<Option<String>>>,
    shared_vault:   Arc<Mutex<Option<VaultHandle>>>,
    store:          Store,
    ws_tx:          broadcast::Sender<WsEvent>,
    pii:            PiiCtx,
    provider:       Provider,
}
```

All six suppressed `handle_*` and `finalize_*` functions reduce from 9–10 params to `ctx: &InterceptContext` plus their unique params (reader, writer, shutdown).

### Phase 1: `HttpBodyReader` struct

**Decision**: Extract the chunk-accumulation state machine into a small struct with a `push(&[u8]) -> bool` method (returns `true` when body is complete) instead of an `AsyncRead` wrapper or a free function.

Rationale: The state machine is synchronous (no await needed) — it only tracks indices into an already-read buffer. A struct with `push()` integrates into existing read loops without changing the async structure. An `AsyncRead` wrapper would require rewiring tokio I/O chains and changing function signatures beyond the refactor scope.

```rust
// src/proxy/intercept.rs
struct HttpBodyReader {
    header_done:    bool,
    content_length: Option<usize>,
    body_start:     usize,
    body_received:  usize,
}

impl HttpBodyReader {
    fn new() -> Self { … }
    /// Returns true when the full body has been received.
    fn push(&mut self, chunk: &[u8]) -> bool { … }
    fn body<'a>(&self, raw: &'a [u8]) -> &'a [u8] { … }
    fn reset(&mut self) { … }  // for keep-alive reuse
}
```

### Phase 2: Storage conv_cache

**Decision**: Use `Arc<RwLock<HashMap<String, PathBuf>>>` inside `Store` (which is already `Clone + Send + Sync` via an inner `Arc`).

Rationale: `Store` is cloned across tasks; the cache must be shared. `RwLock` is appropriate because reads (path lookups on every write) vastly outnumber writes (new conversations and rotation). The cache is populated synchronously at `Store::open()` by reading the existing log directory once — no lazy loading needed since open is already blocking.

Invalidation: `rotate_old()` removes files; the cache entries for rotated conversations are also removed. `insert_conversation()` inserts the new path into the cache at the same time as writing the file.

### Phase 3: `ProviderParser` trait

**Decision**: Define a simple local trait rather than reaching for `enum_dispatch` or `dyn Trait` dispatch through `Box`.

Rationale: There are only three providers (Anthropic, OpenAI, Google). Static dispatch via a local `match` on `Provider` (an enum) avoids a dependency and keeps compilation fast. The trait is used as a documentation/coherence device — each parser module implements it so the compiler enforces the interface, but call sites remain a `match` block that the trait subtypes satisfy.

```rust
// src/parser/mod.rs
pub(crate) trait ProviderParser {
    fn parse_request(body: &[u8]) -> Option<ParsedRequest>;
    fn extract_delta(event: &SseEvent) -> Option<String>;
    fn extract_tokens(event: &SseEvent) -> Option<(i64, i64)>;
}
```

### Phase 4: CA constant deduplication

**Decision**: Declare `pub(crate) const CA_ORG: &str` and `pub(crate) const CA_CN: &str` in `ca/mod.rs` and reference them from `cert_gen.rs`.

Rationale: The two files already share a module boundary (`ca/`). A `pub(crate)` const avoids any public API surface change. The MEMORY note confirms that mismatched DN parameters previously caused TLS failures — a single source of truth prevents recurrence.

## Risks / Trade-offs

| Risk | Likelihood | Mitigation |
|------|-----------|------------|
| `InterceptContext` clone cost if fields are large | Low — all fields are `Arc<…>` or cheap `Copy` types | Verify with `size_of::<InterceptContext>()` assert in tests |
| Cache coherence bug (stale path after rotate) | Low | `rotate_old()` and `insert_conversation()` both hold the write lock; covered by existing rotation tests |
| `ProviderParser` trait constrains future providers | Low | Trait is `pub(crate)`, not part of any public API |
| Phase sequencing: Phase 2 touches storage while `add-test-coverage` is also in progress | Medium | Phases 1 and 2 are independent; coordinate with add-test-coverage before merging Phase 2 |

## Migration Plan

Each phase is a standalone PR. Within each phase, changes are purely mechanical (rename, move, unwrap into struct). The acceptance gate for each phase is:

```
cargo build                    # no errors
cargo test                     # 175+ pass, 0 fail
cargo clippy -- -D warnings    # clean (no new suppressions)
```

Phase 4 (Config validation) adds new behavior (startup rejection of invalid configs). Any config file that was previously silently accepted with an invalid port may now fail `claudovka start`. This is intentional and desirable — document in release notes.

## Open Questions

- Phase 3 parser trait: should `extract_tokens` be part of the trait or remain an ad-hoc function? It is only called in `finalize_response()` and not all providers return token counts in the same event. Defer decision to Phase 3 implementation.
- Phase 4 Config validation: should `validate()` be called inside `Config::load()` (fail-fast on startup) or lazily when the specific sub-config is first used? Consensus: fail-fast at load time is safer — an invalid port is not recoverable at runtime.
