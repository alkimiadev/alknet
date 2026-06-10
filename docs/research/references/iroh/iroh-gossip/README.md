# iroh-gossip Reference Documentation

This directory contains a deep-dive reference on how the `iroh-gossip` crate works, based on source code analysis of the repository at `/workspace/iroh-gossip`.

## Documents

| # | File | Topic |
|---|------|-------|
| 01 | [Overview & Architecture](01-overview-architecture.md) | Crate structure, module organization, design principles, features, dependencies |
| 02 | [HyParView Membership](02-hyparview-membership.md) | Swarm membership protocol: active/passive views, join procedure, shuffle mechanism, failure recovery, PeerData |
| 03 | [PlumTree Broadcast](03-plumtree-broadcast.md) | Epidemic broadcast trees: eager/lazy push, Graft/IHave/Prune, tree optimization, message deduplication, cache management |
| 04 | [State & Topic Coordination](04-state-and-topic.md) | Multi-topic state management, topic lifecycle, event routing between HyParView and PlumTree |
| 05 | [Net Actor & Networking](05-net-actor.md) | Actor model, event loop, connection management, Dialer, wire protocol, address lookup, topic state in the net layer |
| 06 | [API & Data Flow](06-api-data-flow.md) | Public API types, subscription model, event/command flow, channel architecture, configuration defaults |
| 07 | [Utilities & Wire Format](07-utilities-wire-format.md) | IndexSet, TimerMap, TimeBoundCache, serialization, PeerData/AddrInfo, Dialer internals |
| 08 | [Testing & Metrics](08-testing-metrics-refs.md) | Test infrastructure, simulation, key test patterns, metrics, references |

## Quick Reference

### Version
`iroh-gossip` v0.97.0

### ALPN
`/iroh-gossip/1`

### Core Protocols
- **HyParView**: Hybrid partial view membership (active view = 5, passive view = 30 by default)
- **PlumTree**: Epidemic broadcast trees (eager + lazy push with Graft/IHave optimization)

### Key Abstractions
- **TopicId**: 32-byte identifier for a topic/swarm
- **PeerIdentity**: Generic trait (instantiated as `PublicKey` in the net layer)
- **PeerData**: Opaque bytes exchanged on join (carries `AddrInfo` in net layer)
- **IO trait**: Interface for protocol output events (pure state machine, no IO)

### Wire Format
- Postcard (serde) encoding over QUIC unidirectional streams
- Length-prefixed frames (u32 length + postcard payload)
- Stream header with TopicId
- Max message size: 4096 bytes (configurable, minimum 512)