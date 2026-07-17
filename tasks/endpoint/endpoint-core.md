---
id: endpoint/endpoint-core
name: Implement AlknetEndpoint struct, new, builder methods, run, and shutdown
status: completed
depends_on: [endpoint/registry]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Phase 2, Task 3 of the crate extraction. Implement the `AlknetEndpoint` struct and its
core methods (`new`, `with_quinn`, `with_iroh`, `with_tcp_tls`, `run`, `shutdown`,
`shutdown_sender`) in `crates/alknet-endpoint/src/endpoint.rs`.

This is a **fresh build against the ADR-083 shape**, not a direct copy of the old
`endpoint.rs`. The old `AlknetEndpoint::new()` took a `StaticConfig` and built transports
internally. The new `new()` takes no `StaticConfig` and no TLS config — the assembly layer
builds transports and hands them to the endpoint via builder methods.

### Target shape (per ADR-083 / architecture spec)

```rust
pub struct AlknetEndpoint {
    #[cfg(feature = "quinn")]
    quinn: Option<quinn::Endpoint>,
    #[cfg(feature = "iroh")]
    iroh: Option<iroh::Endpoint>,
    #[cfg(feature = "tcp")]
    tcp_tls: Option<TcpTlsListener>,       // (TcpListener, TlsAcceptor)
    handlers: Arc<HandlerRegistry>,
    dynamic: Arc<ArcSwap<DynamicConfig>>,
    identity_provider: Arc<dyn IdentityProvider>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    drain_timeout: Duration,
}

impl AlknetEndpoint {
    pub fn new(
        handlers: HandlerRegistry,
        dynamic: Arc<ArcSwap<DynamicConfig>>,
        identity_provider: Arc<dyn IdentityProvider>,
        drain_timeout: Duration,
    ) -> Self;

    #[cfg(feature = "quinn")]
    pub fn with_quinn(mut self, endpoint: quinn::Endpoint) -> Self;

    #[cfg(feature = "iroh")]
    pub fn with_iroh(mut self, endpoint: iroh::Endpoint) -> Self;

    #[cfg(feature = "tcp")]
    pub fn with_tcp_tls(
        mut self,
        listener: tokio::net::TcpListener,
        acceptor: tokio_rustls::TlsAcceptor,
    ) -> Self;

    pub fn shutdown_sender(&self) -> watch::Sender<bool>;

    pub async fn run(self: Arc<Self>);

    /// Infallible — signals all owned accept loops to stop, waits for
    /// in-flight handlers with drain_timeout, then forcefully closes.
    pub async fn shutdown(&self);
}
```

### Key differences from the old `endpoint.rs`

1. **`new()` takes no `StaticConfig`**: The old `new()` read `listen_addr`, `tls_identity`,
   `iroh_relay` from `StaticConfig` and built transports internally. The new `new()` takes
   only `HandlerRegistry`, `DynamicConfig`, `IdentityProvider`, and `drain_timeout` — no
   transport construction. The assembly layer reads `StaticConfig` and builds transports.

2. **Builder methods instead of internal construction**: `with_quinn(endpoint)`,
   `with_iroh(endpoint)`, `with_tcp_tls(listener, acceptor)` replace the internal
   `TlsSetup::new()` + `build_quinn_server_config_from_rustls()` + `build_iroh_endpoint()`
   chain. The endpoint receives pre-built, pre-bound transports.

3. **`TcpTlsListener` type**: A new type alias for the TCP+TLS transport pair:
   ```rust
   #[cfg(feature = "tcp")]
   pub(crate) type TcpTlsListener = (tokio::net::TcpListener, tokio_rustls::TlsAcceptor);
   ```

4. **`shutdown()` is infallible**: Returns `()` not `Result<(), EndpointError>`. The old
   `shutdown()` returned `Result` but could never actually fail (the `?` was on
   `iroh.close().await` which is infallible). The new `shutdown()` is `async fn shutdown(&self)`
   with no `Result`.

5. **No `EndpointError`**: The error type is removed entirely. `BindFailed` is vestigial
   (the endpoint doesn't bind). `HandlerNotFound` is swallowed by `dispatch` (close + log).
   `TlsConfig` was already removed by ADR-083.

6. **No `acme_state_handle` field**: ACME state lives on `TlsServerConfig` in `alknet-tls`
   now. The endpoint doesn't see it.

7. **`run()` spawns accept loops for each active transport**: Quinn, iroh, and TCP+TLS
   each get their own `tokio::spawn`'d accept loop. The old `run()` only handled quinn
   and iroh; the new one adds TCP+TLS.

### `run()` implementation

```rust
pub async fn run(self: Arc<Self>) {
    let mut tasks: Vec<tokio::task::JoinHandle<()>> = Vec::new();

    #[cfg(feature = "quinn")]
    if let Some(quinn) = &self.quinn {
        let quinn = quinn.clone();
        let handlers = self.handlers.clone();
        let identity_provider = self.identity_provider.clone();
        let mut shutdown_rx = self.shutdown_rx.clone();
        tasks.push(tokio::spawn(async move {
            crate::accept::quinn::run_accept_loop(quinn, handlers, identity_provider, &mut shutdown_rx).await;
        }));
    }

    #[cfg(feature = "iroh")]
    if let Some(iroh) = &self.iroh {
        // ... same pattern for iroh
    }

    #[cfg(feature = "tcp")]
    if let Some((listener, acceptor)) = self.tcp_tls.take() {
        // ... same pattern for TCP+TLS
    }

    for task in tasks {
        let _ = task.await;
    }
}
```

### `shutdown()` implementation

```rust
pub async fn shutdown(&self) {
    let _ = self.shutdown_tx.send(true);

    #[cfg(feature = "quinn")]
    if let Some(quinn) = &self.quinn {
        quinn.close(0u32.into(), b"shutdown");
    }

    #[cfg(feature = "iroh")]
    if let Some(iroh) = &self.iroh {
        iroh.close().await;
    }

    #[cfg(feature = "tcp")]
    // TCP+TLS: the accept loop watches shutdown_rx; no explicit close needed
    // (the listener is dropped when the endpoint is dropped)

    tokio::time::sleep(self.drain_timeout).await;

    #[cfg(feature = "quinn")]
    if let Some(quinn) = &self.quinn {
        quinn.wait_idle().await;
    }
}
```

### What stays in core

The old `AlknetEndpoint` in `endpoint.rs` lines 118-277 is **not deleted** — it stays as a
duplicate. The prune happens in Phase 4. This task only adds code to `alknet-endpoint`.

## Acceptance Criteria

- [ ] `AlknetEndpoint` struct defined with all fields (quinn, iroh, tcp_tls, handlers, dynamic, identity_provider, shutdown_tx/rx, drain_timeout)
- [ ] `AlknetEndpoint::new()` takes `HandlerRegistry`, `Arc<ArcSwap<DynamicConfig>>`, `Arc<dyn IdentityProvider>`, `Duration` — no `StaticConfig`, no TLS config
- [ ] `with_quinn(endpoint)` builder method (feature-gated on `quinn`)
- [ ] `with_iroh(endpoint)` builder method (feature-gated on `iroh`)
- [ ] `with_tcp_tls(listener, acceptor)` builder method (feature-gated on `tcp`)
- [ ] `TcpTlsListener` type alias defined (feature-gated on `tcp`)
- [ ] `shutdown_sender()` returns a clone of the shutdown watch sender
- [ ] `run()` spawns accept loops for each active transport
- [ ] `shutdown()` is infallible (`async fn shutdown(&self)`, no `Result`)
- [ ] `Debug` impl for `AlknetEndpoint` (lists handlers, drain_timeout; no transport internals)
- [ ] No `EndpointError` type (removed)
- [ ] No `acme_state_handle` field (ACME lives in `alknet-tls`)
- [ ] No `has_iroh_identity` function (transport-building decision moved to assembly layer)
- [ ] Feature gates correct: `quinn`, `iroh`, `tcp` each gate their respective fields/methods
- [ ] `cargo check -p alknet-endpoint` succeeds (all feature combos)
- [ ] `cargo clippy -p alknet-endpoint` succeeds with no warnings
- [ ] `cargo test -p alknet-core` still passes (old code untouched)

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 2, endpoint module
- docs/architecture/crates/endpoint/README.md — AlknetEndpoint spec (lines 48-103)
- docs/architecture/decisions/083-endpoint-as-accept-loop-runner.md — ADR-083
- crates/alknet-core/src/endpoint.rs — lines 118-277 (old code, reference only)

## Notes

> This is the core structural task of Phase 2. The `AlknetEndpoint` is built fresh
> against the ADR-083 shape — it's not a copy-paste of the old code. The key
> difference: the old `new()` built transports internally from `StaticConfig`; the
> new `new()` takes no transport config and receives pre-built transports via
> builder methods. `EndpointError` is removed entirely. `shutdown()` is infallible.
> The old code in `endpoint.rs` is NOT deleted — that's Phase 4.

## Summary

> To be filled on completion
