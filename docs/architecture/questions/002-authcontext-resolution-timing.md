# OQ-02: AuthContext Resolution Timing

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: Hybrid model (Option C) — endpoint resolves what it can (e.g., TLS client certificate), handler resolves what it must (e.g., AuthToken in first frame). AuthContext may be partial when `handle()` is called. See ADR-004.
- **Cross-references**: ADR-002, ADR-004
