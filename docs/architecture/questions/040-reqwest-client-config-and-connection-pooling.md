# OQ-40: reqwest Client Config and Connection Pooling

- **Origin**: [http-adapters.md](crates/http/http-adapters.md),
  [http-mcp.md](crates/http/http-mcp.md), the alknet-http Phase 0
  findings DH-7
- **Status**: resolved (2026-06-30)
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: `alknet-http` owns a shared HTTP client constructed
  once and reused across all `from_openapi`/`from_mcp` forwarding
  handlers. The client carries connection pooling, keep-alive, TLS,
  and a retry stack. The config shape is:

  | Aspect | Decision |
  |--------|----------|
  | Shared client type | `reqwest_middleware::ClientWithMiddleware` (not a bare `reqwest::Client`) — required because both retry and Retry-After are middleware on the stack |
  | Middleware stack | `RetryTransientMiddleware` (from `reqwest-retry` — exponential backoff on transient failures: connection errors, 5xx) + inlined `RetryAfterMiddleware` (parses the `Retry-After` header on 429/503 and sleeps before the next request to that URL) |
  | `Retry-After` handler | Inlined from `melotic/reqwest-retry-after` (MIT, ~50 lines of real logic). The crate is complementary to `reqwest-retry`, not a replacement — `reqwest-retry`'s default strategy does not honor `Retry-After`, which is why the separate middleware exists. Inlining lets the unbounded `HashMap<Url, SystemTime>` storage in the upstream crate be bounded (the melotic version grows without limit over a long-running process). |
  | Pooling / keep-alive / TLS | `reqwest::ClientBuilder` defaults; system trust store for outbound HTTPS (standard calls to OpenAI, Anthropic, etc.) |
  | Hot-reload | Rebuild-and-swap the `ClientWithMiddleware` via `ArcSwap` (same pattern as `ConfigIdentityProvider`, ADR-035). A rebuild drops the connection pool / keep-alive state — acceptable, since a config change wanting a fresh pool is the case that triggers it. Retry policy is baked into the middleware at `ClientBuilder::build()` time; live policy mutation is not supported by `reqwest-retry` (no cheap per-policy update path exists). |
  | Credentials | Per-request from `OperationContext.capabilities` — see the one-way constraints below |

  The one-way constraints (settled before this OQ, restated unchanged):
  (1) `alknet-http` owns its HTTP client — no env-var-based client
  config, no shared global client; (2) credential injection happens
  per-request (from `OperationContext.capabilities`), not at client
  construction — the client is shared across all operations, the
  credentials are per-call; (3) TLS for outbound calls uses the
  system trust store by default (custom CA bundle + client certs are
  an optional config for self-hosted API gateways).

  **Downstream layering boundary (so the agent crate doesn't
  accidentally re-invent a client).** The agent crate's provider SSE
  normalization (replicating the solid part of aisdk's pattern — the
  Vercel-UI-message normalization that maps different providers' SSE
  to a common shape) sits *on top of* this `ClientWithMiddleware`: it
  consumes the `reqwest::Response` stream the forwarding handler
  produces and emits `call.responded` events. It does not replace the
  client or own transport/pooling/retry. `alknet-http` owns transport;
  the agent crate owns provider-specific SSE → Vercel-UI-message
  mapping. The aisdk `core/client.rs` reference for HTTP client
  construction is *not* carried forward — its env-var config and
  hand-rolled retry are the anti-patterns being discarded; the
  aisdk/`@alkdev/operations/src/from_openapi.ts` SSE *normalization*
  pattern is separate and stays referenced in the forwarding-handler
  section of [http-adapters.md](crates/http/http-adapters.md).

  No ADR — the decision is internal to `alknet-http`: the client type
  does not cross crate boundaries (`alknet-call` never sees reqwest),
  the library choice is reversible, and it does not touch the
  system's structure, constraints, or API surface across crates.
- **Cross-references**: ADR-014, ADR-017, ADR-035,
  [http-adapters.md](crates/http/http-adapters.md),
  [http-mcp.md](crates/http/http-mcp.md)
