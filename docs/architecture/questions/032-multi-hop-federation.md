# OQ-32: Multi-Hop Federation

- **Origin**: [ADR-029](decisions/029-peer-graph-routing-model.md) §3.7, `docs/research/alknet-call-peer-routing/findings.md` §3.7
- **Status**: deferred(scope)
- **Door type**: One-way (federation model), two-way (mechanism)
- **Priority**: low
- **Impacts**: None currently — the one-hop model covers all current use
  cases. Would impact peer-graph routing if a multi-hop topology becomes
  needed (e.g., chained hubs).
- **Blocked on**: A concrete use case for multi-hop federation. The one-hop model covers all current use cases (head→worker, runner→hub).
- **Resolution**: The model is **one-hop** — worker A does not transitively
  see worker B's ops through the head unless the head explicitly re-exports
  them. The peer-keyed overlay model extends to multi-hop without redesign
  (a chain of `PeerRef::Specific` routing decisions), but path-finding
  (which peer reaches which op transitively) is where a graph library
  (petgraph) would pay off. For one-hop (shallow), a nested
  `HashMap<PeerId, HashMap<String, ...>>` suffices. Multi-hop federation is
  a feature extension — the one-hop model is the architectural commitment;
  extending to multi-hop doesn't break downstream crates. Whether multi-hop
  becomes a real use case is a future decision; the peer-keyed model does
  not foreclose it.
- **Cross-references**: ADR-029, [client-and-adapters.md](crates/call/client-and-adapters.md)
