---
id: meta/auth-layer
name: Complete auth layer — error types, key loading, server auth, client auth
status: completed
depends_on:
  - auth/error-types
  - auth/key-loading
  - auth/server-auth-handler
  - auth/client-auth-handler
scope: system
risk: medium
impact: phase
level: planning
---

## Description

Meta task that clusters all auth module tasks. Once complete, the auth layer provides key loading from file or memory, server-side Ed25519 key + cert-authority validation, and client-side key-based authentication.

## Acceptance Criteria

- [ ] All auth tasks completed
- [ ] Key loading supports file paths and in-memory data in OpenSSH format
- [ ] Server accepts Ed25519 keys and cert-authority signed certificates
- [ ] Client presents Ed25519 key pairs
- [ ] Error types cover transport, auth, channel, and config failures

## References

- docs/architecture/client.md, docs/architecture/server.md

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion