# OQ-01: BiStream Type Definition

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: BiStream is a trait (`AsyncRead + AsyncWrite + Send + Unpin`). Handlers receive a `Connection` (not a single BiStream). This preserves the WASM door — browser clients can implement BiStream over WebTransport streams. See ADR-007.
- **Cross-references**: ADR-002, ADR-007, ADR-009
