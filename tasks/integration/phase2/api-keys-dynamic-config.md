---
id: api-keys-dynamic-config
name: Add API keys to DynamicConfig.auth and extend IdentityProvider token resolution
status: pending
depends_on: [credential-provider-trait]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Add `[[auth.api_keys]]` support to `DynamicConfig` and extend `ConfigIdentityProvider::resolve_from_token()` to verify API keys alongside existing AuthTokens. API keys are shorter, simpler bearer strings (hash-verified, with optional TTL and scopes) for service accounts and automation — they don't require Ed25519 key pairs like AuthTokens do.

Per ADR-037 and research/phase2/interface-model.md (Config section):

**API Key format**: `alk_<random_prefix>_<secret>` (or similar). Storage uses SHA-256 hash of the full key. Lookup is by prefix (first N characters), then hash verification of the full key.

**Config format**:
```toml
[[auth.api_keys]]
prefix = "alk_dGhl"
hash = "sha256:abc123..."
scopes = ["relay:connect"]
description = "dashboard service account"
```

**Key changes**:
- Add `ApiKeyEntry` struct: `prefix`, `hash`, `scopes`, `description`, `optional ttl/expires_at`
- Add `api_keys: Vec<ApiKeyEntry>` to `AuthPolicy` (or a separate section on `DynamicConfig`)
- Extend `ConfigIdentityProvider::resolve_from_token()` to check API keys: prefix match → hash verification → return `Identity`
- API keys produce `Identity { id: "<prefix>", scopes: <from entry>, resources: {} }`
- The `AuthToken` path (Ed25519 signed timestamp) is unchanged — both go through the same `resolve_from_token()` method, discriminated by format/prefix

**Why this is Phase 2**: The HTTP interface (task 2.7) needs bearer token auth, and API keys are the simplest mechanism for `IdentityProvider::resolve_from_token()`. Without this, HTTP auth has no config-based auth mechanism.

## Acceptance Criteria

- [ ] `ApiKeyEntry` struct defined with `prefix`, `hash`, `scopes`, `description`, `expires_at: Option<u64>` fields
- [ ] `AuthPolicy` gains an `api_keys: Vec<ApiKeyEntry>` field (or `DynamicConfig` gains a separate `api_keys` section)
- [ ] `ConfigIdentityProvider::resolve_from_token()` checks API keys: matches prefix, verifies SHA-256 hash of the full token, returns `Identity` on success
- [ ] API key lookup: tokens starting with `alk_` (or configured prefix) are treated as API keys; others go through the `AuthToken` verification path
- [ ] Expired API keys (where `expires_at` is set and in the past) are rejected
- [ ] API key scopes propagate to the returned `Identity.scopes` field
- [ ] `DynamicConfig::default()` includes an empty `api_keys` list (no behavioral change)
- [ ] `ConfigReloadHandle` reloads API keys along with the rest of `AuthPolicy`
- [ ] Unit test: valid API key authenticates via `resolve_from_token()`
- [ ] Unit test: expired API key is rejected
- [ ] Unit test: wrong hash is rejected
- [ ] Unit test: unknown prefix is rejected (falls through to AuthToken path)
- [ ] Unit test: API key scopes appear in the resolved `Identity`
- [ ] All existing auth tests continue to pass (no behavioral change for SSH key auth)

## References

- docs/architecture/decisions/037-api-keys-dynamic-config.md — ADR-037
- docs/research/phase2/interface-model.md — API keys in config, auth table
- docs/research/integration-plan.md — Phase 2.6
- crates/alknet-core/src/config/dynamic_config.rs — DynamicConfig, AuthPolicy
- crates/alknet-core/src/auth/identity.rs — ConfigIdentityProvider, IdentityProvider trait

## Notes

> The prefix match approach means we don't store the full API key in config — just the first ~8 chars for fast lookup and the SHA-256 hash for verification. This mirrors how GitHub/personal access tokens work.

> Consider whether `api_keys` should live on `AuthPolicy` or be a separate section. Putting it on `AuthPolicy` keeps all auth-related config together and ensures atomic reloads. The `ConfigIdentityProvider` already has access to `Arc<ArcSwap<DynamicConfig>>` so it can read both `authorized_keys` and `api_keys` from the same reload.

> The `resolve_from_token()` method currently takes `&AuthToken` — API keys are NOT AuthTokens (they're simple bearer strings). The method signature may need to accept a generic `&str` or a new enum that can be either an AuthToken string or an API key string. Alternatively, `resolve_from_token()` can accept `&str` and internally discriminate by prefix/format.

## Summary

> To be filled on completion