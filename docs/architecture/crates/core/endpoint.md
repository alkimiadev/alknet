---
status: deprecated
last_updated: 2026-07-17
---

# Endpoint (in `alknet-endpoint`)

> **This document is deprecated.** The `AlknetEndpoint` and
> `HandlerRegistry` types live in a separate crate, `alknet-endpoint`
> (ADR-083 Amendment 2026-07-15). `EndpointError` is removed (both
> variants were vestigial). The canonical spec is
> [`crates/endpoint/README.md`](../endpoint/README.md).
>
> The shared types the endpoint imports (`ProtocolHandler`,
> `Connection`, `AuthContext`, `IdentityProvider`, `DynamicConfig`) are
> in `alknet-core` — see [`core-types.md`](core-types.md),
> [`auth.md`](auth.md), [`config.md`](config.md).

## What is in `alknet-core`

`Connection::from_quinn` / `from_iroh` are in core's `types.rs` — they
are shared-type constructors used by both the endpoint's accept loop
(server, in `alknet-endpoint`) and `alknet-client`'s dial (client,
ADR-089), gated on core's `quinn` / `iroh` features. See
[ADR-083](../../decisions/083-endpoint-as-accept-loop-runner.md) §"The
`quinn` feature split".