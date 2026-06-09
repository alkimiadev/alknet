---
status: draft
last_updated: 2026-06-09
---

# Definitions: Terminology and Concept Disambiguation

## Purpose

Several terms are overloaded across alknet's architecture. This document defines
each term precisely and states the rule for using it in architecture specs. When
ambiguity is possible, specs must use the full qualifier.

This is a normative reference — other architecture documents link here rather
than repeating definitions inline.

## Term Definitions

### Interface (Layer 2)

An **Interface** consumes a Transport stream (Layer 1) or manages its own
transport, and produces call protocol sessions or handles discrete requests.
It is a _protocol parser_, not a network service.

Two subtypes:

| Subtype | Trait | Lifecycle | Transport ownership | Examples |
|---------|-------|-----------|---------------------|----------|
| `StreamInterface` | `accept(stream) → Session` | Long-lived session | Provided by caller | SshInterface, RawFramingInterface |
| `MessageInterface` | `handle_request(req) → Response` | Stateless per-request | Self-managed | HttpInterface, DnsInterface |

**Rule**: In alknet architecture docs, "Interface" (capitalized) refers to
Layer 2. Rust trait definitions use "trait" or "contract." Network URLs use
"endpoint." When discussing auth mechanisms per transport/interface pair, use
"credential presentation" (not "auth interface").

See: [interface.md](interface.md), ADR-035.

### Transport (Layer 1)

A **Transport** produces a byte stream (`AsyncRead + AsyncWrite + Unpin + Send`).
It is a _wire mechanism_, not a protocol. `TransportKind` enumerates:
`Tcp`, `Tls`, `Iroh`, `WebTransport`.

DNS is **not** a transport — it is a `MessageInterface` that manages its own
transport (UDP/TCP port 53).

**Rule**: Never use "transport" to refer to HTTP, DNS, or any protocol that
doesn't produce a `TransportStream`. Use "MessageInterface" instead.

See: [transport.md](transport.md), ADR-026, ADR-035.

### Service (irpc service)

An **irpc service** is an in-cluster, Rust-to-Rust service defined by an irpc
protocol enum. Dispatched by enum variant with postcard serialization. Examples:
`AuthProtocol`, `SecretProtocol`, `ConfigProtocol`.

**Rule**: Always qualify: "irpc service" (in-cluster, enum-dispatched),
"application service" (operation-registered handler), or "external service"
(third-party endpoint). Never use bare "service" in architecture docs.

See: [services.md](services.md), ADR-028, ADR-033.

### Operation (call protocol)

An **operation** is a path-based handler registered in `OperationRegistry`,
dispatched by `namespace + name`. Cross-node, cross-language, JSON
`EventEnvelope` framing.

**Rule**: Use "operation" for call protocol handlers. Use "irpc service method"
for enum-dispatched calls. These are different dispatch mechanisms unified by
OperationEnv.

See: [call-protocol.md](call-protocol.md), ADR-033.

### Identity (core type)

The `Identity` struct `{ id, scopes, resources }` represents an authenticated
principal. Produced by `IdentityProvider` (inbound auth resolution).

| Identity field | Config-backed auth | Database-backed auth |
|---------------|-------------------|---------------------|
| `id` | SSH key fingerprint | Account UUID |
| `scopes` | From authorized_keys entry | From peer_credentials + ACL |
| `resources` | From authorized_keys entry | From organization membership |

**Rule**: "Identity" (capitalized, code font) = the alknet struct. "identity
service" = a full identity management system (Keystone, etc.). Never conflate
the two.

See: [identity.md](identity.md), ADR-029.

### IdentityProvider (inbound auth)

`IdentityProvider` resolves **inbound** authentication: given a credential
(fingerprint or token), produce an `Identity`.

**Direction**: Inbound (who is calling alknet).

**Rule**: Never use "IdentityProvider" to describe outbound auth. That is
`CredentialProvider`.

See: [identity.md](identity.md), ADR-029.

### CredentialProvider (outbound auth)

`CredentialProvider` resolves **outbound** credentials: given a service name,
produce a `CredentialSet` for authenticating _to_ that service.

**Direction**: Outbound (how alknet calls others).

**Rule**: Never use "CredentialProvider" for inbound auth. That is
`IdentityProvider`.

See: [credentials.md](credentials.md), ADR-036.

### AuthToken

`AuthToken = base64url(key_id || timestamp || signature)` — an Ed25519-signed
timestamp token used for non-SSH auth. Self-signed by the client, verified
server-side.

**Rule**: Use "AuthToken" (capitalized) for this specific format. Use "API key"
for hash-verified bearer tokens. Never use bare "token" in architecture docs.

See: [auth.md](auth.md), ADR-023.

### API Key

A hash-verified bearer token with a prefix like `alk_...`. Simpler than
AuthToken (no Ed25519 key pair needed). Stored as SHA-256 hash in
`DynamicConfig.auth.api_keys` or `api_keys` table.

**Rule**: Always "API key" (two words) for hash-verified bearer tokens.
"AuthToken" for Ed25519-signed tokens.

See: [auth.md](auth.md), ADR-037.

### Domain Event vs Integration Event

| Type | Scope | Serialization | Example |
|------|-------|---------------|---------|
| Domain event | Within a service boundary | Any format (Honker streams) | `KeyRotated`, `InventoryAdjusted` |
| Integration event | Across service or node boundaries | JSON `EventEnvelope` | `call.requested`, `UserCreated` |

irpc service calls are synchronous request-response, not events.

**Rule**: "Domain event" for internal Honker streams. "Integration event" for
call protocol `EventEnvelope`. "irpc call" for synchronous in-cluster calls.
Per ADR-032, domain events never cross service boundaries without projection.

See: ADR-032, [services.md](services.md).

### Scope

A permission string attached to an `Identity`. Flat strings like
`"relay:connect"`, `"secrets:derive"`. Used by `ForwardingPolicy` and
operation-level ACL.

**Rule**: Use "scope" for `Identity.scopes` flat strings. Use "resource" for
`Identity.resources` entries. Do not conflate with hierarchical role models
unless explicitly noting a comparison to Keystone.

See: [identity.md](identity.md), ADR-031.

### OperationRegistry

The central registry mapping `(namespace, operation_name)` to handlers and
specs. All interfaces resolve to the same registry.

**Rule**: "OperationRegistry" for this specific data structure. "Service
catalog" only when explicitly comparing to Keystone or similar external systems.

See: [call-protocol.md](call-protocol.md), ADR-025.

### Credential Presentation

The mechanism by which credentials are presented on each (Transport, Interface)
pair:

| (Transport, Interface) | Credential presentation | Resolves via |
|----------------------|----------------------|-------------|
| (TLS, SSH) | SSH key handshake | `resolve_from_fingerprint()` |
| (TCP, SSH) | SSH key handshake | `resolve_from_fingerprint()` |
| (iroh, SSH) | SSH key handshake | `resolve_from_fingerprint()` |
| (TLS, raw framing) | AuthToken in frame header | `resolve_from_token()` |
| (TCP, raw framing) | AuthToken in frame header | `resolve_from_token()` |
| (WebTransport, raw framing) | AuthToken in CONNECT request | `resolve_from_token()` |
| (—, HTTP) | `Authorization: Bearer` header | `resolve_from_token()` |
| (—, DNS) | AuthToken in query labels | `resolve_from_token()` |

**Rule**: Use "credential presentation" for the mechanism of presenting
credentials on a specific (Transport, Interface) pair. Not "auth interface"
(which overloads "Interface").

See: [auth.md](auth.md), [interface.md](interface.md).

## Cross-cutting Open Questions

These questions affect multiple specs and need resolution before or during
Phase 2 implementation:

- **OQ-DEF-03**: Should `Identity.scopes` be hierarchical (Keystone implied roles)
  or stay flat? Recommendation: Stay flat. Add implied scope resolution in
  alknet-storage when multi-tenant deployment requires it.

- **OQ-DEF-07**: Should the on-chain `IdentityProvider` be a separate impl or a
  `CredentialProvider` extension? Recommendation: Separate `IdentityProvider`
  impl (`OnChainIdentityProvider`). `IdentityProvider` resolves inbound auth,
  not outbound credentials.

- **OQ-DEF-08**: Should "credential presentation" replace overloaded "interface" in
  auth contexts? Recommendation: Yes. Adopted in this document.

See: [open-questions.md](open-questions.md) for tracking.

## References

- [interface.md](interface.md) — StreamInterface / MessageInterface
- [auth.md](auth.md) — AuthToken, credential presentation per interface
- [identity.md](identity.md) — Identity, IdentityProvider
- [credentials.md](credentials.md) — CredentialProvider, CredentialSet
- [services.md](services.md) — irpc services vs application services
- [call-protocol.md](call-protocol.md) — Operations, OperationEnv
- [research/phase2/definitions.md](../research/phase2/definitions.md) — Full research with cross-domain mappings