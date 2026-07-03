# alknet-docker: POC Research Summary

**Status:** Research complete — all three high-leverage unknowns validated against a live docker daemon. The approach is viable; the remaining unknowns are spec-scope, not feasibility.
**Date:** 2026-07-02
**Scope:** Captures what the POC proved about mapping bollard's docker operations onto framed bidirectional streams, the two-carriage model (JSON call protocol vs raw bytes), and what remains open for the `alknet-docker` crate spec.

---

## Executive Summary

A POC (`alknet-docker-poc`, `/workspace/alknet-docker-poc`) validated the three highest-leverage unknowns for wrapping bollard into alknet's call protocol:

1. **Interactive attach round-trip via raw carriage** — a client drives an interactive `sh` session in a container through a framed bidi stream. After a single JSON `call.requested` frame, the stream switches to a 1-byte-prefixed chunk format for stdin/stdout. Proves the stdin question is solved without modifying the core call protocol's wire format.
2. **Logs subscription → deterministic completion** — a container's log stream maps to `call.responded` frames and container exit produces a single `call.completed` frame on the client. Proves the stopgap coordination path: a coordinator spawns a container, subscribes to logs, and gets a reliable completion notification — no plugin state to corrupt.
3. **Exec with exit code propagation** — exit code rides on a final `call.responded` frame `{ "exitCode": N }` before `call.completed`. Proves streaming operations can carry a result-at-end without changing `call.completed`'s empty-payload shape.

**6 tests pass** (3 docker-integration + 3 frame/codec unit tests) against a live docker daemon (Docker Engine 29.2.1, API 1.53) using `alpine:3`.

The POC depends on the local bollard checkout (0.21.0 at `/workspace/bollard`) and uses `tokio::io::duplex` as a stand-in for a QUIC bidi stream. The framing layer is byte-identical to alknet-call's `protocol/wire.rs`, so a future swap to `alknet_call::protocol::wire::*` is mechanical.

---

## The Two-Carriage Model

The central design decision validated by the POC: **the call protocol is the negotiation layer; the carriage is per-operation.** A single `call.requested` frame carries the operation name, parameters, and a `carriage` field that tells both sides what bytes come next on the bidi stream.

### JSON carriage (`carriage: "json"`)

Used for request/response operations (lifecycle, list, inspect) and for log/progress subscriptions where each event is naturally JSON-shaped.

- After `call.requested`, all bytes on the stream are length-prefixed `EventEnvelope` frames (identical to alknet-call's `FrameFramedReader`/`FrameFramedWriter`).
- For subscriptions: each event → `call.responded`, natural stream end → `call.completed`, error → `call.error` (terminal, no `completed`).
- The dispatcher's `pump_stream` (`alknet-call/src/protocol/dispatch.rs:340`) already does exactly this — a docker logs subscription is just a `StreamingHandler` wrapping `bollard::container::logs()` in a stream of `ResponseEnvelope::ok(...)`.

### Raw carriage (`carriage: "raw"`)

Used for interactive attach/exec where JSON-encoding every byte chunk is wasteful and lossy (containers emit binary, TTYs stream partial lines, and — as noted in the conversation — "it might not be JSON").

- After `call.requested`, the stream switches to a chunk format:
  ```text
  [stream_type: u8][length: u32 be][payload bytes]
  ```
- `stream_type` mirrors bollard's `NewlineLogOutputDecoder` header byte (`/workspace/bollard/src/read.rs:46`): 0=stdin, 1=stdout, 2=stderr.
- This is the smallest viable framing that still gives multiplexing (stdout vs stderr) and length-delimiting on a stream without natural message boundaries.
- The same pattern generalizes to `alknet-ssh` and other protocols that are "just bytes on a bidi stream" — the call protocol negotiates the mode, the protocol is the bytes.

### Why not JSON for everything?

The conversation identified the core tension: the call protocol is a JSON-schema-backed JSON-RPC, which maps cleanly to websockets, HTTP request/response, MCP, etc. But it doesn't fit every situation — a container's stdout isn't JSON, a TTY streams partial bytes, and forcing everything through `serde_json` is both wasteful (base64 for binary) and lossy (line-boundary semantics).

The two-carriage model resolves this: **JSON is the default/fallback for structured operations; raw is the escape hatch for byte-stream protocols.** The `carriage` field in the initial `call.requested` is the one byte of negotiation that selects which mode the rest of the stream uses. This keeps the call protocol's wire format unchanged (the `call.requested` frame is still a normal JSON envelope) while letting the *subsequent* bytes on the same bidi stream be whatever the operation needs.

This connects to the stream-agnostic model from the alknet-ssh research: a protocol can run over QUIC (raw or iroh p2p), TLS, or TCP. The call protocol is the ALPN negotiation layer that sets up the stream; the protocol itself is bytes. The `alknet-docker` crate is the first concrete instance of this pattern, and it validates that the pattern works.

---

## POC Target 1: Interactive Attach (Raw Carriage)

**Question:** Can a client drive an interactive TTY session in a container through a framed bidi stream, with stdin flowing client→server and stdout/stderr flowing server→client, without modifying the core call protocol's wire format?

**Answer:** Yes. The reliable `attach_container()` (HTTP upgrade to TCP, not websocket) returns `AttachContainerResults { output: Stream<LogOutput>, input: AsyncWrite }`. The POC bridges both onto a single raw-chunk bidi stream:

- **server→client:** each `LogOutput` from bollard's output stream becomes a `Chunk` with the matching `stream_type` (StdOut→1, StdErr→2, StdIn→0, Console→1), written via `ChunkWriter`.
- **client→server:** `ChunkReader` reads stdin chunks, writes the bytes to bollard's `container_input` (`AsyncWrite`).
- **completion:** when bollard's output stream ends (container exited), the server sends a zero-length stdout chunk as a "drained" sentinel, then closes.

**Test:** `docker_attach_raw_round_trips_stdin_to_stdout` — creates an interactive `sh` container, sends `echo hello-from-attach\n` as a stdin chunk, reads stdout chunks until the echo appears, sends `exit\n`, cleans up. Passes.

**Why the websocket path was not used:** bollard's own docs (`/workspace/bollard/src/container.rs:577`) warn that the websocket attach endpoint "has compatibility issues with standard RFC 6455 WebSocket implementations" and that "data flow may be unreliable on some Docker versions." The reliable `attach_container()` (HTTP upgrade to TCP) uses the same `process_upgraded()` mechanism and returns the same `AttachContainerResults` shape. The POC uses the reliable path. The websocket path remains available behind bollard's `websocket` feature for browser-attach scenarios, but the inlining/forking concern raised in the conversation would only apply if we needed websocket-specific framing — we don't, because the raw chunk format is our own, layered on top of whichever bollard attach method we use.

**The `NewlineLogOutputDecoder` insight:** bollard's decoder (`read.rs:46`) already parses the docker daemon's 8-byte header (`[stream_type: u8][length: u32 be]`) into `LogOutput::StdOut/StdErr/StdIn/Console`. The POC's chunk format is the same header shape, just on our framed stream instead of docker's upgraded TCP stream. This means the mapping is a near-identity transformation — `LogOutput` → `Chunk` is a one-line match. The bytes are already framed; we just re-emit them on a different transport.

---

## POC Target 2: Logs Subscription → Completion Notification

**Question:** Does a container's log stream map cleanly to `call.responded` frames, and does container exit produce a deterministic `call.completed` on the client?

**Answer:** Yes. `bollard::container::logs()` with `follow=true` returns a `Stream<Item = Result<LogOutput, Error>>` that ends when the container exits (for non-running containers, it returns historical logs then ends immediately). The POC's `drive_logs`:

1. Reads one `call.requested` frame (the request).
2. Calls `docker.logs(container, follow=true, stdout=true, stderr=true)`.
3. For each `LogOutput` → `EventEnvelope::responded(request_id, { "stream": "stdout"|"stderr", "text": "..." })`.
4. On stream end → `EventEnvelope::completed(request_id)`.
5. On error → `EventEnvelope::error(...)` (terminal, no `completed`).

**Test:** `docker_logs_subscription_pumps_frames_and_completes` — container runs `echo line1; echo line2; exit 0`, client receives 2× `call.responded` (with timestamped text) + 1× `call.completed`. Passes.

**The stopgap coordination path this validates:** a coordinator spawns a container, subscribes to its logs, and gets `call.completed` when the container exits — no plugin state, no polling, no worktree-tracking to corrupt. This is the "reliable completion notification" the conversation identified as the thing that would have saved the session from the mid-point crisis. The completion comes from the docker daemon's own stream-termination semantics, which is as reliable as the daemon itself — far more reliable than an opencode plugin's session tracking.

**Timestamps:** the POC sets `timestamps=true` on the logs query, so each `call.responded` carries the docker timestamp in the `text` field. A production version would separate `timestamp` and `text` into distinct JSON fields.

---

## POC Target 3: Exec with Exit Code

**Question:** Can the exit code of an exec operation propagate cleanly through the streaming completion path?

**Answer:** Yes, via a final `call.responded` frame carrying `{ "exitCode": N, "terminal": true }` before `call.completed`. This keeps `call.completed`'s payload empty (`{}`), matching alknet-call's current wire format (`wire.rs:48`) — no core protocol change needed.

**Test:** `docker_exec_streams_output_and_exit_code` — exec runs `echo hello-from-exec; exit 7`, client receives stdout `call.responded` frames + a final `call.responded` with `exitCode: 7` + `call.completed`. Passes.

**The completion-shape decision this validates:** the conversation raised whether `call.completed` should carry a payload (for exit codes) or whether the exit code rides on a final `call.responded`. The POC validates the latter: **`call.completed` stays empty; the exit code is the last `call.responded` before completion.** This is less invasive — no change to alknet-call's wire format — and it composes with the dispatcher's existing `pump_stream` logic, which already writes `call.completed` on natural stream end after the last `call.responded`.

**bollard API note:** `start_exec` returns `StartExecResults::Attached { output, input }` (an enum, not a struct — the POC had to fix this against 0.21's API). The `output` is a `Stream<LogOutput>`; the exit code is *not* on the stream — it requires a separate `inspect_exec()` call after the stream ends. The POC does this: pump the output stream, then `inspect_exec` for the exit code, then send the exit-code `call.responded`, then `call.completed`. This is the correct ordering and it works.

---

## What the POC Does NOT Validate

Following the filesystem POC's pattern of distinguishing feasibility-validated from scope-deferred:

1. **Real QUIC transport.** Uses `tokio::io::duplex` as a stand-in. The framing layer is transport-agnostic (`AsyncRead`/`AsyncWrite`); the alknet-core `Connection` type wraps the same shape. Swapping to quinn is mechanical.

2. **Operation registry integration.** The POC's `DockerOps` exposes three `drive_*` methods. The real crate registers `OperationSpec`s into a shared `OperationRegistry` and lets the dispatcher's `handle_stream` call them. The `StreamingHandler` shape in alknet-call (`registry/registration.rs:20`) maps 1:1 to what `drive_logs`/`drive_exec` do — return a `Stream<ResponseEnvelope>`. The raw-carriage attach is the exception: it needs the dispatcher to hand off the raw bidi stream after the request frame, which is the one place the call protocol's `handle_stream` (`protocol/dispatch.rs:295`) would need a branch for `carriage: "raw"`.

3. **Access control / identity.** The call protocol's `AccessControl` (scopes, resources) is orthogonal. The POC has no auth. The real crate would use `AccessControl::resource_type("container")` + `resource_action("exec")` to gate operations by peer identity.

4. **Lifecycle mutations (create/start/stop/remove/list/inspect).** Mechanical bollard wrapping, no feasibility risk. The POC deliberately skips these — they're `Query`/`Mutation` operations with single `call.responded` responses, the boring case.

5. **Image management (pull, list, build).** Pull is a subscription (progress events → `call.responded`, done → `call.completed`) — same shape as logs, no new unknowns. Build (buildkit) is a large feature, deferred.

6. **Label namespace / ownership.** Dispatch used `dispatch.managed=true`. The real crate needs a configurable label prefix and ownership mapping (`alknet.owner=<peer-id>`) tied to the call protocol's identity model. Spec-scope, not feasibility.

7. **Fleet view (multiple hosts).** The POC is single-host (one `bollard::Docker` client, local socket). The fleet view — multiple dedicated servers + rented instances (e.g. runpod) — is a client-side concern: a `CallClient` talking to multiple endpoints, each running alknet-docker locally. This composes with the ALPN model cleanly. The later normalization crate is the fleet client that picks which endpoint to call — see §6 below for the boundary and the head-worker/machine-node model that frames it.

> **Note (correction):** an earlier draft of this section called the normalization crate `alknet-compute`. That name is wrong. `alknet-compute` is an example of something a *normalized* `alknet-container` might **run inside** a container — a workload, not the fleet layer. The normalization crate is `alknet-container` (or similar), and its job is making any docker-capable machine addressable through one shape.

---

## Open Unknowns (For the Spec)

### 1. Raw-carriage handoff in the dispatcher (design)

The POC's `drive_attach_raw` reads the `call.requested` frame itself, then switches to raw chunks. In the real crate, the dispatcher's `handle_stream` (`alknet-call/src/protocol/dispatch.rs:295`) currently reads the request frame and calls `dispatch()` which returns a `DispatchResult::Stream(ResponseStream)`. For raw carriage, the handler needs the *raw bidi stream* (the `send`/`recv` pair), not just a `ResponseStream` to pump.

Two options:
- **(a)** Branch in `handle_stream` on the `carriage` field in the request payload: if `raw`, hand the raw streams to a `RawHandler` trait instead of pumping a `ResponseStream`. Localizes the change to `handle_stream`; the wire format and dispatcher stay unchanged.
- **(b)** A separate ALPN for raw-carriage operations (e.g. `alknet/docker-raw`). Avoids touching the call dispatcher entirely; the `ProtocolHandler` for that ALPN owns the whole stream. Less elegant but zero blast radius.

The POC validates the *mechanism* (raw chunks on a bidi stream after a JSON request); the *integration point* is a spec decision. Option (a) is cleaner and keeps all docker ops on `alknet/call`; option (b) is the safest for a first cut.

### 2. ALPN layout (design)

Should docker ops register on the shared `alknet/call` ALPN (as operations in a shared `OperationRegistry`) or get their own `alknet/docker` ALPN (as a `ProtocolHandler`)? The conversation leans shared. The POC doesn't resolve this — it's a spec decision tied to how the assembly layer (the CLI binary) composes handlers. Shared registry is more composable (docker ops are callable from any call client, including peer routing); separate ALPN is more isolated.

### 3. Container-as-resource identity model (design)

How do containers map to the call protocol's `AccessControl::resource_type`/`resource_action`? A container ID is a natural resource. `docker/container/exec` could require `resource: container/<id>:exec`. But containers are created at runtime — the resource set is dynamic. The `IdentityProvider` model in alknet-core is currently static (`PeerEntry` set). Dynamic resource ownership (who created this container, who can exec into it) needs a spec.

### 4. Stdin closure semantics for raw carriage (design)

The POC uses a zero-length stdin chunk as "client done sending input." bollard's `container_input.shutdown()` then closes the container's stdin so the process sees EOF. This works for the interactive case. But for a non-interactive exec with stdin (piping bytes in), the closure semantics need to be clearer: does the client send a zero-length chunk, or just close the write half of the duplex? The POC handles both (zero-length chunk breaks the loop; `ConnectionClosed` also breaks the loop), but the spec should pick one as the canonical "stdin done" signal.

### 5. bollard version pinning (scoping)

The POC uses the local checkout at 0.21.0. The real crate should depend on published 0.21 from crates.io (the dispatch POC pinned 0.18 — a 3-version jump). The `websocket` feature is optional; the `http` and `pipe` features are needed for socket/http connect. Confirm the published 0.21 has the same API surface as the checkout (it should — same version number).

### 6. The normalization crate boundary (scoping)

Where does `alknet-docker` end and the later normalization crate begin?

**alknet-docker** stays a thin, single-host, bollard-specific wrapper. It talks to one local docker daemon and exposes operations over the call protocol. The POC validates this side.

**The normalization layer** — tentatively `alknet-container` — is the fleet client that talks to multiple alknet-docker endpoints over the call protocol (not bollard). It makes "any docker-capable machine" addressable through one shape, regardless of whether that machine is a dedicated OVH server, a runpod non-GPU instance ($0.07/hr), a vast.ai GPU box, or a local dev box.

**What `alknet-compute` actually is:** a workload — an example of something a normalized `alknet-container` would *run inside* a container it manages, not the fleet layer itself. An earlier conflation of these two is the thing being corrected here.

**The head-worker / machine-node model.** Framed ray.io-style to untangle the fleet topology:

- **Machine node** — any node capable of running docker. Neutral about role.
- **Head node (hub)** — a node that other nodes connect *to* and that manages them. E.g. a dedicated server hosting its existing containers *plus* a hub endpoint running in a container on that same node.
- **Worker node (spoke)** — a node that connects *to* a head and exposes its local operations so the head can manage its containers. E.g. a second dedicated server would connect to the hub and expose its docker operations for remote management.

A machine can be both spoke and hub. Two dedicated servers (e.g. rented from OVH) are both machine nodes; one additionally hosts the hub. When scaling dev agents or needing GPUs, rented runpod/vast.ai instances become worker spokes that dial the same hub.

**Prior art — the dispatch POC.** `/workspace/@alkdev/dispatch` is an older, out-of-date-deps POC that demonstrates the *reverse* of a typical GitHub/Gitea runner: instead of the runner dialing a control plane, the control plane dials into worker nodes over SSH. Its `InstanceProvider` trait (`src/provider.rs`) and `DockerProvider` (`src/docker.rs`, bollard 0.18, `dispatch.managed=true` labels, SSH-key-injection into containers) is the same "normalize heterogeneous compute" idea, but implemented by requiring SSH on the worker end. The SSH requirement is realistic for runpod/vast.ai but is exactly the friction alknet-container removes: the worker dials the hub over the call protocol and exposes its docker operations directly — no SSH, no key injection, no port binding to 127.0.0.1.

**How external providers normalize.** runpod exposes a standard OpenAPI spec; `alknet-http`'s `from_openapi` adapter (`crates/alknet-http/src/adapters/from_openapi.rs`) can import it wholesale and surface its operations as call-protocol operations. vast.ai has a similar API but needs customization (no clean OpenAPI drop-in). The normalization crate wraps both behind one `InstanceProvider`-shaped trait so the fleet client is provider-agnostic.

This keeps alknet-docker single-host and bollard-specific; the normalization layer is transport- and provider-agnostic (it talks the call protocol and `from_openapi`-imported HTTP APIs, not bollard or raw SSH).

---

## Test Coverage

```
running 6 tests
test frame_completed_carries_empty_payload ... ok
test raw_chunk_round_trip_stdin_and_stdout ... ok
test frame_round_trip_request_and_response ... ok
test docker_attach_raw_round_trips_stdin_to_stdout ... ok
test docker_logs_subscription_pumps_frames_and_completes ... ok
test docker_exec_streams_output_and_exit_code ... ok

test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 9.65s
```

The three docker-integration tests run against a live daemon (Docker Engine 29.2.1, API 1.53) using `alpine:3`. They pull the image if missing, create short-lived labeled containers, and clean up after. The three unit tests validate the frame/codec round-trip without docker.

---

## POC Structure

```
alknet-docker-poc/
  Cargo.toml          — depends on bollard (path = "../bollard"), tokio, serde_json
  src/
    lib.rs            — module docs, the two-carriage model rationale
    frame.rs          — EventEnvelope, FrameFramedReader/Writer (mirrors alknet-call wire.rs)
    raw.rs            — Chunk, ChunkReader/Writer (1-byte stream-type + 4-byte length)
    ops.rs            — DockerOps: drive_logs, drive_exec, drive_attach_raw
  tests/
    integration.rs    — 6 tests (3 docker-integration + 3 codec unit)
```

---

## Key Code-to-Concept Mappings

| POC concept | alknet-call equivalent | bollard equivalent |
|---|---|---|
| `EventEnvelope` (`frame.rs`) | `alknet_call::protocol::wire::EventEnvelope` | — |
| `FrameFramedReader/Writer` | `alknet_call::protocol::wire::FrameFramedReader/Writer` | — |
| `call.requested`/`responded`/`completed` | same event types | — |
| `Chunk` stream_type 0/1/2 | — | `NewlineLogOutputDecoder` header byte (`read.rs:46`) |
| `drive_logs` pump | `StreamingHandler` returning `Stream<ResponseEnvelope>` | `Docker::logs()` → `Stream<LogOutput>` |
| `drive_exec` exit code | final `call.responded` before `call.completed` | `Docker::inspect_exec()` → `ExecInspectResponse.exit_code` |
| `drive_attach_raw` raw handoff | `handle_stream` branch on `carriage: "raw"` (spec decision) | `Docker::attach_container()` → `AttachContainerResults { output, input }` |
| `Carriage::Json`/`Raw` | (new field in `call.requested` payload) | — |

---

## References

- bollard source (0.21.0): `/workspace/bollard` — `src/container.rs` (`attach_container` at :540, `attach_container_websocket` at :613, `LogOutput` at :96, `AttachContainerResults` at :80), `src/exec.rs` (`CreateExecOptions` at :28, `StartExecResults` enum at :99, `start_exec` at :225), `src/read.rs` (`NewlineLogOutputDecoder` at :32)
- bollard examples: `/workspace/bollard/examples/attach_container.rs` (reliable attach + tty), `/workspace/bollard/examples/websocket_attach.rs` (websocket attach with reliability warning)
- alknet-call wire format: `/workspace/@alkdev/alknet/crates/alknet-call/src/protocol/wire.rs` (EventEnvelope, FrameFramedReader/Writer — the POC's `frame.rs` mirrors this)
- alknet-call dispatch: `/workspace/@alkdev/alknet/crates/alknet-call/src/protocol/dispatch.rs` (`handle_stream` at :295, `pump_stream` at :340 — the streaming pump the POC's `drive_logs`/`drive_exec` mirror)
- alknet-call registry: `/workspace/@alkdev/alknet/crates/alknet-call/src/registry/registration.rs` (`StreamingHandler` at :20 — the handler shape for subscription ops)
- dispatch POC (prior art, "reverse runner"): `/workspace/@alkdev/dispatch` — `src/provider.rs` (`InstanceProvider` trait), `src/docker.rs` (bollard 0.18 wrapping, SSH-key-injection model), `src/vast.rs`, `AGENTS.md` (provider architecture summary)
- alknet-http `from_openapi` adapter (runpod-style provider import): `/workspace/@alkdev/alknet/crates/alknet-http/src/adapters/from_openapi.rs`
- filesystem POC summary (structure reference): `/workspace/@alkdev/alknet/docs/research/alknet-filesystem/poc-summary.md`
- SDD process: `/workspace/@alkdev/alknet/docs/sdd_process.md` (Phase 0 exploration → Phase 1 architecture)
- System docs (private, not in-repo): the maintainer's two-server fleet setup that motivates this design. The fleet use case is captured abstractly in §6 above; the concrete hostnames/IPs/paths are kept out of the public repo.