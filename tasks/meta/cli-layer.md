---
id: meta/cli-layer
name: Complete CLI layer — alknet serve and alknet connect commands
status: completed
depends_on:
  - cli/serve-command
  - cli/connect-command
scope: moderate
risk: low
impact: phase
level: planning
---

## Description

Meta task that clusters CLI tasks. Once complete, the `alknet` binary has both `serve` and `connect` subcommands with all flags matching the architecture specs.

## Acceptance Criteria

- [x] Both CLI tasks completed
- [x] `alknet serve --help` and `alknet connect --help` match architecture spec flag lists
- [x] End-to-end: `alknet serve` + `alknet connect` establishes working SSH tunnel

## References

- docs/architecture/client.md, docs/architecture/server.md

## Summary

CLI layer complete. Both `alknet serve` and `alknet connect` subcommands implemented with all architecture spec flags.