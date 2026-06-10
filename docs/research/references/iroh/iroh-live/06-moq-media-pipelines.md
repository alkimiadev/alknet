# moq-media: Media Pipelines

## Overview

`moq-media` owns the media pipeline: broadcast management, codec orchestration, playout timing, adaptive bitrate, and audio backend. **It has no dependency on iroh** — it works with any transport that implements `PacketSource` and `PacketSink`. This makes it usable for recording pipelines, studio links, and camera dashboards without RTC.

## Module Structure

```
moq-media/
├── lib.rs              — Re-exports and feature-gated modules
├── publish.rs          — LocalBroadcast, VideoPublisher, AudioPublisher
├── subscribe.rs         — RemoteBroadcast, VideoTrack, AudioTrack, MediaTracks
├── transport.rs         — PacketSource/PacketSink traits, MoqPacketSource, MoqPacketSink
├── net.rs               — NetworkSignals (RTT, loss rate, available bandwidth)
├── adaptive.rs          — Adaptive rendition switching algorithm
├── playout.rs           — PlaybackPolicy, SyncMode
├── chat.rs              — ChatPublisher, ChatSubscriber (MoQ track-based)
├── frame_channel.rs     — Single-frame channel (last-writer-wins for video)
├── sync.rs              — Shared playout clock (Sync) for A/V sync
├── stats.rs             — Metric, Label, NetStats, EncodeStats, RenderStats, etc.
├── pipeline.rs          — Pipeline orchestration
├── pipeline/            — VideoEncoderPipeline, AudioEncoderPipeline, VideoDecoderPipeline, etc.
├── audio_backend.rs     — AudioBackend trait and device enumeration
├── audio_backend/       — Platform-specific audio backends (cpal, etc.)
├── capture.rs           — Camera/screen capture integration
├── source_spec.rs       — VideoInput, PreEncodedTrack
├── test_util.rs         — Test utilities (feature-gated)
└── processing/          — Scale, color conversion, etc.
```

## Publish Pipeline — `LocalBroadcast`

`LocalBroadcast` manages encoder pipelines and publishes a catalog that subscribers use to discover available renditions. It owns a `BroadcastProducer` (from moq-lite) and coordinates video and audio track lifecycles.

### Construction

```rust
let broadcast = LocalBroadcast::new();
broadcast.video().set_source(camera, VideoCodec::H264, [VideoPreset::P720])?;
broadcast.audio().set(mic, AudioCodec::Opus, [AudioPreset::Hq])?;

// Or pre-encoded sources
broadcast.video().set(VideoInput::pre_encoded("video/h264-pi", config, factory))?;
```

### Slot Handles

- `broadcast.video()` → `VideoPublisher` (borrows `&self`)
- `broadcast.audio()` → `AudioPublisher` (borrows `&self`)

Both use interior mutability. Calling `set()` tears down any existing pipeline and installs the new one.

### Video Input Modes

```rust
pub enum VideoInput {
    Renditions(VideoRenditions),  // Raw source → multiple encoded renditions (simulcast)
    PreEncoded(Vec<PreEncodedTrack>), // Already-encoded tracks pass through
}
```

**`VideoRenditions`** holds a `SharedVideoSource` and a map of rendition names to encoder factories. Multiple renditions share the same source via `watch::Receiver<Option<VideoFrame>>`. Slow encoders never cause backpressure on the source — intermediate frames are silently skipped.

**`PreEncodedTrack`** is for hardware encoders that produce compressed output directly (e.g., rpicam-vid on Raspberry Pi). Each track carries a name, `VideoConfig`, and a factory closure that creates a fresh source per subscriber.

### SharedVideoSource

Runs the capture source on a dedicated OS thread. Parks when no subscribers are connected (releasing camera/screen resources) and unparks when the first subscriber arrives. Uses `AtomicU32` subscriber counting with proper memory ordering (`AcqRel`/`Acquire`).

Frames are distributed via `watch::Sender<Option<VideoFrame>>` — always contains the latest frame, so slow encoders never block the source.

### Demand-Driven Track Startup

The broadcast's run loop (`LocalBroadcast::run_dynamic`) calls `producer.requested_track().await` to wait for subscriber demand. When a subscriber requests a specific rendition:

1. The loop looks up the rendition in the current `VideoInput` or `AudioRenditions`
2. It starts the corresponding encoder pipeline on a dedicated OS thread
3. When all subscribers disconnect (tracked via `track.unused().await`), the pipeline is stopped

This means encoder threads only run when someone is actually consuming.

### Catalog

`LocalBroadcast` maintains a catalog track (hang's built-in catalog mechanism) listing all available video and audio renditions with codec configuration, dimensions, and bitrate. Updated whenever video or audio is set/cleared.

Catalog format follows the `hang::catalog::Catalog` structure with `Video` and `Audio` entries, each containing a `BTreeMap<String, Config>` of rendition names to configurations.

### Encoder Pipeline Architecture

All encoder pipelines run on **dedicated OS threads** (`spawn_thread`), not tokio tasks. Codec operations are CPU-intensive and sometimes block on hardware (VAAPI, V4L2), so running on tokio tasks would starve other async work.

Communication with the async runtime:
- **VideoEncoderPipeline**: reads `SharedVideoSource` via `watch::Receiver`, writes encoded frames to `MoqPacketSink`
- **AudioEncoderPipeline**: reads from `AudioSource`, writes to `MoqPacketSink`
- **PreEncodedVideoPipeline**: reads from `PreEncodedVideoSource`, writes to `MoqPacketSink`

### Chat

```rust
let chat_publisher = broadcast.enable_chat()?;
chat_publisher.send("Hello!")?;

// Subscriber side
if let Some(chat_sub) = remote_broadcast.chat() {
    let msg = chat_sub.recv().await;
}
```

Each chat message is a single MoQ group with one frame of UTF-8 text. The track name is `"chat"` with priority 10.

## Subscribe Pipeline — `RemoteBroadcast`

`RemoteBroadcast` wraps a `BroadcastConsumer` and watches its catalog for available video and audio renditions. Created with a `BroadcastConsumer` and a `PlaybackPolicy`.

### Construction

```rust
let broadcast = RemoteBroadcast::new("stream-name", consumer).await?;
// Or with explicit policy
let broadcast = RemoteBroadcast::with_playback_policy("stream", consumer, policy).await?;
```

On construction, spawns a catalog-watching task that publishes snapshots via `Watchable<CatalogSnapshot>`.

### `CatalogSnapshot`

Point-in-time view of the broadcast's catalog. Derefs to `hang::Catalog`. Carries a sequence number for change detection.

```rust
let catalog = broadcast.catalog();
catalog.video_renditions()  // Iterator of rendition names sorted by width
catalog.audio_renditions()  // Iterator of audio rendition names
catalog.select_video_rendition(Quality::High)?  // Best match for quality
catalog.has_video()
catalog.has_audio()
catalog.has_chat()
catalog.user()  // User metadata from publisher
```

### Rendition Selection

```rust
pub enum Quality { Highest, High, Mid, Low }

pub struct VideoTarget {
    pub max_pixels: Option<u32>,
    pub max_bitrate_kbps: Option<u32>,
    pub rendition: Option<String>,  // Pin to specific rendition
}
```

`Quality::High` → `max_pixels(1280*720)`, etc. If `rendition` is set, it takes priority.

### VideoTrack

Represents a decoded video stream from a remote broadcast. The decoder runs on a dedicated OS thread.

**Creation flow:**

1. Pick a rendition (via `VideoTarget` or explicit name)
2. Create `TrackConsumer` from `BroadcastConsumer`, wrap in `OrderedConsumer` with `PlaybackPolicy::max_latency`
3. Wrap in `MoqPacketSource`
4. A `forward_packets` async task reads from `MoqPacketSource` → `mpsc` channel
5. Decoder thread reads `mpsc` → decoder → output via `Sync` playout clock (or `FramePacer`)
6. Output channel: `FrameReceiver<VideoFrame>` (latest-frame wins, suitable for rendering)

**Frame access:**
- `track.try_recv()` — Returns latest frame, draining older buffered frames (for game loops)
- `track.next_frame().await` — Async wait for next frame
- `track.has_frame()` — Check without consuming

**Adaptive rendition switching:**
```rust
track.enable_adaptation(broadcast, signals, config, decode_config)?;
track.disable_adaptation();
track.is_adaptive();
track.selected_rendition();
track.set_rendition_mode(RenditionMode::Fixed("video/h264-360p".into()));
track.set_rendition_mode(RenditionMode::Auto);
track.rendition_watcher();  // Direct<String> watcher for rendition changes
```

### AudioTrack

Same pattern as `VideoTrack` but sends decoded samples to an `AudioSink` (typically cpal + sonora). The audio decoder thread runs a 10ms tick loop.

### MediaTracks

Convenience struct combining `RemoteBroadcast` with optional `VideoTrack` and `AudioTrack`:

```rust
pub struct MediaTracks {
    pub broadcast: RemoteBroadcast,
    pub video: Option<VideoTrack>,
    pub audio: Option<AudioTrack>,
}
```

### Lifecycle

Both `VideoTrack` and `AudioTrack` use drop-based cleanup. Dropping cancels the decoder thread (via `CancellationToken`) and the `forward_packets` task (via `AbortOnDropHandle`). The `OrderedConsumer` is dropped, signaling the transport that the track is no longer needed.

## Transport Abstraction — `PacketSource` / `PacketSink`

The transport boundary between moq-media and the network:

```rust
pub trait PacketSource: Send + 'static {
    fn read(&mut self) -> impl Future<Output = Result<Option<MediaPacket>>> + Send;
}

pub trait PacketSink: Send + 'static {
    fn write(&mut self, packet: EncodedFrame) -> Result<()>;
    fn finish(&mut self) -> Result<()>;
}
```

**`MoqPacketSink`** wraps an `OrderedProducer`. When it receives an `EncodedFrame` with `is_keyframe = true`, it calls `keyframe()` on the producer to start a new MoQ group. This keyframe-to-group mapping is how subscribers can join at any group boundary.

**`MoqPacketSource`** wraps an `OrderedConsumer` and reads frames, converting them to `MediaPacket`.

**`PipeSink` / `PipeSource`** — In-memory pipe for local encode→decode without network (testing, local preview).

## Adaptive Rendition Switching

The adaptation algorithm runs in a background task that monitors `NetworkSignals` and decides whether to switch to a different video rendition.

### Algorithm

Renditions are ranked by pixel count (highest first). The algorithm maintains state across ticks:

```rust
pub enum Decision {
    Hold,                      // Stay on current rendition
    Downgrade(usize),          // Switch to lower at index
    Emergency,                 // Drop to lowest immediately
    StartProbe(usize),         // Try upgrading to index
}
```

**Emergency** (immediate): Loss rate ≥ 20% → drop to lowest rendition

**Downgrade** (sustained 500ms): Loss rate ≥ 10% OR available bandwidth < 85% of current rendition's bitrate

**Upgrade probe** (sustained 4s good conditions): Loss ≤ 2%, bandwidth ≥ 120% of next-higher rendition's bitrate → start 3-second probe on the higher rendition

**Probe abort**: Loss ≥ 5% or new congestion events during probe → abort, 8s cooldown

**Post-downgrade cooldown**: 4s after any downgrade before probes are allowed

### Implementation

The adaptation task (`adaptation_task_v2`) creates new `VideoDecoderPipeline`s that write to the same `FrameSender` via `with_sender()`. The frame channel stays the same while the underlying decoder pipeline gets swapped. When switching:

1. Create a new decoder pipeline for the target rendition
2. Drop the old pipeline handle
3. Update `selected_rendition` Watchable

## Playback and Sync

### PlaybackPolicy

```rust
pub struct PlaybackPolicy {
    pub sync: SyncMode,           // Synced (shared clock) or Unmanaged (PTS pacing)
    pub max_latency: Duration,    // Default: 150ms — how much buffering before skipping forward
}
```

### SyncMode

- **`Synced`** (default): Shared playout clock (`Sync`). Video frames are gated by `Sync::wait(pts)`, which blocks until `reference + pts + latency` arrives. Audio paces itself through its ring buffer (~80ms).
- **`Unmanaged`**: No synchronization. `FramePacer` sleeps between frames based on PTS deltas, clamped to 2× frame period.

### Sync

The `Sync` type records arrival offsets via `received(pts)` and blocks on `wait(pts)` until `reference + pts + latency`. This keeps audio and video aligned without cross-path gating or signaling. Ported from the moq/js implementation.

## Stats

moq-media has a structured stats system for debug overlays:

- **`NetStats`** — RTT, loss%, bandwidth, path type (written by iroh-live transport bridge)
- **`EncodeStats`** — FPS, encode time, bitrate, codec, encoder, resolution, capture path
- **`RenderStats`** — FPS, decode time, decoder, renderer, rendition
- **`TimingStats`** — Audio buffer level, video/audio lag, A/V delta, video buffer depth
- **`Timeline`** — Ring buffer of `FrameMeta` entries for timeline visualization

Each `Metric` has EMA smoothing, a history ring buffer, and optional color thresholds. `Label` provides atomic string values.

## Codec Support

Feature-gated codec support:

| Feature | Codec | Backend |
|---------|-------|---------|
| `h264` | H.264 | openh264 (software) |
| `av1` | AV1 | rav1e encoder, rav1d decoder |
| `opus` | Opus | opus crate |
| `vaapi` | VAAPI | Linux hardware encode/decode |
| `videotoolbox` | VideoToolbox | macOS hardware |
| `v4l2` | V4L2 | Raspberry Pi hardware |
| `pcm` | Raw PCM | No encoding |