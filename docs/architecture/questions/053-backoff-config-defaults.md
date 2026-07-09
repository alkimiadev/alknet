# OQ-53: BackoffConfig Default Policy

- **Origin**: [crates/hub/README.md](crates/hub/README.md)
- **Status**: open
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: Not yet decided.

The `BackoffConfig` struct provides configurable backoff for worker
reconnection. The default policy (1s initial, 60s max, 2x multiplier) is a
starting point. Whether this is the right policy for production deployments
is an operational question, not an architectural one.

The `BackoffConfig` struct shape is committed; the defaults are a starting
point that can be changed without breaking the API. The question is whether
to change the defaults before the first release, based on operational
experience from the alkapi deployment.

- **Cross-references**: [crates/hub/README.md](crates/hub/README.md)
