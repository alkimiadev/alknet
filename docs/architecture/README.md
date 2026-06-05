---
status: draft
last_updated: 2026-06-04
---

# Alknet Architecture

## Current State

Architecture specification in active development. 22 ADRs accepted. Unified
auth and call protocol architecture being specified — see [auth.md](auth.md)
and [call-protocol.md](call-protocol.md). Configuration architecture under
exploration — see [research/configuration.md](../research/configuration.md).

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

## Research Documents

| Document | Status | Description |
|----------|--------|-------------|
| [configuration.md](../research/configuration.md) | draft | Configuration architecture: static/dynamic split, hot reload, forwarding policy |

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

## Open Questions

Most open questions have been resolved. Open questions remain for
configuration, auth, and call protocol — see
[open-questions.md](open-questions.md) for details.

## Lifecycle Definitions

| Status | Meaning | Transitions |
|--------|---------|-------------|
| `draft` | Under active development. May change significantly. | → `reviewed` when open questions resolved |
| `reviewed` | Architecture final. Implementation may begin. Changes require review. | → `stable` when implementation verified |
| `stable` | Locked. Changes require review and may warrant an ADR. | → `deprecated` when superseded |
| `deprecated` | Superseded. Kept for reference. | Removed when no longer referenced |