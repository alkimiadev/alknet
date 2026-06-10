# iroh-live Reference Documentation

> **Status:** Early tech preview. APIs are unstable. Based on source code analysis of the iroh-live workspace.

## Files

| File | Topic |
|------|-------|
| [01-overview-and-architecture](01-overview-and-architecture.md) | Workspace structure, crate layers, design principles, data flow, dependencies |
| [02-core-api](02-core-api.md) | `Live`, `LiveTicket`, `Call`, `Subscription`, `DisconnectReason`, `util` module |
| [03-iroh-moq-transport](03-iroh-moq-transport.md) | `Moq`, `MoqSession`, `MoqProtocolHandler`, actor internals, session lifecycle, error types |
| [04-rooms](04-rooms.md) | `Room`, `RoomHandle`, `RoomTicket`, `RoomEvent`, gossip KV coordination, actor architecture |
| [05-relay](05-relay.md) | `iroh-live-relay`: browser bridging, pull model, `RelayConfig`, `PullState`, web viewer |
| [06-moq-media-pipelines](06-moq-media-pipelines.md) | `LocalBroadcast`, `RemoteBroadcast`, `VideoTrack`, `AudioTrack`, transport abstraction, codec support |
| [07-network-signals-and-adaptive-bitrate](07-network-signals-and-adaptive-bitrate.md) | `NetworkSignals`, adaptation algorithm, `AdaptiveConfig`, `Decision`, probe lifecycle |
| [08-p2p-and-relay](08-p2p-and-relay.md) | iroh P2P connectivity, relay architecture, pull model, ticket format, connection access |

## Quick Navigation

### "How do I..."

- **Publish a stream?** → [02-core-api](02-core-api.md) (`Live::publish`) + [06-moq-media-pipelines](06-moq-media-pipelines.md) (`LocalBroadcast`)
- **Subscribe to a stream?** → [02-core-api](02-core-api.md) (`Live::subscribe`) + [06-moq-media-pipelines](06-moq-media-pipelines.md) (`RemoteBroadcast`)
- **Make a 1:1 call?** → [02-core-api](02-core-api.md) (`Call::dial` / `Call::accept`)
- **Create a multi-party room?** → [04-rooms](04-rooms.md) (`Room::new`, `RoomTicket`)
- **Bridge to browsers?** → [05-relay](05-relay.md) (`iroh-live-relay`)
- **Adapt quality to network conditions?** → [07-network-signals-and-adaptive-bitrate](07-network-signals-and-adaptive-bitrate.md)
- **Understand the MoQ transport?** → [03-iroh-moq-transport](03-iroh-moq-transport.md)
- **Understand the media pipeline?** → [06-moq-media-pipelines](06-moq-media-pipelines.md)

### Key Source Files

| Component | Path |
|-----------|------|
| iroh-live crate | `iroh-live/src/{lib, live, call, subscription, ticket, types, util, rooms}.rs` |
| iroh-moq crate | `iroh-moq/src/lib.rs` |
| iroh-live-relay | `iroh-live-relay/src/{lib, main, pull}.rs` |
| moq-media publish | `moq-media/src/publish.rs` |
| moq-media subscribe | `moq-media/src/subscribe.rs` |
| moq-media adaptive | `moq-media/src/adaptive.rs` |
| moq-media transport | `moq-media/src/transport.rs` |
| moq-media network signals | `moq-media/src/net.rs` |