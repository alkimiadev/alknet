---
id: cleanup/ssh-session-recv-stub-doc
name: Document SshSession::recv() stub as planned future work
status: pending
depends_on:
  - review/phase1-core-modifications
scope: single
risk: trivial
impact: isolated
level: implementation
---

## Description

`SshSession::recv()` and `send()` in `crates/alknet-core/src/interface/ssh.rs` are stubs — `recv()` unconditionally returns `None` and `send()` returns `Ok(())`. Per the architecture spec (interface.md), the `alknet-control:0` channel should eventually route to call protocol events through this interface.

This is acceptable for Phase 1 (tunnel functionality still works via the existing `ServerHandler`), but it should be documented as planned future work so it doesn't get forgotten.

**Fix**:
- Add doc comment on `SshSession::recv()` and `send()` explicitly marking them as stubs
- Note that call protocol event bridging from SSH channels is planned for Phase 2/3
- Add `// TODO:` or similar marker

## Acceptance Criteria

- [ ] `SshSession::recv()` has doc comment stating it's a stub for Phase 1
- [ ] `SshSession::send()` has doc comment stating it's a stub for Phase 1
- [ ] Comment references call protocol integration as planned future work

## References

- crates/alknet-core/src/interface/ssh.rs — recv() and send() stubs
- docs/architecture/interface.md — alknet-control:0 channel routing to call protocol

## Notes

> Identified during Phase 1 review (W3)

## Summary

> To be filled on completion