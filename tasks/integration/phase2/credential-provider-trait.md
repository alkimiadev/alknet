---
id: credential-provider-trait
name: Define CredentialProvider trait, CredentialSet enum, and ConfigCredentialProvider implementation
status: pending
depends_on: [stream-interface-message-interface-split]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Define the `CredentialProvider` trait and `CredentialSet` enum in `alknet_core::credentials`, implementing the outbound authentication abstraction that complements the inbound `IdentityProvider`. This is the "Phase A" / "Phase 2.4a" work — the trait and enum must exist in core before alknet-secret (Phase 3) can wire `SecretStoreCredentialProvider` against them.

Per ADR-036 and research/phase2/credential-provider.md:

- `CredentialProvider` resolves **outbound** credentials: "how does alknet authenticate TO external services?"
- `CredentialSet` is a structured enum of credential types: `ApiKey`, `Basic`, `Bearer`, `S3AccessKey`, `OidcToken`, `Custom`
- `ConfigCredentialProvider` reads API keys and static credentials from `DynamicConfig` — the Phase 2 default (simple, no secret service dependency)
- `SecretStoreCredentialProvider` is a **stub** that returns `None` for all lookups until Phase 3 provides the alknet-secret dependency
- Wire `CredentialProvider` into `OperationEnv`/`OperationContext` so handlers can access credentials

**Relationship to IdentityProvider**: These are opposite-direction abstractions. `IdentityProvider` resolves inbound auth (who is calling alknet). `CredentialProvider` resolves outbound auth (how alknet calls others). Both live at the same architectural layer.

**Relationship to OperationEnv**: Handlers compose through `context.env`. The `OperationEnv` needs access to `CredentialProvider` so that handlers calling external services can resolve credentials. This could be a dedicated field on `OperationContext` or accessible through the `env` — the implementation detail is flexible, but the behavioral contract must match: given a service name, return credentials for that service.

## Acceptance Criteria

- [ ] `CredentialProvider` trait defined in `crates/alknet-core/src/credentials/mod.rs` with `get_credentials(&self, service: &str) -> Option<CredentialSet>` and `refresh_credentials(&self, service: &str) -> Option<CredentialSet>`
- [ ] `CredentialSet` enum defined with variants: `ApiKey { header_name, token }`, `Basic { username, password }`, `Bearer { token }`, `S3AccessKey { access_key, secret_key, session_token }`, `OidcToken { access_token, refresh_token, expires_at }`, `Custom { scheme, params }`
- [ ] `ConfigCredentialProvider` struct implemented — reads credentials from `DynamicConfig.auth` (or a new `DynamicConfig.credentials` section). For Phase 2, this is a simple config-backed lookup returning `CredentialSet::Bearer` or `CredentialSet::ApiKey` entries.
- [ ] `SecretStoreCredentialProvider` struct defined as a stub — `get_credentials()` always returns `None`. Full implementation deferred to Phase 3.
- [ ] `CredentialProvider` wired into `OperationContext` or `OperationEnv` so handlers can access outbound credentials
- [ ] `credentials` module re-exported from `crates/alknet-core/src/lib.rs`
- [ ] Unit test: `ConfigCredentialProvider` returns configured credentials for a service name
- [ ] Unit test: `ConfigCredentialProvider` returns `None` for unknown service names
- [ ] Unit test: `SecretStoreCredentialProvider` stub returns `None` for all service names
- [ ] Unit test: `OperationEnv`/`OperationContext` provides access to `CredentialProvider` from handler context
- [ ] `CredentialSet` derives `Clone`, `Debug`, `serde::Serialize`, `serde::Deserialize`

## References

- docs/architecture/decisions/036-credentialprovider-core-type.md — ADR-036
- docs/research/phase2/credential-provider.md — Full design rationale
- docs/research/integration-plan.md — Phase 2.4
- crates/alknet-core/src/auth/identity.rs — IdentityProvider (opposite direction, same pattern)
- crates/alknet-core/src/call/env.rs — OperationEnv
- crates/alknet-core/src/call/context.rs — OperationContext

## Notes

> This task is "2.4a" — the core types and config-backed implementation. "2.4b" (SecretStoreCredentialProvider backed by SecretProtocol::Decrypt) is deferred to Phase 3 when alknet-secret exists.

> For `ConfigCredentialProvider`, consider whether to add a `[[credentials]]` section to `DynamicConfig` or to reuse a subsection. The simplest Phase 2 approach is a new `credentials: HashMap<String, CredentialSet>` field on `DynamicConfig` that stores static bearer tokens/API keys from config.

> The `OperationEnv`/`OperationContext` wiring can follow either pattern: (a) a `credential_provider: Arc<dyn CredentialProvider>` field on `OperationContext`, or (b) `CredentialProvider` accessible through a registry-style `env.credentials(service)` method. The integration plan says "wire into OperationEnv so handlers can access credentials through context.env" — approach (b) aligns with the OperationEnv composition model. This is an implementation detail to resolve during implementation.

## Summary

> To be filled on completion