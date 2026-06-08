---
id: cleanup/panic-free-static-config
name: Replace panic/expect/unwrap with Result-based error handling in StaticConfig
status: pending
depends_on:
  - review/phase1-core-modifications
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

The `parse_proxy_config` function and related code in `crates/alknet-core/src/config/static_config.rs` uses `expect()`, `panic!()`, and bare `unwrap()` calls. This is bad form for production code — panics in library code should be avoided unless truly unreachable.

Since `StaticConfig::from_serve_options()` already returns `Result<..., ConfigError>`, the proxy config parsing should propagate errors through the `Result` chain instead of panicking. A misconfigured proxy string should produce a clear `ConfigError`, not crash the process.

**Fix**:
- Replace `expect()` and `panic!()` in `parse_proxy_config` with proper `Result::Err` returns
- Replace bare `unwrap()` calls with `?` or explicit error mapping
- Ensure all error paths produce meaningful `ConfigError` variants

## Acceptance Criteria

- [ ] No `panic!()`, `expect()`, or bare `unwrap()` in `static_config.rs` production code paths
- [ ] All error paths return `Result<..., ConfigError>` with descriptive messages
- [ ] Invalid proxy config strings produce clear errors instead of panicking
- [ ] All existing tests pass
- [ ] New test: malformed proxy string returns `Err(ConfigError)`, doesn't panic

## References

- crates/alknet-core/src/config/static_config.rs — lines with panic/expect/unwrap
- crates/alknet-core/src/error.rs — ConfigError type

## Notes

> Identified during Phase 1 review (W5)

## Summary

> To be filled on completion