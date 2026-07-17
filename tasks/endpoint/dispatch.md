---
id: endpoint/dispatch
name: Implement public dispatch, build_auth_context, and ACME guard
status: completed
depends_on: [endpoint/endpoint-core]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Phase 2, Task 4 of the crate extraction. Implement the shared `dispatch` method,
`build_auth_context`, and the ACME `acme-tls/1` guard in
`crates/alknet-endpoint/src/dispatch.rs`.

`dispatch` is the shared dispatch path for every transport — the endpoint's own accept
loops call it after transport-specific extraction (ALPN, fingerprint, remote address),
and external dispatch callers (SSH channels, future WebTransport streams) call it after
their own extraction. It is **public** and **synchronous** (non-async): performs the
ACME guard, handler lookup, `build_auth_context`, and `tokio::spawn`s the handler.

### Types to extract / build

From `endpoint.rs` lines 330-490:

| Type/Function | Lines | Destination |
|---------------|-------|-------------|
| `dispatch_quinn()` | 330-365 | Adapted into `dispatch()` (public, transport-agnostic) |
| `extract_quinn_alpn()` | 368-378 | Stays in `accept/quinn.rs` (Task 5) |
| `extract_quinn_client_fingerprint()` | 381-388 | Stays in `accept/quinn.rs` (Task 5) |
| `dispatch_iroh()` | 440-466 | Adapted into `dispatch()` (public, transport-agnostic) |
| `extract_iroh_client_fingerprint()` | 469-472 | Stays in `accept/iroh.rs` (Task 6) |
| `build_auth_context()` | 475-490 | `dispatch.rs` |

### `dispatch` (public)

The new `dispatch` is transport-agnostic — it receives already-extracted values instead
of extracting them from a transport-specific connection:

```rust
/// Dispatch an accepted connection to its `ProtocolHandler` by ALPN.
///
/// Synchronous (non-async): performs the ACME guard, handler lookup,
/// `build_auth_context`, and `tokio::spawn`s the handler. Returns
/// immediately after spawning.
///
/// Public for connection-internal multiplexing shapes (SSH channels,
/// future WebTransport streams) that the endpoint can't own.
pub fn dispatch(
    &self,
    connection: Connection,
    alpn: Vec<u8>,
    fingerprint: Option<String>,
    remote_addr: Option<SocketAddr>,
) {
    // ACME guard
    #[cfg(feature = "acme")]
    if alpn == b"acme-tls/1" {
        debug!("acme-tls/1 challenge connection; closing");
        connection.close(0u32.into(), b"acme done");
        return;
    }

    let handler = match self.handlers.get(&alpn) {
        Some(h) => h.clone(),
        None => {
            connection.close(0u32.into(), b"no handler");
            warn!("dispatch: no handler for ALPN {:?}", String::from_utf8_lossy(&alpn));
            return;
        }
    };

    let auth = build_auth_context(&alpn, remote_addr, fingerprint, &self.identity_provider);
    tokio::spawn(async move {
        if let Err(e) = handler.handle(connection, &auth).await {
            error!("handler returned error: {e}");
        }
    });
}
```

Key differences from the old `dispatch_quinn` / `dispatch_iroh`:

1. **Takes `Connection` not `quinn::Connection` / `iroh::Connection`**: The accept loop
   converts the transport-specific connection to `alknet_core::Connection` before calling
   `dispatch`. This is the same pattern the old code used (`Connection::from_quinn_with_alpn`,
   `Connection::from_iroh`).

2. **Takes pre-extracted values**: `alpn`, `fingerprint`, `remote_addr` are passed in
   rather than extracted inside `dispatch`. The extraction is transport-specific and
   lives in the accept loop modules.

3. **No `EndpointError`**: Handler-not-found is swallowed (close + log). No error return.

4. **ACME guard is `#[cfg(feature = "acme")]`**: The `acme` feature is not on
   `alknet-endpoint` itself (the endpoint doesn't build ACME configs), but the guard
   is kept for forward-compatibility. If the assembly layer registers an ACME handler,
   the guard prevents it from being dispatched as a normal protocol handler.

### `build_auth_context`

Extracted from `endpoint.rs` lines 475-490, with imports updated:

```rust
pub(crate) fn build_auth_context(
    alpn: &[u8],
    remote_addr: Option<SocketAddr>,
    tls_client_fingerprint: Option<String>,
    identity_provider: &Arc<dyn IdentityProvider>,
) -> AuthContext {
    let identity = tls_client_fingerprint
        .as_ref()
        .and_then(|fp| identity_provider.resolve_from_fingerprint(fp));
    AuthContext {
        identity,
        alpn: alpn.to_vec(),
        remote_addr,
        tls_client_fingerprint,
    }
}
```

### What stays in core

The old `dispatch_quinn`, `dispatch_iroh`, and `build_auth_context` in `endpoint.rs` are
**not deleted** — they stay as duplicates. The prune happens in Phase 4.

## Acceptance Criteria

- [ ] `dispatch()` is public, synchronous, takes `&self`, `Connection`, `alpn`, `fingerprint`, `remote_addr`
- [ ] `dispatch()` performs ACME guard (`acme-tls/1` → close + return) when `acme` feature enabled
- [ ] `dispatch()` looks up handler by ALPN, closes connection + logs warning on miss
- [ ] `dispatch()` calls `build_auth_context` and `tokio::spawn`s the handler
- [ ] `build_auth_context()` resolves identity from fingerprint via `IdentityProvider`
- [ ] `build_auth_context()` returns `AuthContext` with all fields populated
- [ ] No `EndpointError` — handler-not-found is swallowed (close + log)
- [ ] All imports use `alknet_core::` (not `crate::`)
- [ ] Feature gates: `acme` guard is `#[cfg(feature = "acme")]`; rest is always available
- [ ] `cargo check -p alknet-endpoint` succeeds (all feature combos)
- [ ] `cargo clippy -p alknet-endpoint` succeeds with no warnings
- [ ] `cargo test -p alknet-core` still passes (old code untouched)

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 2, dispatch module
- docs/architecture/crates/endpoint/README.md — dispatch spec (lines 195-211)
- docs/architecture/decisions/083-endpoint-as-accept-loop-runner.md — ADR-083
- crates/alknet-core/src/endpoint.rs — lines 330-490 (source code to extract/adapt)

## Notes

> `dispatch` is the shared dispatch path for all transports. It's public because
> connection-internal multiplexing shapes (SSH channels, future WT streams) need to
> call it after their own extraction. The accept loops (quinn, iroh, TCP+TLS) call
> it internally. The old `dispatch_quinn` and `dispatch_iroh` are merged into one
> transport-agnostic `dispatch`. The old code in `endpoint.rs` is NOT deleted.

## Summary

> To be filled on completion
