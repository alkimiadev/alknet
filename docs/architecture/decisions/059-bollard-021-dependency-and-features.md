# ADR-059: bollard 0.21 Dependency and Feature Selection

## Status

Accepted

## Context

The alknet-docker POC (`docs/research/alknet-docker/poc-summary.md`)
depended on a local bollard checkout at `/workspace/bollard` (version
0.21.0). Its "Open Unknowns" #5 noted: "The real crate should depend on
published 0.21 from crates.io (the dispatch POC pinned 0.18 — a
3-version jump). The `websocket` feature is optional; the `http` and
`pipe` features are needed for socket/http connect. Confirm the
published 0.21 has the same API surface as the checkout."

Two concerns motivate an explicit version check:

1. **Agent training-data drift.** Implementation agents tend to default
   to the bollard version represented in their training data unless
   explicitly told to check. The dispatch POC used bollard 0.18 (3
   versions behind); an agent that pattern-matched dispatch would pin
   0.18 and silently use a 3-version-stale API. The cost of checking is
   low (one crates.io lookup); the cost of using a stale version is
   potentially high (API mismatches, missing methods, security regressions).

2. **API surface stability across the path-vs-published boundary.** The
   POC used a path dependency; the crate uses a published version. A
   version number match (0.21.0) should mean the API surface is
   identical, but this is worth confirming rather than assuming.

### Version verification

A check against crates.io / docs.rs confirmed:

- **bollard 0.21.0 is the latest published version** on crates.io (as
  of 2026-07-08). The local checkout at `/workspace/bollard` (0.21.0)
  matches the published version. No version jump is needed; the POC's
  dependency is current.
- The published 0.21.0 `Cargo.toml` (verified via docs.rs source) has
  the same API surface the POC used: `attach_container` (container.rs:540),
  `logs` (container.rs:928), `create_exec`/`start_exec`/`inspect_exec`
  (exec.rs:172/225/315), `AttachContainerResults` (container.rs:80),
  `StartExecResults` enum (exec.rs:99), `LogOutput` (container.rs:96),
  `NewlineLogOutputDecoder` (read.rs:32). The POC's code-to-concept
  mappings hold against the published version.
- bollard-stubs is pinned at `=1.53.1-rc.29.3.1` (the Docker API 1.53
  schema), matching the Docker Engine 29.2.1 / API 1.53 the POC tested
  against.

### Feature selection

bollard 0.21's feature surface (from the published `Cargo.toml`):

| Feature | What it pulls | Needed for alknet-docker? |
|---------|---------------|--------------------------|
| `default = ["http", "pipe"]` | hyper-util (http), hyperlocal + hyper-named-pipe (pipe) | **Yes** — the default. `pipe` connects to the unix socket (`/var/run/docker.sock`); `http` connects to a TCP/HTTP daemon. Both are the standard local-daemon paths. |
| `ssl` | rustls TLS for remote daemon connections | No — alknet-docker talks to a local daemon; remote daemons are reached over the call protocol (the fleet layer, not bollard's TLS). |
| `ssh` | openssh for SSH-tunneled daemons | No — the dispatch POC's SSH-tunnel model is exactly what alknet removes; the worker dials the hub over the call protocol instead of the hub SSHing into the worker. |
| `websocket` | tokio-tungstenite for the websocket attach endpoint | No — the POC deliberately used the reliable `attach_container()` (HTTP upgrade to TCP), not the websocket path (bollard's own docs warn of RFC 6455 compatibility issues). The websocket feature is for browser-attach scenarios alknet-docker doesn't have. |
| `buildkit` / `buildkit_providerless` | tonic + bollard-buildkit-proto for buildkit image builds | No — image build is deferred (POC "What the POC Does NOT Validate" #5; OQ-049 scopes this). |
| `chrono` / `time` | timestamp parsing for log timestamps | **Optional** — `docker/container/logs` sets `timestamps: true`; the `chrono` or `time` feature types the timestamp field. The choice between them is a two-way-door implementation detail (both are serde-compatible). `time` is the lighter dependency. |
| `json_data_content` | serde for body content types | No — alknet-docker serializes its own JSON shapes from bollard's types; it doesn't need bollard's content-type tagging. |
| `aws-lc-rs` / `webpki` / `test_*` | TLS provider variants and test-only features | No — not for production deps. |

The POC's `Cargo.toml` depended on bollard with default features (path
dependency). The crate does the same: `bollard = { version = "0.21",
default-features = true }`, with the `time` feature added for log
timestamp typing. No `ssl`, `ssh`, `websocket`, or `buildkit` features.

## Decision

### 1. Depend on published bollard 0.21 from crates.io

```toml
[dependencies]
bollard = { version = "0.21", features = ["time"] }
```

`default-features = true` (the default) pulls `http` + `pipe`, the two
local-daemon connect paths. The `time` feature adds timestamp typing
for `docker/container/logs`. No other features are enabled.

The local-checkout path dependency the POC used is retired. The
published 0.21.0 has the identical API surface (confirmed via docs.rs
source — same version number, same method signatures, same stubs pin).

### 2. Connect via `connect_with_local_defaults()` (unix socket)

The standard deployment talks to a local docker daemon over
`/var/run/docker.sock` (Unix) or the named pipe on Windows.
`Docker::connect_with_local_defaults()` handles both. The `Docker`
client is constructed once at assembly-layer startup and injected into
the docker ops registration. See
[overview.md](../crates/docker/overview.md) §"Assembly Layer Wiring".

A TCP daemon path (`connect_with_http_defaults`) is available if a
deployment runs the docker daemon on a different host — but this is the
exception, not the default. The fleet case (multiple docker daemons on
different machines) is handled at the call-protocol layer (a `CallClient`
per remote daemon, each running alknet-docker locally), not by pointing
one bollard client at a remote daemon over TCP. See ADR-058 §"Why shared
`alknet/call`" and the POC §6 (the normalization crate boundary).

### 3. Do not enable `websocket`, `ssl`, `ssh`, or `buildkit`

- **`websocket`** — the reliable `attach_container()` (HTTP upgrade to
  TCP) is the attach path for `DockerTtyBackend` (ADR-061). The websocket
  attach endpoint has documented reliability issues (bollard
  `container.rs:577`) and is for browser-attach scenarios alknet-docker
  doesn't have. The raw chunk format (ADR-052) is layered on top of the
  reliable attach, not the websocket.
- **`ssl`** — remote daemons are reached over the call protocol (the
  fleet layer), not bollard's TLS. alknet-docker is single-host by
  construction (POC §6).
- **`ssh`** — the SSH-tunnel model is what the call-protocol fleet
  layer replaces. The dispatch POC's `DockerProvider` SSHed into workers;
  alknet-docker's workers dial the hub over `alknet/call` and expose
  their local docker ops directly.
- **`buildkit`** — image build is deferred (OQ-049). The `build_image`
  method is in bollard but the buildkit feature pulls tonic +
  bollard-buildkit-proto, a heavy dependency for an out-of-scope feature.

### 4. `time` feature for log timestamps

`docker/container/logs` sets `timestamps: true` on the bollard `logs()`
query. Each `LogOutput` then carries a docker timestamp; the
`call.responded` frame separates `timestamp` and `text` into distinct
JSON fields (the POC noted this refinement over its single-`text`-field
POC shape). The `time` feature types the timestamp field as
`time::OffsetDateTime` (serde-compatible). The `chrono` feature is the
alternative; `time` is lighter and sufficient.

The timestamp-feature choice (`time` vs `chrono`) is a two-way-door
implementation detail within the one-way version-pin decision. Switching
features is a `Cargo.toml` change, not an API break.

## Consequences

**Positive:**

- bollard 0.21 is current (verified against crates.io); no stale-version
  risk. The POC's API-surface mappings hold against the published version.
- The feature set is minimal: `http` + `pipe` (default) + `time`. No
  `ssl`/`ssh`/`websocket`/`buildkit` weight. The compile-time and
  dependency surface stays small.
- The single-host contract is clear: alknet-docker talks to one local
  daemon over the unix socket. The fleet case is a call-protocol concern,
  not a bollard-feature concern.

**Negative:**

- The `websocket` feature being disabled means the browser-attach
  scenario (bollard's websocket attach for a browser that speaks docker's
  websocket framing directly) is not available. This is intentional —
  browsers reach docker through `alknet/tty` (via `DockerTtyBackend`) or
  the call protocol, not bollard's websocket endpoint. If a future use
  case forces the websocket path, enabling the feature is a `Cargo.toml`
  addition (two-way door).
- The `ssh` feature being disabled means bollard's SSH-tunneled daemon
  path is not available. This is intentional — the dispatch POC's
  SSH-tunnel model is what alknet's call-protocol fleet layer replaces
  (POC §6, "Prior art"). Enabling `ssh` would reintroduce the friction
  (SSH key injection, port binding) the fleet layer exists to remove.

## Door type

**One-way (version pin) + two-way (feature set).** The version pin
(bollard 0.21, not 0.18 or a future 0.22) is a one-way commitment for
the implementation: the operation handlers are written against 0.21's
API surface, and a major-version bump is a migration. The feature set
(`http` + `pipe` + `time`, no `ssl`/`ssh`/`websocket`/`buildkit`) is
two-way-door: features can be added or removed in `Cargo.toml` without
an API break. The decision to *not* enable `ssl`/`ssh`/`buildkit` is a
scope decision (not needed for the current scope), not a capability
closure — the features can be added later if a use case forces them.

The `time`-vs-`chrono` choice is a two-way-door implementation detail.

## References

- `docs/research/alknet-docker/poc-summary.md` §"Open Unknowns" #5
  (bollard version pinning)
- `docs/research/alknet-docker/poc-summary.md` §"What the POC Does NOT
  Validate" #5 (image management / buildkit deferred)
- bollard 0.21.0 published `Cargo.toml` (verified via docs.rs source)
- bollard source: `src/container.rs` (`attach_container` :540, `logs`
  :928), `src/exec.rs` (`create_exec` :172, `start_exec` :225,
  `inspect_exec` :315), `src/read.rs` (`NewlineLogOutputDecoder` :32)
- [ADR-058](058-alknet-docker-on-alknet-call.md) — the single-host /
  call-protocol-fleet contract this feature set reflects
- [ADR-061](061-docker-tty-backend-in-alknet-docker.md) — the
  `DockerTtyBackend` that uses the reliable (non-websocket) attach path
- OQ-049 (build/image scope, deferred)
- Spec: [overview.md](../crates/docker/overview.md) §"Dependencies"