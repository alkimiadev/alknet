---
id: napi/project-setup
name: Set up alknet-napi project with napi-rs build tooling and TypeScript types
status: pending
depends_on:
  - setup/project-init
scope: moderate
risk: low
impact: component
level: implementation
---

## Description

Set up the napi-rs project for the `@alkdev/alknet` Node.js native addon. This includes the napi-rs build configuration, TypeScript type definitions, and the package structure.

Per ADR-015 and ADR-016: napi-rs is the FFI bridge, and the wrapper exposes `connect()` and `serve()` functions. The NAPI layer is transport-agnostic — it doesn't know about pubsub's `EventEnvelope`.

The Cargo.toml skeleton was created in setup/project-init. This task configures the actual napi-rs build pipeline, TypeScript types, and verifies the build works.

## Acceptance Criteria

- [ ] `crates/alknet-napi/` has `Cargo.toml` with `crate-type = ["cdylib"]`, `napi` and `napi-derive` dependencies
- [ ] `crates/alknet-napi/src/lib.rs` with napi module registration
- [ ] `packages/alknet-napi/` directory (or similar) with `package.json` named `@alkdev/alknet`
- [ ] `packages/alknet-napi/tsconfig.json` for TypeScript type generation
- [ ] TypeScript type definitions for `AlknetConnectOptions`, `AlknetServeOptions`, `AlknetServer`, `ConnectionInfo` matching napi-and-pubsub.md interfaces
- [ ] `napi.config.js` or `NapiRs.config` with correct cargo path, module name
- [ ] Build command: `npm run build` builds the native addon
- [ ] Feature flags: `iroh` feature optional; base package includes tcp + tls
- [ ] `npm install` and initial build succeed

## References

- docs/architecture/napi-and-pubsub.md — NAPI Wrapper section, TypeScript interfaces
- docs/architecture/decisions/015-napi-rs-for-ffi-bridge.md — napi-rs choice
- docs/architecture/decisions/016-napi-expose-connect-and-serve.md — both connect() and serve()

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion