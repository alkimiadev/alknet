# ADR-019: `--proxy` Has Different Semantics on Client vs Server

## Status
Accepted

## Context
The `--proxy` CLI flag appears on both `alknet connect` (client) and `alknet serve` (server), but the two sides proxy fundamentally different things:

- **Client**: `--proxy` routes the *transport connection* through the proxy. For example, `alknet connect --transport iroh --proxy socks5://127.0.0.1:1080` means the iroh endpoint's outbound TCP connections go through the specified SOCKS5 proxy before reaching the iroh relay. The proxy wraps the transport layer.

- **Server**: `--proxy` routes *outbound target connections* through the proxy. For example, `alknet serve --proxy socks5://127.0.0.1:9050` means when an SSH client opens a `direct_tcpip` channel to `db.internal:5432`, the server connects to that target through the specified proxy. The proxy wraps the data-plane connections.

Using the same flag name for both is intentional — from the user's perspective, both mean "route traffic through a proxy." But the layer at which the proxy operates differs, and this needs to be explicit so implementers don't confuse the two.

ADR-010 addressed transport chaining for the client side only. The server-side outbound proxy behavior has no ADR. This ADR documents both semantics and the rationale for sharing the flag name.

## Decision
The `--proxy` flag uses the same name on client and server, with documented different semantics:

| Side | Flag | What gets proxied | Example |
|------|------|-------------------|---------|
| Client | `--proxy` | Transport connection (outbound to server/relay) | `--transport iroh --proxy socks5://...` → iroh endpoint connects through proxy |
| Server | `--proxy` | Outbound target connections (data plane) | `--proxy socks5://...` → direct_tcpip targets reached through proxy |

On the **client**, `--proxy` affects the transport layer. It only applies to transports that make outbound TCP connections (iroh through a proxy, TLS through a proxy). For plain TCP transport, `--proxy` has no meaningful effect since the transport is already a direct TCP connection — use the SOCKS5 server instead.

On the **server**, `--proxy` affects the data plane. All `channel_open_direct_tcpip` outbound connections are routed through the proxy, regardless of transport mode.

This is not a naming collision — it's the same conceptual operation ("route through a proxy") at different layers. The shared name avoids forcing users to learn two proxy flags.

## Consequences
- **Positive**: One flag name (`--proxy`) instead of two. Users already understand "proxy" as "route through this."
- **Positive**: Client-side proxy is minimal implementation — iroh's endpoint builder accepts proxy config natively.
- **Positive**: Server-side proxy is straightforward — all outbound TCP from channel handlers goes through the proxy.
- **Negative**: Implementers must read the correct spec (client vs server) to understand what `--proxy` does for their side. This is mitigated by CLI help text that clearly describes the behavior per side.
- **Negative**: On the client, `--proxy` with `--transport tcp` is effectively a no-op (the transport is already a direct TCP connection to the server). The CLI should handle this case gracefully.

## References
- [ADR-010](010-transport-chaining-cli.md) — client-side transport chaining
- [transport.md](../transport.md) — transport layer spec
- [client.md](../client.md) — client CLI
- [server.md](../server.md) — server outbound proxy