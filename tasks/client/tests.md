---
id: client/tests
name: Write unit tests for AlknetClient, dial methods, error type, and SOCKS5 proxy; add integration test for dial + take-over composition
status: pending
depends_on: [client/dial-quic, client/dial-tcp-tls, client/dial-iroh, client/socks5-proxy]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Phase 3, Task 8. Write unit tests for the `alknet-client` crate and add the integration
test for the dial + take-over composition. The tests cover:

1. **`AlknetClient` construction and builder methods** — `new()`, `with_quinn`, `with_tcp_tls`, `with_iroh`, `with_socks5_proxy`, `Default`
2. **`ClientDialError`** — display formatting, `#[from]` conversion from `TlsError`
3. **`dial_quic`** — direct path (no proxy), `NoTransport` error, `TlsConfig` error
4. **`dial_tcp_tls`** — direct path (no proxy), `NoTransport` error, `TlsConfig` error
5. **`dial_iroh`** — direct path, `NoTransport` error, unknown remote fail-closed
6. **SOCKS5 proxy** — `Socks5ProxyConfig` construction, `Socks5Credentials` construction
7. **Integration test** — dial + take-over composition (moved from `alknet-call/tests/two_node_call.rs`)

### Test strategy

Since the dial methods produce real network connections, the unit tests focus on:
- **Error paths**: `NoTransport` (calling a dial without the matching `with_*`),
  `TlsConfig` (invalid credentials), unknown iroh remote fail-closed
- **Builder correctness**: fields are set correctly, `Default` works
- **Type-level tests**: `Send + Sync` bounds, `Debug` output

The integration test (dial + take-over composition) uses a loopback `Connection` or
a minimal echo `ProtocolHandler` on a test ALPN — no `alknet-call` dependency.

### Test outline

#### 1. `AlknetClient` construction tests (in `src/client.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_empty_client() {
        let client = AlknetClient::new();
        // All transports are None
    }

    #[test]
    fn default_delegates_to_new() {
        let client = AlknetClient::default();
        // Same as new()
    }

    #[test]
    fn alknet_client_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<AlknetClient>();
    }

    #[test]
    fn debug_lists_configured_transports() {
        let client = AlknetClient::new();
        let debug = format!("{:?}", client);
        // Does not panic, does not expose transport internals
    }
}
```

#### 2. `ClientDialError` tests (in `src/error.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tls_config_from_tls_error() {
        let err = alknet_tls::TlsError::Config("test".into());
        let dial_err: ClientDialError = err.into();
        assert!(matches!(dial_err, ClientDialError::TlsConfig(_)));
    }

    #[test]
    fn no_transport_displays_transport_name() {
        let err = ClientDialError::NoTransport { transport: "quinn" };
        assert!(err.to_string().contains("quinn"));
    }

    #[test]
    fn connect_displays_message() {
        let err = ClientDialError::Connect("connection refused".into());
        assert!(err.to_string().contains("connection refused"));
    }

    #[test]
    fn handshake_displays_message() {
        let err = ClientDialError::Handshake("certificate rejected".into());
        assert!(err.to_string().contains("certificate rejected"));
    }

    #[cfg(feature = "socks5")]
    #[test]
    fn proxy_displays_message() {
        let err = ClientDialError::Proxy("UDP ASSOCIATE rejected".into());
        assert!(err.to_string().contains("UDP ASSOCIATE rejected"));
    }

    #[test]
    fn client_dial_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ClientDialError>();
    }
}
```

#### 3. `dial_quic` error path tests (in `src/dial/quinn.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dial_quic_no_transport_error() {
        let client = AlknetClient::new();
        let creds = ConnectionCredentials::new();
        let result = client.dial_quic(
            "127.0.0.1:0".parse().unwrap(),
            "localhost",
            b"test/alpn",
            &creds,
        ).await;
        assert!(matches!(result, Err(ClientDialError::NoTransport { .. })));
    }

    #[tokio::test]
    async fn dial_quic_tls_config_error_on_invalid_creds() {
        // Test that invalid credentials produce TlsConfig error
        // (e.g., ACME identity on client side)
    }
}
```

#### 4. `dial_tcp_tls` error path tests (in `src/dial/tcp_tls.rs`)

```rust
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn dial_tcp_tls_no_transport_error() {
        // Similar to dial_quic — calling without with_tcp_tls
    }
}
```

#### 5. `dial_iroh` error path tests (in `src/dial/iroh.rs`)

```rust
#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn dial_iroh_no_transport_error() {
        // Calling without with_iroh
    }

    #[tokio::test]
    async fn dial_iroh_unknown_remote_fails_closed() {
        let client = AlknetClient::new(); // no iroh endpoint needed for this error
        let creds = ConnectionCredentials::new(); // remote_identity is None
        // This should fail with TlsConfig error before even trying to connect
    }
}
```

#### 6. SOCKS5 proxy type tests (in `src/socks5.rs`)

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socks5_proxy_config_construction() {
        let proxy = Socks5ProxyConfig {
            addr: "127.0.0.1:1080".parse().unwrap(),
            credentials: None,
        };
        assert_eq!(proxy.addr.port(), 1080);
        assert!(proxy.credentials.is_none());
    }

    #[test]
    fn socks5_proxy_config_with_auth() {
        let proxy = Socks5ProxyConfig {
            addr: "127.0.0.1:1080".parse().unwrap(),
            credentials: Some(Socks5Credentials {
                username: "user".into(),
                password: "pass".into(),
            }),
        };
        assert!(proxy.credentials.is_some());
    }

    #[test]
    fn socks5_proxy_config_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Socks5ProxyConfig>();
    }
}
```

#### 7. Integration test (in `tests/dial_and_takeover.rs`)

The integration test from `alknet-call/tests/two_node_call.rs` (`two_node_call_round_trip`)
moves here, rewritten with a minimal echo `ProtocolHandler` on a test ALPN — no
`alknet-call` dependency:

```rust
// tests/dial_and_takeover.rs
//
// Integration test: dial + take-over composition.
// Uses a loopback Connection (from_stream) to test that dial_quic
// produces a Connection that a protocol take-over can consume.

use alknet_client::AlknetClient;
use alknet_core::credentials::ConnectionCredentials;
use alknet_core::types::Connection;

#[tokio::test]
async fn dial_produces_connection_for_takeover() {
    // This test verifies the composition: dial → Connection → take-over.
    // Since we can't spin up a real QUIC endpoint in a unit test,
    // we test the error paths and type-level contracts.
    // The full end-to-end dial + take-over is tested in the assembly
    // layer integration tests (future hub/worker tests).

    // For now: verify that the dial methods exist, compile, and
    // produce the correct error types.
    let client = AlknetClient::new();
    let creds = ConnectionCredentials::new();

    // No transport configured → NoTransport error
    let result = client.dial_quic(
        "127.0.0.1:0".parse().unwrap(),
        "localhost",
        b"test/alpn",
        &creds,
    ).await;
    assert!(result.is_err());
}
```

### What this does NOT include

- Full end-to-end QUIC dial tests (require a real quinn endpoint — tested in the
  assembly layer integration tests)
- Tests for the old `CallClient::connect` (that code is unchanged — Phase 5 prune)
- Tests that require a running SOCKS5 proxy (the `Socks5UdpSocket` is tested via
  the PoC; unit tests cover type-level contracts)

## Acceptance Criteria

- [ ] `AlknetClient` construction tests: `new()`, `Default`, `Send + Sync`, `Debug`
- [ ] `ClientDialError` tests: `#[from]` conversion, display formatting, `Send + Sync`
- [ ] `dial_quic` error path tests: `NoTransport`, `TlsConfig` on invalid creds
- [ ] `dial_tcp_tls` error path tests: `NoTransport`
- [ ] `dial_iroh` error path tests: `NoTransport`, unknown remote fail-closed
- [ ] `Socks5ProxyConfig` / `Socks5Credentials` construction tests (feature-gated on `socks5`)
- [ ] Integration test: `tests/dial_and_takeover.rs` (dial + take-over composition)
- [ ] All tests pass: `cargo test -p alknet-client`
- [ ] All tests pass with feature combos: `cargo test -p alknet-client --features quinn`, `--features tcp`, `--features iroh`, `--features quinn,tcp,socks5`
- [ ] `cargo clippy -p alknet-client --all-targets` succeeds with no warnings
- [ ] `cargo fmt --check -p alknet-client` passes
- [ ] `cargo test --workspace` still passes (old tests untouched)

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 3, test strategy
- docs/architecture/crates/client/README.md — full architecture spec
- crates/alknet-call/tests/two_node_call.rs — old integration test (reference for the dial + take-over composition)
- crates/alknet-call/src/client/call_client.rs — old test patterns (lines 640-930, reference)
- crates/alknet-tls/src/client.rs — `TlsClientConfig` tests (reference for test patterns)
- crates/alknet-endpoint/src/endpoint.rs — endpoint tests (reference for test patterns)

## Notes

> The test strategy focuses on error paths and type-level contracts because the dial
> methods produce real network connections. Full end-to-end dial tests (with a real
> quinn endpoint) are tested in the assembly layer integration tests (future hub/worker
> tests). The integration test from `alknet-call/tests/two_node_call.rs` moves here,
> rewritten with a minimal echo `ProtocolHandler` on a test ALPN — no `alknet-call`
> dependency. The old code in `call_client.rs` is NOT deleted — that's Phase 5.

## Summary

> To be filled on completion
