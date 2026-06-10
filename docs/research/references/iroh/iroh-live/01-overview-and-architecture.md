# iroh-live: Overview and Architecture

## What It Is

iroh-live is a real-time audio/video streaming system built on top of [iroh](https://github.com/n0-computer/iroh) (QUIC-based P2P networking) and [Media over QUIC (MoQ)](https://moq.dev/). It handles the full pipeline: camera/mic capture → encoding → transport → decoding → rendering. Connections are peer-to-peer by default, with an optional relay server for browser access via WebTransport.

**Status:** Early tech preview. APIs are unstable. Windows support is missing. Audio-video sync is basic.

## Workspace Crates

| Crate | Description |
|-------|-------------|
| `iroh-live` | High-level API: `Live`, `Call`, `Room`, tickets, subscriptions |
| `iroh-moq` | MoQ transport layer over iroh/QUIC via `web-transport-iroh` |
| `iroh-live-relay` | Relay server bridging iroh P2P to browser WebTransport |
| `moq-media` | Media pipelines: capture, encode, decode, publish, subscribe, adaptive bitrate. No iroh dependency |
| `rusty-codecs` | Codec implementations (H264/openh264, AV1/rav1e+ rav1d, Opus), hardware accel (VAAPI, V4L2, VideoToolbox) |
| `rusty-capture` | Cross-platform capture: PipeWire, V4L2, X11, ScreenCaptureKit, AVFoundation |
| `moq-media-egui` | egui integration for video rendering |
| `moq-media-dioxus` | dioxus-native integration for video rendering |
| `moq-media-android` | Android camera, EGL rendering, JNI bridge |
| `iroh-live-cli` | CLI tool (`irl`) for publishing, playing, calls, rooms, relay |

## Layer Architecture

Three distinct layers, each usable independently:

```
┌──────────────────────────────────────────────────────────┐
│  iroh-live                                                │
│  Session management, tickets, rooms, calls                │
│  Re-exports: moq-media, iroh-moq                         │
├──────────────────────────────────────────────────────────┤
│  moq-media                                                │
│  Media pipelines: LocalBroadcast, RemoteBroadcast,       │
│  codecs, adaptive bitrate, playout                        │
│  NO iroh dependency (transport-agnostic)                 │
├──────────────────────────────────────────────────────────┤
│  iroh-moq                                                 │
│  MoQ session management, publish/subscribe over QUIC      │
│  Uses web-transport-iroh + moq-lite                       │
└──────────────────────────────────────────────────────────┘

Below moq-media:
  rusty-codecs  ─  codec implementations, hardware accel, wgpu rendering
  rusty-capture ─  platform-specific screen/camera capture
```

## Design Principles

1. **`&self` everywhere** — All public types use interior mutability. Safe to share across async tasks/threads without wrappers.
2. **Drop-based cleanup** — Dropping a `Call` closes it. Dropping `LocalBroadcast` tears down encoders. Dropping `VideoTrack` stops its decoder thread.
3. **Watcher for continuous state, Stream for discrete events** — Connection quality and catalog contents use `n0_watcher::Direct<T>`. Participant joins use `impl Stream`.
4. **Declarative intent, not mechanism** — `VideoTarget::default().max_pixels(1280*720)` describes what quality you need. The catalog selects the best rendition.
5. **moq-media is standalone** — A recording pipeline can use `LocalBroadcast`/`RemoteBroadcast` without iroh-live. The transport boundary is the `PacketSink`/`PacketSource` trait pair.

## Data Flow (End-to-End)

```
Publisher Side:
  capture source (rusty-capture, VideoSource trait)
      │
      ▼
  encoder pipeline (moq-media, dedicated OS thread)
      │
      ▼  EncodedFrame
  PacketSink (MoqPacketSink — starts new MoQ group on keyframe)
      │
      ▼  MoQ transport (iroh-moq, QUIC streams)
      
Subscriber Side:
  PacketSource (MoqPacketSource — reads ordered frames from MoQ)
      │
      ▼  MediaPacket
  decoder pipeline (moq-media, dedicated OS thread)
      │
      ▼  VideoFrame
  FramePacer (PTS-based sleep) or Sync (shared playout clock)
      │
      ▼
  renderer (wgpu texture upload or egui widget)
```

Encoder and decoder pipelines run on **dedicated OS threads**, not tokio tasks, so slow codec operations never block the async runtime. The `forward_packets` async task bridges the network-side `PacketSource` into an mpsc channel that the decoder thread reads synchronously.

## Key Dependencies

| Dependency | Purpose |
|------------|---------|
| `iroh` | QUIC endpoint, connection management, P2P connectivity |
| `iroh-gossip` | Gossip protocol for room participant discovery |
| `iroh-tickets` | Ticket serialization for `RoomTicket` |
| `iroh-smol-kv` | Distributed KV store for room state (gossip-backed) |
| `moq-lite` | Core MoQ protocol: BroadcastProducer, BroadcastConsumer, Track, Group |
| `hang` | Catalog management for broadcast metadata |
| `moq-mux` | MoQ multiplexing |
| `moq-relay` | Relay server implementation (used by iroh-live-relay) |
| `web-transport-iroh` | WebTransport over iroh QUIC connections |
| `n0-future` | Async utilities (FuturesUnordered, AbortOnDropHandle) |
| `n0-watcher` | Watchable/Direct reactive state |

## License

Dual-licensed: MIT OR Apache-2.0. Copyright 2025 N0, INC.