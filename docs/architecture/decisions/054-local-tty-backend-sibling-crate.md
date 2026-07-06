# ADR-054: Local TTY Backend as a Sibling Crate (`alknet-tty-local`)

## Status

Accepted

## Context

The alknet-tty research (`docs/research/alknet-tty/phase-0-findings.md`
DP-1) posed the placement question for the local-process backend
(`std::process::Command` with piped stdio, or `portable_pty` for a real
PTY):

- **(a) In alknet-tty**: the crate ships with the local backend built-in.
  Pro: zero-config runner, one crate gets a terminal/process-streaming
  endpoint. Con: alknet-tty pulls in `portable_pty` even for deployments
  that only use docker/ssh backends.
- **(b) In a sibling crate (`alknet-tty-local`)**: alknet-tty defines the
  trait (ADR-053); the local backend is a separate crate. Pro:
  alknet-tty stays dependency-light; consumers opt into the local
  backend explicitly. Con: one extra crate for the common case.

The local backend is the simplest backend and the one that enables the
runner pattern (a process whose stdin/stdout/stderr/exit-code stream over
a framed bidi connection — the same shape as GitHub/Gitea Actions runners,
just over alknet's transport instead of HTTP polling). It has no heavy
dependencies in the pipe case — just `std` — but the PTY case pulls in
`portable_pty` (a non-trivial native dependency that builds on Unix
`openpty`/`ioctl` and Windows ConPTY).

The relevant constraint from ADR-053: alknet-tty itself depends only on
alknet-core and the wire-format codec (ADR-052). The `portable_pty`
dependency does not belong in the core tty crate — a docker-only
deployment or an SSH-PTY-only deployment should not pull in PTY allocation
code. This is the same inversion as `OperationAdapter` (ADR-017): the
trait lives where the types live; the implementations live where their
transport dependencies live.

### The feature-re-export compromise

The research recommended (b) sibling crate **behind a feature flag on
alknet-tty** for the common case (`features = ["local"]` → re-export from
`alknet-tty-local`). This keeps alknet-tty's default dependency surface
minimal while making the local backend a one-feature opt-in. The
`portable_pty` dependency lives in `alknet-tty-local`; alknet-tty itself
never depends on `portable_pty`.

## Decision

### 1. The local backend is a sibling crate, `alknet-tty-local`

`alknet-tty-local` implements `TtyBackend` for `LocalTtyBackend` (ADR-053),
backed by `portable_pty` for the PTY case and `std::process::Command` with
`Stdio::piped()` for the pipe/runner case. It depends on alknet-tty (for
the `TtyBackend`/`TtyHandle`/`TtyControl` traits) and on `portable_pty`
for the PTY case.

```
alknet-tty-local
├── alknet-tty      (TtyBackend trait, TtyHandle, TtyControl, wire types)
├── alknet-core      (Connection — via alknet-tty's re-export; not direct)
├── portable_pty     (PTY allocation — the heavy dep, here not in alknet-tty)
└── libc             (signal forwarding — REQ-TTY-02, Unix only)
```

### 2. `alknet-tty` re-exports the local backend behind a `local` feature

```toml
# alknet-tty Cargo.toml
[features]
default = []
local = ["dep:alknet-tty-local"]   # re-export LocalTtyBackend

[dependencies]
alknet-tty-local = { path = "../alknet-tty-local", optional = true }
```

A consumer that wants the local backend enables `features = ["local"]`
on alknet-tty and gets `alknet_tty::local::LocalTtyBackend` re-exported
from `alknet-tty-local`. A consumer that only wants docker/ssh backends
uses the default features and depends on `alknet-tty-docker` /
`alknet-tty-ssh` (or their own backend crate) directly — no
`portable_pty` in the dependency tree.

This is the same feature-re-export pattern the Rust ecosystem uses for
optional heavy dependencies (e.g., `tokio`'s `full` feature pulling in
`tokio-util`, `h2`, etc.). The seam is the `TtyBackend` trait; extraction
is cheap because the trait is the contract.

### 3. PTY vs pipe is a per-session choice, not a per-deployment choice

`TtyParams.terminal: Option<TerminalParams>` (ADR-053) selects the mode:

- **`terminal: Some(TerminalParams { ... })`** — allocate a real PTY
  via `portable_pty`. Terminal semantics: resize (via
  `ioctl(TIOCSWINSZ)`), signal delivery to the foreground process group
  (via `libc::kill(-pgid, sig)`, REQ-TTY-02), escape-sequence handling
  (the kernel PTY's line discipline). stdout and stderr are merged
  (kernel PTY property — one output stream from the slave), so
  `TtyHandle.stderr` is `None`.
- **`terminal: None`** — pipe mode, no PTY. `std::process::Command` with
  `Stdio::piped()` for stdin/stdout/stderr. No resize, no
  escape-sequence handling, but `kill(pid, sig)` still works for signal
  forwarding. stdout and stderr are separate streams, so
  `TtyHandle.stderr` is `Some`. This is the **runner** case — a
  command-streaming endpoint with no terminal semantics.

The same `LocalTtyBackend` serves both; the `allocate()` call branches on
`params.terminal`. A deployment that only does terminals always sends
`Some`; a deployment that only does runners always sends `None`; a
deployment that does both (a hub that runs agents in PTYs and runs
`cargo test` as a runner) sends the appropriate one per session.

### 4. The runner pattern is preserved, not specialized

The pipe mode (`terminal: None`) is the "runner" generalization the
research identified: a process whose stdin/stdout/stderr/exit-code stream
over a framed bidi connection. This is functionally identical to
GitHub/Gitea Actions runners, just over alknet's transport instead of
HTTP polling:

- A coordinator sends a negotiation frame with
  `{ "backend": "local", "tty": null, "cmd": ["cargo", "test"] }`.
- The endpoint runs `cargo test` with piped stdio, streams stdout/stderr
  chunks back, sends `{"type":"exit","code":N}` when it finishes (ADR-055).
- The coordinator gets reliable completion notification (the exit
  control chunk + stream close) — no polling.

The runner-specific API surface (job management, log persistence, task
graph integration) is **out of scope for alknet-tty**. alknet-tty provides
the *mechanism* (a framed byte stream for a process); the runner *policy*
is a downstream crate's job. This ADR commits to preserving the option
(`terminal: None` → pipe mode) and not building runner policy into
alknet-tty. See OQ-46.

## Consequences

**Positive:**

- alknet-tty's default dependency surface is minimal (alknet-core + the
  wire-format codec). A docker-only or ssh-only deployment never pulls
  in `portable_pty`.
- The local backend is a one-feature opt-in (`features = ["local"]`) for
  the common case — a consumer that wants a terminal/runner endpoint with
  no docker or SSH gets it with one feature flag, not a separate
  dependency.
- PTY vs pipe is per-session, so one `LocalTtyBackend` serves terminals
  and runners. A hub that does both doesn't need two backends.
- The runner pattern is preserved without baking runner policy into
  alknet-tty. The mechanism is the framed byte stream; the policy is
  downstream.
- The sibling-crate placement composes with ADR-003's no-handler-depends-
  on-another-handler rule: alknet-tty-local depends on alknet-tty (for
  the trait), alknet-tty does not depend on alknet-tty-local (the
  feature re-export is optional).

**Negative:**

- A consumer that wants the local backend must enable a feature flag
  (`features = ["local"]`). Forgetting the flag results in the
  `alknet_tty::local` module not existing — a compile error, not a silent
  miss. This is the standard Rust feature-flag trade and is
  self-documenting.
- One extra crate in the workspace (`alknet-tty-local` alongside
  `alknet-tty`). The workspace `Cargo.toml` gains a member; version
  coordination is per-crate. This is the established pattern (alknet-http
  is one crate with colocated server + adapters; alknet-call is separate
  from alknet-core).
- The runner-specific API surface (job management, log persistence) is
  not in alknet-tty. A downstream crate that wants a full runner builds
  on the pipe mode + the wire format. This is the right layering
  (mechanism vs policy) but means a "runner crate" is a separate future
  deliverable, not part of alknet-tty. See OQ-46.

## Door type

**Two-way.** The sibling-crate placement is reversible: if the local
backend turned out to be the only backend anyone used, merging
`alknet-tty-local` back into `alknet-tty` (behind the same `local`
feature, just in the same crate) is mechanical — the trait is the seam,
and the feature gate already exists. The cost of reversal is low
(re-exports become local modules), and no downstream consumer breaks (the
`alknet_tty::local::LocalTtyBackend` path stays valid).

This is a two-way door that is **decided** (sibling crate + feature
re-export), not deferred. The decision is made now; the reversal is cheap
if a future consolidation warrants it. See ADR-009 §"What this framework
is NOT" — door type classifies reversal cost, not urgency.

## References

- `docs/research/alknet-tty/phase-0-findings.md` DP-1 — the placement
  question this ADR resolves
- [ADR-003](003-crate-decomposition.md) + Amendment 1 — crate
  decomposition rule (the sibling-crate placement preserves it)
- [ADR-009](009-one-way-door-decision-framework.md) — door-type-as-deferral
  anti-pattern (this ADR's two-way-door classification is reversal cost,
  not a deferral)
- [ADR-017](017-call-protocol-client-and-adapter-contract.md) — the
  adapter-location-map pattern (trait where types live, implementation
  where deps live) this ADR follows
- [ADR-053](053-ttybackend-trait-and-ttyhandle.md) — the `TtyBackend`
  trait this crate implements
- OQ-46 — runner API surface (deferred(scope): mechanism in alknet-tty,
  policy is a downstream crate)
- Spec: [crates/tty/tty-local.md](../crates/tty/tty-local.md)