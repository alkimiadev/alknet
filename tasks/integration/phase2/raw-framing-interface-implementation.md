---
id: raw-framing-interface-implementation
name: Implement RawFramingInterface accept/recv/send with first-frame auth
status: pending
depends_on: [stream-interface-message-interface-split]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Implement `RawFramingInterface` and `RawFramingSession` to handle length-prefixed `EventEnvelope` frames over a byte stream, with first-frame authentication. Currently `RawFramingInterface::accept()` returns an error and `RawFramingSession` stubs exist.

Per the integration plan section 2.2 and interface.md:

**RawFramingInterface**: Reads 4-byte length-prefixed JSON `EventEnvelope` frames from a transport stream (TCP, TLS, iroh, etc.). No SSH wrapping — the raw framing interface carries call protocol events directly.

**First-frame auth**: The first `InterfaceEvent` on a `RawFramingSession` carries an auth token in the `InterfaceEvent.identity` field or a dedicated auth event type. After `IdentityProvider::resolve_from_token()` verifies the token and produces an `Identity`, the session is authenticated. Subsequent frames are call protocol `EventEnvelope` data. If auth fails, the session is terminated immediately.

**Current state of the code**:
- `RawFramingInterface` accepts any `TransportStream` but returns an error
- `RawFramingSession` is an empty struct with stub `recv()` (returns `None`) and `send()` (returns error)
- `call::frame::{encode, decode, decode_with_remainder}` already implement the wire format
- `IdentityProvider::resolve_from_token()` exists but is not yet wired to `AuthToken` verification (that's coming in the API keys task)

**Implementation approach**:
1. `RawFramingInterface::accept()` takes a `TransportStream`, wraps it in a `BufReader` for buffered reading, stores it in `RawFramingSession`. The `RawFramingSession` is created in an "unauthenticated" state.
2. `RawFramingSession::recv()` reads frames from the stream:
   - If unauthenticated: read the first frame, extract the auth token, call `IdentityProvider::resolve_from_token()`. On success, transition to "authenticated" with the resolved `Identity`. On failure, return an error (session terminated).
   - If authenticated: read `EventEnvelope` frames, wrap in `InterfaceEvent::with_identity(envelope, identity)`.
3. `RawFramingSession::send()` writes `EventEnvelope` frames to the stream using `call::frame::encode`.

The `RawFramingSession` needs:
- A `BufReader<Box<dyn TransportStream>>` for reading framed data
- A `Box<dyn TransportStream>` (or WriteHalf) for writing framed data
- An `Option<Identity>` tracking auth state
- A reference to `IdentityProvider` for token resolution
- A buffer for partial frame reads (`decode_with_remainder` pattern)

## Acceptance Criteria

- [ ] `RawFramingInterface::accept()` takes a `TransportStream` and `StreamInterfaceConfig::RawFraming` config, creates a `RawFramingSession` wrapping the stream
- [ ] `RawFramingSession` holds a buffered reader and writer over the transport stream, an auth state (`Option<Identity>`), and a reference to `IdentityProvider`
- [ ] `RawFramingSession::recv()` reads length-prefixed `EventEnvelope` frames from the stream using `call::frame::decode_with_remainder`
- [ ] First-frame auth: the first `recv()` call resolves the auth token via `IdentityProvider::resolve_from_token()` and stores the resulting `Identity`
- [ ] Subsequent `recv()` calls produce `InterfaceEvent::with_identity(envelope, identity)` using the authenticated identity
- [ ] Auth failure terminates the session: `recv()` returns an error result on bad tokens
- [ ] `RawFramingSession::send()` writes `EventEnvelope` frames to the stream using `call::frame::encode`
- [ ] Unit test: `RawFramingInterface::accept()` succeeds with a valid stream
- [ ] Unit test: `RawFramingSession` round-trips an `EventEnvelope` through `send()` and `recv()` (after mock auth)
- [ ] Unit test: First-frame auth with a valid token transitions to authenticated state
- [ ] Unit test: First-frame auth with an invalid token returns an error
- [ ] Integration test: `RawFramingSession` over a `tokio::io::duplex` stream (simulated TCP) sends and receives multiple frames

## References

- docs/research/integration-plan.md — Phase 2.2
- docs/architecture/interface.md — RawFramingInterface, first-frame auth model
- crates/alknet-core/src/interface/raw_framing.rs — Current stubs
- crates/alknet-core/src/call/frame.rs — Frame encode/decode
- crates/alknet-core/src/auth/identity.rs — IdentityProvider, resolve_from_token

## Notes

> The frame format is already implemented and tested in `call::frame`. This task is primarily about wiring the frame reader/writer to the `InterfaceSession` trait and adding first-frame auth logic.

> Consider using `tokio::io::BufReader` for buffered reading and `tokio::io::BufWriter` for buffered writing. The `decode_with_remainder` function handles partial reads by returning how many bytes were consumed — the session needs to maintain a read buffer for reassembly.

> The `RawFramingInterface` config should include an `Arc<dyn IdentityProvider>` for first-frame auth. This follows the same pattern as `SshInterfaceConfig`.

## Summary

> To be filled on completion