# ADR-037: API Keys as DynamicConfig Auth

## Status
Accepted

## Context

Alknet's token auth uses Ed25519-signed `AuthToken`s — the same key material
used for SSH auth. This is appropriate for interactive clients (browsers, CLI)
that can generate and sign Ed25519 key pairs.

But for service accounts, automation, and simple integrations, Ed25519 key
pairs are inconvenient. A dashboard backend, a CI/CD pipeline, or a monitoring
script needs a simple bearer token that can be stored in an environment variable
or config file without managing cryptographic key pairs.

The HTTP interface (Phase 2+) requires bearer token auth for `Authorization:
Bearer <token>` headers. `AuthToken` works but requires client-side Ed25519
signing. API keys offer a simpler alternative: short bearer tokens verified by
SHA-256 hash lookup, with optional scope restrictions and TTL.

## Decision

Add `[[auth.api_keys]]` section to `DynamicConfig`:

```toml
[[auth.api_keys]]
prefix = "alk_"
hash = "sha256:abc..."
scopes = ["relay:connect", "secrets:derive"]
description = "dashboard service account"
ttl = "30d"  # optional
```

`ConfigIdentityProvider::resolve_from_token()` handles both token types:
- If the input starts with the configured prefix (default `alk_`), treat it as
  an API key: hash it with SHA-256 and look up the hash in the `api_keys` table.
- Otherwise, treat it as an `AuthToken`: decode, verify Ed25519 signature,
  check timestamp, resolve from `authorized_keys`.

Both paths produce the same `Identity` result. In database-backed deployments,
both resolve to the same account UUID.

API keys are stored as SHA-256 hashes (like password hashing — the cleartext
key is never stored, only its hash). The prefix enables O(1) routing between
AuthToken and API key verification without trying both paths.

The full key is provided to the client exactly once (at creation time). Subsequent
verifications only compare hashes.

## Consequences

**Positive**: Simple bearer token auth for HTTP and other non-SSH interfaces.
No cryptographic key management for service accounts. Consistent with industry
practice (Stripe, GitHub, AWS all use prefixed API keys).

**Positive**: Both AuthTokens and API keys go through `resolve_from_token()`.
The caller doesn't need to know which type they're using. This keeps the
authentication layer unified.

**Positive**: Scoped API keys enable fine-grained access control for service
accounts. A monitoring tool gets `["monitoring:read"]`, not full access.

**Negative**: API keys are bearer tokens — anyone who obtains the key has the
associated permissions. The hash storage and optional TTL mitigate but do not
eliminate this risk. Ed25519 AuthTokens remain the preferred auth method for
interactive clients.

**Negative**: API key rotation requires updating `DynamicConfig` (or the
`api_keys` database table). The `ConfigReloadHandle` / `ConfigService` reload
mechanism handles this, but it's a deliberate operation, not automatic.

**Negative**: No rate limiting on API key verification is built into this ADR.
Rate limiting on the HTTP interface is a separate concern.

## References

- ADR-023 (unified auth, shared key material)
- ADR-029 (Identity as core type)
- ADR-030 (static/dynamic config split)
- [auth.md](../auth.md) — Token auth, AuthPolicy, API keys
- [configuration.md](../configuration.md) — DynamicConfig, AuthPolicy
- [research/phase2/interface-model.md](../../research/phase2/interface-model.md) — API keys in config