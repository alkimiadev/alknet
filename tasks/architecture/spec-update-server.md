---
id: architecture/spec-update-server
name: Update server.md — add DynamicConfig, ForwardingPolicy, IdentityProvider references
status: pending
depends_on:
  - architecture/adr-030-static-dynamic-config-split
  - architecture/adr-031-forwarding-policy
  - architecture/adr-028-auth-irpc-service
  - architecture/adr-026-transport-interface-separation
  - architecture/spec-configuration
  - architecture/spec-identity
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Update `docs/architecture/server.md` to reflect the architectural changes from Phase 1: DynamicConfig, ForwardingPolicy in channel handling, IdentityProvider replacing direct ServerAuthConfig reads, and the interface abstraction concept.

**Phase boundary note**: Phase 1 ships `ConfigIdentityProvider` (ArcSwap-backed) as the only `IdentityProvider` implementation. The irpc `AuthProtocol` and `StorageIdentityProvider` are contracted in the specs but not built yet. Server.md should describe what the server actually does in Phase 1 — reading auth from `ArcSwap<DynamicConfig>` via `ConfigIdentityProvider` — with a forward reference to identity.md for the full trait hierarchy. Don't describe irpc service wiring or SQLite-backed auth as if they exist.

The current server.md is thorough but reflects the alpha architecture where auth is read directly from `ServerAuthConfig` and there's no forwarding policy concept.

**Changes needed**:
1. Update Authentication section: auth goes through `IdentityProvider` trait (reference identity.md, ADR-029), with `ConfigIdentityProvider` as the Phase 1 impl reading from `ArcSwap<DynamicConfig>` (reference ADR-030). Note that `StorageIdentityProvider` is a future implementation.
2. Add ForwardingPolicy check in Channel Handling section: before proxy spawn, evaluate ForwardingPolicy against Identity (reference configuration.md, ADR-031)
3. Replace `Arc<ServerAuthConfig>` with `Arc<ArcSwap<DynamicConfig>>` in ServerHandler description (reference ADR-030)
4. Add note about Interface abstraction: SSH is one interface (Layer 2), ServerHandler logic maps to SshInterface (reference interface.md, ADR-026) — but detail is in interface.md, not here
5. Update CLI interface section: mention `--config` flag for TOML config, `[[listeners]]` for multi-transport
6. Update constraint about single transport: "Currently binds to a single transport" → note that multi-transport is coming per ADR-030

**What stays the same**: TLS cert provisioning, stealth mode, outbound proxy modes, logging/rate limiting, graceful shutdown, error handling, most CLI flags.

## Acceptance Criteria

- [ ] Authentication section updated: references `IdentityProvider` trait with `ConfigIdentityProvider` as Phase 1 impl, notes `StorageIdentityProvider` as future
- [ ] Channel Handling section updated: ForwardingPolicy check before proxy spawn, reference ADR-031
- [ ] ServerHandler struct updated: `Arc<ArcSwap<DynamicConfig>>`, not `Arc<ServerAuthConfig>`
- [ ] Note added about Interface abstraction pointing to interface.md and ADR-026
- [ ] CLI section mentions `--config` flag (TOML) and `[[listeners]]` for multi-transport
- [ ] Single-transport constraint softened (noted as current, changing per ADR-030)
- [ ] Phase boundary clear: what ships in Phase 1 vs what's contracted for later
- [ ] `last_updated` in YAML frontmatter updated
- [ ] ADR table updated with references to 026, 028, 029, 030, 031
- [ ] References section updated to include configuration.md, identity.md, interface.md

## References

- docs/architecture/server.md — current content to update
- docs/architecture/decisions/030-static-dynamic-config-split.md
- docs/architecture/decisions/031-forwarding-policy.md
- docs/architecture/decisions/028-auth-irpc-service.md
- docs/architecture/decisions/026-transport-interface-separation.md

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion