# OQ-07: Call Protocol Scope Within a Connection

- **Origin**: ADR-005
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: The call protocol uses bidirectional streams with EventEnvelope framing and ID-based correlation via PendingRequestMap. The protocol is transport-agnostic (ADR-065) and stream-agnostic — the client can open one stream per operation, multiplex on one stream, or any mix. Correlation is by request ID, not by stream. Both sides can initiate calls. One `alknet/call` connection gives access to the full operation registry (call, subscribe, batch, schema). No multiplexing layer is needed inside the connection. See ADR-012.
- **Cross-references**: ADR-005, ADR-012
