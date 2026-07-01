---
id: http/client/shared-http-client
name: Implement shared HTTP client (ClientWithMiddleware + retry + Retry-After, OQ-40)
status: completed
depends_on: [http/crate-init]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Implement the shared HTTP client in `src/client/http_client.rs`. This is
the `reqwest_middleware::ClientWithMiddleware` used by all
`from_openapi`/`from_mcp` forwarding handlers. The client owns connection
pooling, keep-alive, TLS, and a retry stack. It is constructed once and
reused across all forwarding handlers; credential injection happens
per-request (from `OperationContext.capabilities`), not at client
construction — the client is shared across all operations, the
credentials are per-call.

### The middleware stack (OQ-40 resolved)

The shared type is `reqwest_middleware::ClientWithMiddleware`, not a
bare `reqwest::Client` — both retry and Retry-After are middleware on the
stack, and middleware requires the `ClientWithMiddleware` wrapper. The
stack has two layers:

1. **`RetryTransientMiddleware`** (from `reqwest-retry`) — exponential
   backoff on transient failures (connection errors, 5xx). The "retry N
   times with increasing intervals" part. Configured via an
   `ExponentialBackoff` policy at client construction.

2. **Inlined `RetryAfterMiddleware`** — parses the `Retry-After` header
   on 429/503 and sleeps before the next request to that URL. The
   "respect what the server told you" part. **Inlined** (MIT, ~50 lines
   of real logic) from `melotic/reqwest-retry-after`, not pulled as a
   dependency: the crate is complementary to `reqwest-retry` (whose
   default strategy does not honor `Retry-After`), and inlining lets the
   upstream's unbounded `HashMap<Url, SystemTime>` storage be bounded
   for a long-running process.

### Pooling, keep-alive, TLS

Pooling, keep-alive, and TLS come from `reqwest::ClientBuilder` defaults;
outbound TLS uses the system trust store (standard HTTPS to external
APIs like OpenAI, Anthropic). Custom CA bundle + client certs are an
optional config for self-hosted API gateways (two-way-door
implementation detail; the credential comes from `Capabilities`, the TLS
trust comes from the system).

### Hot-reload (rebuild-and-swap)

Hot-reload of the pooling/retry config is **rebuild-and-swap**: a config
change rebuilds the `ClientWithMiddleware` and swaps it via `ArcSwap`
(the same pattern `ConfigIdentityProvider` uses, ADR-035). A rebuild
drops the connection pool / keep-alive state, which is acceptable — a
config change wanting a fresh pool is the case that triggers it. The
retry policy is baked into the middleware at `ClientBuilder::build()`
time; live policy mutation is not supported by `reqwest-retry`, so cheap
per-policy updates are not part of the model.

### API

```rust
/// Configuration for the shared HTTP client (two-way-door, OQ-40 resolved).
pub struct HttpClientConfig {
    /// Pool max idle connections per host (reqwest default if None).
    pub pool_max_idle_per_host: Option<usize>,
    /// Request timeout (reqwest default if None).
    pub request_timeout: Option<Duration>,
    /// Retry policy for transient failures (ExponentialBackoff).
    pub retry_policy: ExponentialBackoff,
    /// Custom CA bundle path for self-hosted API gateways (optional).
    pub ca_bundle: Option<PathBuf>,
    /// Client cert for mutual TLS (optional, from Capabilities at call time,
    /// not here — this is the trust config, not the credential).
    pub client_cert: Option<ClientCertConfig>,
}

/// The shared HTTP client. Constructed once, reused across all forwarding
/// handlers. Wrapped in `ArcSwap` for rebuild-and-swap hot-reload.
pub struct SharedHttpClient {
    inner: ArcSwap<ClientWithMiddleware>,
    config: ArcSwap<HttpClientConfig>,
}

impl SharedHttpClient {
    pub fn new(config: HttpClientConfig) -> Self { ... }

    /// Get the current client (for forwarding handlers to use).
    pub fn client(&self) -> Arc<ClientWithMiddleware> {
        self.inner.load_full()
    }

    /// Rebuild the client with new config (rebuild-and-swap hot-reload).
    pub fn reload(&self, config: HttpClientConfig) { ... }
}
```

### The inlined `RetryAfterMiddleware`

The inlined middleware is ~50 lines: a bounded `HashMap<Url, SystemTime>`
holding per-URL retry-after deadlines, a `before` hook that sleeps if the
deadline is in the future, and an `after` hook that parses the
`Retry-After` header on 429/503 and records the deadline. The bound
(e.g., LRU eviction at N entries) prevents unbounded growth in a
long-running process — the upstream's unbounded `HashMap<Url, SystemTime>`
is the reason it's inlined rather than depended on.

### Downstream layering boundary

The agent crate's provider SSE normalization sits on top of this
`ClientWithMiddleware`: it consumes the `reqwest::Response` stream the
forwarding handler produces and emits `call.responded` events. It does
not replace the client or own transport/pooling/retry. `alknet-http`
owns transport; the agent crate owns provider-specific SSE →
Vercel-UI-message mapping. The aisdk `core/client.rs` reference for HTTP
client construction is *not* carried forward — its env-var config and
hand-rolled retry are the anti-patterns discarded in favor of the
middleware stack above.

## Acceptance Criteria

- [ ] `SharedHttpClient` struct in `src/client/http_client.rs`
- [ ] Wraps `ArcSwap<ClientWithMiddleware>` for rebuild-and-swap
- [ ] `HttpClientConfig` with pool, timeout, retry policy, optional CA bundle
- [ ] `RetryTransientMiddleware` (from `reqwest-retry`) in the middleware stack
- [ ] Inlined `RetryAfterMiddleware` (~50 lines, bounded `HashMap<Url, SystemTime>`)
- [ ] `RetryAfterMiddleware` parses `Retry-After` header on 429/503
- [ ] `RetryAfterMiddleware` sleeps before next request to a URL with an active deadline
- [ ] `RetryAfterMiddleware` storage is bounded (LRU eviction or equivalent)
- [ ] Pooling/keep-alive/TLS from `reqwest::ClientBuilder` defaults
- [ ] System trust store for outbound TLS by default
- [ ] Custom CA bundle optional (self-hosted API gateways)
- [ ] `reload(config)` rebuilds and swaps via `ArcSwap`
- [ ] Credential injection is per-request (NOT at client construction)
- [ ] No `std::env::var` reads (no-env-vars invariant — ADR-014)
- [ ] No shared global client (alknet-http owns its client)
- [ ] Unit test: `client()` returns a usable `ClientWithMiddleware`
- [ ] Unit test: `reload()` swaps the client (new client returned by `client()`)
- [ ] Unit test: `RetryAfterMiddleware` records deadline from `Retry-After: <seconds>`
- [ ] Unit test: `RetryAfterMiddleware` records deadline from `Retry-After: <HTTP-date>`
- [ ] Unit test: bounded storage evicts old entries (no unbounded growth)
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings

## References

- docs/architecture/crates/http/http-adapters.md — HTTP client (§"HTTP client (reqwest)")
- docs/architecture/crates/http/overview.md — OQ-40 resolved (ClientWithMiddleware + middleware stack)
- docs/architecture/decisions/014-secret-material-flow-and-capability-injection.md — ADR-014 (no env vars)
- https://docs.rs/reqwest-retry/ — RetryTransientMiddleware / ExponentialBackoff
- https://github.com/melotic/reqwest-retry-after — RetryAfterMiddleware source (MIT, inlined, not a dependency)

## Notes

> The shared HTTP client is the no-env-vars-compliant outbound transport.
> The middleware stack (RetryTransientMiddleware + inlined
> RetryAfterMiddleware) is the resolved shape (OQ-40). The
> RetryAfterMiddleware is inlined (not a dependency) so its storage can
> be bounded for a long-running process — the upstream's unbounded
> HashMap is the reason. Hot-reload is rebuild-and-swap via ArcSwap (same
> pattern as ConfigIdentityProvider, ADR-035). Credential injection is
> per-request (from OperationContext.capabilities), not at client
> construction — the client is shared, the credentials are per-call. The
> agent crate's SSE normalization sits on top of this client; it does not
> replace it.

## Summary

> Implemented SharedHttpClient (ArcSwap<ClientWithMiddleware>) with HttpClientConfig
> (pool/timeout/retry/optional CA bundle+client cert), RetryTransientMiddleware from
> reqwest-retry, and inlined RetryAfterMiddleware (~90 lines, bounded HashMap with LRU
> eviction by earliest deadline, parses Retry-After seconds + HTTP-date, sleeps before
> next request on 429/503). reload() rebuilds and swaps via ArcSwap. No env-var reads;
> per-request credential injection only. Added arc-swap, httpdate, http, url deps +
> rustls TLS feature to reqwest. 24 unit tests. Build/clippy/test all clean.