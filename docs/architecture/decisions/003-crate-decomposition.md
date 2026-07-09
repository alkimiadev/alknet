# ADR-003: Crate Decomposition

## Status

Accepted

## Context

The previous alknet-core crate was a monolith containing transport, interface, server, client, call, auth, config, socks5, credentials, and HTTP — all in one crate with interdependent modules. This created coupling (interface types depended on auth, server depended on call, everything depended on config) and made it impossible to use individual components independently.

The new ALPN dispatch model eliminates the need for a shared interface layer. Each handler is self-contained — it receives a byte stream and manages its own protocol. This naturally decomposes into separate crates.

Key constraints:
- Protocol crates must depend on alknet-core for auth/identity/config — but not on each other
- alknet-vault is already standalone (no alknet-core dependency) and must remain so (see ADR-008)
- The CLI binary assembles everything — it's the only crate that depends on all handler crates
- Handlers with protocol-agnostic cores (SFTP, call protocol) preserve the WASM door — browser clients can implement the wire format over WebTransport (see ADR-009, ADR-013)
- alknet-call includes the call protocol client and adapter traits, not just the server side — this enables alknet-agent and alknet-napi to use it for remote invocation
- Rust is the canonical implementation language. TypeScript is a reference/browser adaptation, not a parallel implementation (see ADR-013)

## Decision

The workspace decomposes into the following crates:

| Crate | Responsibility | Depends on |
|-------|---------------|------------|
| `alknet-core` | ProtocolHandler trait, ALPN router, endpoint, BiStream, AuthContext, IdentityProvider, config, ArcSwap dynamic config | tokio, quinn, rustls, iroh (feature-gated, added by ADR-010) |
| `alknet-vault` | Local key vault: BIP39/SLIP-0010/AES-GCM key derivation, encryption | (standalone, no alknet-core) |
| `alknet-ssh` | SshAdapter (russh, SOCKS5, port forwarding) | alknet-core, russh |
| `alknet-call` | CallAdapter (JSON-RPC via hand-rolled EventEnvelope framing, operation registry, pub/sub, access control, call protocol client, adapter traits) | alknet-core |
| `alknet-agent` | Agent service: LLM execution loop (forked aisdk), tool dispatch via call protocol, provider key retrieval via vault | alknet-call |
| `alknet-git` | GitAdapter (gix, pkt-line protocol) | alknet-core, gix |
| `alknet-sftp` | SftpAdapter (russh-sftp protocol core) | alknet-core, russh-sftp |
| `alknet-msg` | MessageAdapter (E2E encryption, mixnet) | alknet-core |
| `alknet-http` | HttpAdapter (axum, REST API, MCP endpoint) | alknet-core, axum |
| `alknet-dns` | DnsAdapter (hickory-proto, pkarr, service discovery) | alknet-core, hickory-proto |
| `alknet-napi` | Node.js native addon — thin NAPI projection of the call protocol client | alknet-call, napi-rs |
| `alknet` | CLI binary — registers handlers, starts endpoint | all handler crates, alknet-vault |

Dependency flow:
```
alknet-vault (standalone)
alknet-core ← all handler crates ← alknet (CLI)
alknet-call ← alknet-agent
alknet-call ← alknet-napi
```

No handler crate depends on another handler crate. Cross-handler communication goes through the call protocol (alknet-call) or through alknet-core's endpoint.

alknet-agent depends on alknet-call (not alknet-core directly) because it uses the call protocol client for tool dispatch and the operation registry for tool registration. It receives LLM provider keys through capabilities injected at the assembly layer (from alknet-vault), never from environment variables and never over the call protocol. See ADR-008 and ADR-014.

alknet-napi is a thin projection layer — it exposes the Rust call protocol client to Node.js via NAPI. It does not contain business logic or adapter implementations. See ADR-013.

## Consequences

**Positive:**
- Each handler can be developed, tested, and versioned independently
- WASM-compatible handlers (sftp, call) don't pull in heavy dependencies (russh, axum)
- alknet-vault remains standalone — no circular dependency risk
- New handlers are added by creating a crate and registering it with the endpoint
- Clean separation of concerns — each crate has one job

**Negative:**
- More crates to manage in the workspace — workspace Cargo.toml and version coordination
- Shared types (AuthContext, BiStream) must live in alknet-core — if they change, all handlers recompile
- The CLI binary has a large dependency tree (all handlers) — but this is expected for a binary that assembles everything
- Testing cross-handler behavior requires integration tests in the CLI or a test utility crate

## References

- Pivot proposal: `docs/research/pivot/alpn-service-architecture.md`
- ADR-001: ALPN-based protocol dispatch
- ADR-002: ProtocolHandler trait
- ADR-004: Auth as shared core (IdentityProvider)
- ADR-005: irpc as call protocol foundation (superseded by ADR-064)

## Amendments

### Amendment 1 (2026-06-29): `alknet-call` is a protocol-foundation crate

The Decision table lists `alknet-call` as a handler crate that "depends
on alknet-core, irpc." The dependency-flow diagram and the "No handler
crate depends on another handler crate" rule were written before
`alknet-http` (which implements `from_openapi`/`from_mcp`/`to_openapi`/
`to_mcp` and therefore needs `alknet-call`'s `OperationSpec`, `Handler`,
`HandlerRegistration`, and `OperationAdapter` trait) was specced.

**Clarification:** `alknet-call` is both a handler crate (it implements
`ProtocolHandler` on ALPN `alknet/call`) *and* the protocol-foundation
crate that `alknet-agent`, `alknet-napi`, and `alknet-http` consume for
the operation registry, adapter contract, and call client. The "no
handler crate depends on another handler crate" rule applies to peer
handler crates (e.g., `alknet-http` does not depend on `alknet-ssh`);
`alknet-call` is a protocol-foundation crate in the same spirit that
`alknet-core` is, just at a different layer (operations/RPC vs.
transport/auth/config).

`alknet-http` depending on `alknet-call` is "HTTP uses the call protocol
types," not "HTTP depends on SSH." This is within the spirit of this
ADR's decomposition. The `alknet-call` → `alknet-http` edge is recorded
in the `alknet-http` spec (`crates/http/overview.md`) and in the adapter
location map (`crates/call/client-and-adapters.md`).

### Amendment 2 (2026-07-07): alknet-tty does not depend on alknet-call

Amendment 1's protocol-foundation framing was extended to alknet-tty in
an earlier draft ("alknet-tty depends on alknet-call for the
`FrameFramedReader`/`FrameFramedWriter` framing utility"). A
pre-implementation sanity check found this was unsound:
`FrameFramedReader::read_frame()` is hardcoded to deserialize
`EventEnvelope` — the length-prefix read and the type-specific
deserialize are one entangled call, not a separable "framing utility."
alknet-tty's negotiation frame is a `NegotiateRequest`, not an
`EventEnvelope`, so `read_frame()` cannot return what alknet-tty needs;
the claimed reuse did not exist in a usable form.

**Clarification:** alknet-tty does **not** depend on alknet-call.
alknet-tty implements its own length-prefixed framing (~30 lines: 4-byte
big-endian length + UTF-8 JSON body) directly on tokio's
`AsyncRead`/`AsyncWrite`. The format coincides with alknet-call's
framing by convention (both are length-prefixed JSON); the
implementations are independent. The Amendment 1 protocol-foundation
exception remains for alknet-http/agent/napi (which use alknet-call's
`OperationSpec`/`Handler`/`OperationAdapter` types — actual type reuse,
not framing glue); it no longer covers alknet-tty. See
[ADR-057](057-alknet-tty-no-alknet-call-dep.md) for the full decision
and the three options considered (duplicate / promote to core / use
alknet-call).

### Amendment 3 (2026-07-09): irpc is not a dependency of any crate

The Decision table listed `irpc` as a dependency of `alknet-core` ("tokio,
quinn, rustls, irpc, iroh") and `alknet-call` ("alknet-core, irpc"). This
was carried over from the previous architecture and never verified against
the implementation: **no `.rs` file in the workspace ever imported irpc**.
The call protocol's wire format (`crates/alknet-call/src/protocol/wire.rs`)
is hand-rolled length-prefixed JSON; the `EventEnvelope` shape was derived
from the `@alkdev/pubsub` TypeScript prior art (ADR-013), not from irpc.
The dead `irpc` / `irpc-derive` workspace deps and the `alknet-call` consumer
dep were removed in commit `668d777`. See
[ADR-064](064-irpc-never-integrated-hand-rolled-framing.md) for the full
record (ADR-005, which accepted "irpc as the call protocol foundation," is
superseded).