---
status: deprecated
last_updated: 2026-07-15
---

# Endpoint (moved to `alknet-endpoint`)

> **This document is deprecated.** The `AlknetEndpoint` and
> `HandlerRegistry` types have been extracted from `alknet-core` into a
> new crate `alknet-endpoint` (ADR-083 Amendment 2026-07-15).
> `EndpointError` is removed (both variants were vestigial). The
> canonical spec is now
> [`crates/endpoint/README.md`](../endpoint/README.md).
>
> The shared types the endpoint imports (`ProtocolHandler`,
> `Connection`, `AuthContext`, `IdentityProvider`, `DynamicConfig`) stay
> in `alknet-core` — see [`core-types.md`](core-types.md),
> [`auth.md`](auth.md), [`config.md`](config.md).

## Historical summary

The endpoint was originally in `alknet-core/endpoint.rs` as the central
runtime type — a multi-transport accept-loop runner that dispatches
incoming connections by ALPN (ADR-010, ADR-083). ADR-082 extracted the
TLS setup code to `alknet-tls`; ADR-083 restructured the endpoint to
take pre-built transports via `with_quinn` / `with_iroh` /
`with_tcp_tls` (no TLS config); ADR-083 Amendment 2026-07-15 extracted
the endpoint itself into `alknet-endpoint` so that handler crates no
longer transitively link quinn/iroh/rcgen via core.

The endpoint's semantics — ALPN dispatch, `HandlerRegistry`, accept
loops, public `dispatch` for SSH/WT, graceful shutdown — are unchanged
by the extraction. See
[`crates/endpoint/README.md`](../endpoint/README.md) for the current
spec and [ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md)
for the full decision.

## What stayed in `alknet-core`

`Connection::from_quinn` / `from_iroh` stay in core's `types.rs` — they
are shared-type constructors used by both the endpoint's accept loop
(server) and `alknet-client`'s dial (client, ADR-089), gated on core's
`quinn` / `iroh` features. See
[ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md) §"The
`quinn` feature split".