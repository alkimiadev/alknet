---
status: draft
last_updated: 2026-06-07
---

# Alknet Architecture

## Current State

Architecture specification in active development. Phase 0 foundation ADRs
completed (026–034). New spec documents created for identity, services,
interface, configuration, storage, flowgraph, and secret service. Existing
specs updated for the three-layer model, crate decomposition, and unified
identity. See [open-questions.md](open-questions.md) for remaining open
questions.

## Architecture Documents

| Document | Status | Description |
|----------|--------|-------------|
| [overview.md](overview.md) | reviewed | Package purpose, exports, dependencies |
| [transport.md](transport.md) | reviewed | Transport abstraction: TCP, TLS, iroh |
| [auth.md](auth.md) | draft | Unified auth: SSH + token, IdentityProvider trait |
| [call-protocol.md](call-protocol.md) | draft | Bidirectional call/event protocol, operation registry |
| [client.md](client.md) | reviewed | Client connection, SOCKS5, port forwarding |
| [server.md](server.md) | reviewed | Server acceptance, channel handling, proxy |
| [tun-shim.md](tun-shim.md) | deprecated | TUN interface wrapper — **deferred**, use tun2proxy |
| [napi-and-pubsub.md](napi-and-pubsub.md) | reviewed | NAPI wrapper and pubsub event target adapter |
| [identity.md](identity.md) | draft | Identity type, IdentityProvider trait, auth flows |
| [services.md](services.md) | draft | irpc service layer, OperationEnv, three dispatch paths |
| [interface.md](interface.md) | draft | Layer 2: Interface trait, SshInterface, RawFramingInterface |
| [configuration.md](configuration.md) | draft | StaticConfig, DynamicConfig, forwarding policy, reload |
| [storage.md](storage.md) | draft | alknet-storage: metagraph, identity, ACL, honker |
| [flowgraph.md](flowgraph.md) | draft | alknet-flowgraph: call graph, operation graph, petgraph |
| [secret-service.md](secret-service.md) | draft | alknet-secret: BIP39, SLIP-0010, AES-GCM, SecretProtocol |

## Research Documents

| Document | Status | Description |
|----------|--------|-------------|
| [configuration.md](../research/configuration.md) | draft | Configuration architecture (source for promoted spec) |
| [core.md](../research/core.md) | draft | Core overview, transport, call protocol, DNS |
| [services.md](../research/services.md) | draft | irpc service protocols, OperationContext, application services |
| [storage.md](../research/storage.md) | draft | Metagraph, identity, ACL, secrets, honker |
| [flow.md](../research/flow.md) | draft | FlowGraph, operation graph, call graph, petgraph mapping |
| [integration-plan.md](../research/integration-plan.md) | draft | Phased integration plan for services, pubsub, and operations |

## ADR Table

| ADR | Title | Status |
|-----|-------|--------|
| [001](decisions/001-pluggable-transport.md) | Pluggable transport via `AsyncRead+AsyncWrite` trait | Accepted |
| [002](decisions/002-tun-separate-process.md) | TUN shim as separate process | Superseded by ADR-014 |
| [003](decisions/003-iroh-stream-join.md) | iroh stream via `tokio::io::join` | Accepted |
| [004](decisions/004-ssh-over-transport.md) | SSH runs over transport, not alongside | Accepted |
| [005](decisions/005-socks5-before-tun.md) | SOCKS5 as primary interface, TUN as add-on | Accepted |
| [006](decisions/006-no-logging-of-tunnel-destinations.md) | No logging of tunnel destinations | Accepted |
| [007](decisions/007-napi-single-stream.md) | NAPI exposes single duplex stream | Accepted |
| [008](decisions/008-acme-lets-encrypt.md) | ACME/Let's Encrypt certificate provisioning | Accepted |
| [009](decisions/009-default-iroh-relay.md) | Default iroh relay with override | Accepted |
| [010](decisions/010-transport-chaining-cli.md) | Transport chaining in CLI | Accepted |
| [011](decisions/011-no-ssh-config-programmatic-api.md) | Programmatic-first API, no file-based config | Accepted |
| [012](decisions/012-auth-ed25519-and-cert-authority.md) | Ed25519 keys + OpenSSH cert-authority, no password auth | Accepted |
| [013](decisions/013-fail2ban-friendly-logging.md) | Fail2ban-friendly logging + built-in rate limiting | Accepted |
| [014](decisions/014-defer-tun-recommend-socks5-proxy.md) | Defer TUN, recommend local SOCKS5 + tun2proxy | Accepted |
| [015](decisions/015-napi-rs-for-ffi-bridge.md) | napi-rs for FFI bridge | Accepted |
| [016](decisions/016-napi-expose-connect-and-serve.md) | NAPI exposes both connect() and serve() | Accepted |
| [017](decisions/017-stealth-mode-protocol-multiplexing.md) | Stealth mode — protocol multiplexing on port 443 | Accepted |
| [018](decisions/018-control-channel-for-pubsub.md) | Control channel for pubsub over SSH | Accepted |
| [019](decisions/019-proxy-dual-semantics.md) | `--proxy` dual semantics (client vs server) | Accepted |
| [023](decisions/023-unified-auth-shared-key-material.md) | Unified auth with shared key material + token auth | Accepted |
| [024](decisions/024-bidirectional-call-protocol.md) | Bidirectional call protocol (EventEnvelope) | Accepted |
| [025](decisions/025-handler-spec-separation.md) | Handler/spec separation for downstream service registration | Accepted |
| [026](decisions/026-transport-interface-separation.md) | Transport/interface separation (three-layer model) | Accepted |
| [027](decisions/027-crate-decomposition.md) | Crate decomposition (core, secret, storage, flowgraph) | Accepted |
| [028](decisions/028-auth-irpc-service.md) | Auth as irpc service behind feature flag | Accepted |
| [029](decisions/029-identity-core-type.md) | Identity as core type in alknet-core | Accepted |
| [030](decisions/030-static-dynamic-config-split.md) | Static/dynamic config split with ArcSwap | Accepted |
| [031](decisions/031-forwarding-policy.md) | Forwarding policy with rule-based allow/deny | Accepted |
| [032](decisions/032-event-boundary-discipline.md) | Event boundary discipline (domain, irpc, call protocol) | Accepted |
| [033](decisions/033-operationenv-irpc-call-protocol.md) | OperationEnv as universal composition mechanism | Accepted |
| [034](decisions/034-head-worker-terminology.md) | Head/worker terminology replacing hub/spoke | Accepted |

## Open Questions

See [open-questions.md](open-questions.md) for all open and resolved questions.
Key resolved questions from Phase 0: OQ-12, OQ-16, OQ-18 (forwarding policy
and identity scopes), OQ-17 (transport-aware auth), OQ-23 (irpc feature flag),
OQ-24 (DNS control channel scope), OQ-25 (crate irpc dependencies). Key open
questions: OQ-15 (QUIC coexistence), OQ-19 (WebTransport TLS), OQ-20 (worker
registration).

## Lifecycle Definitions

| Status | Meaning | Transitions |
|--------|---------|-------------|
| `draft` | Under active development. May change significantly. | → `reviewed` when open questions resolved |
| `reviewed` | Architecture final. Implementation may begin. Changes require review. | → `stable` when implementation verified |
| `stable` | Locked. Changes require review and may warrant an ADR. | → `deprecated` when superseded |
| `deprecated` | Superseded. Kept for reference. | Removed when no longer referenced |