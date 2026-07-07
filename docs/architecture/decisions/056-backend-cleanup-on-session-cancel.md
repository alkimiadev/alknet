# ADR-056: Backend Cleanup on Session Cancel (Drop of `exit_code` Kills)

## Status

Accepted

## Context

A `alknet/tty` session can be cancelled before the child process exits
naturally. Three cancel paths exist (see `tty-adapter.md` §"Connection and
Stream Lifecycle"):

1. **Connection drop** — the QUIC connection closes; all in-flight sessions
   on it are cancelled.
2. **Stream reset** — the client (or transport) resets the bidi stream
   mid-session.
3. **Task panic / adapter shutdown** — the session pump task exits without
   completing the normal exit-chunk sequence.

In all three, the adapter's per-session pump tasks are dropped (Rust
`Drop`). The pump tasks hold the `TtyHandle`, whose fields — `stdin`,
`stdout`, `stderr`, `exit_code`, `control` — are dropped too.

The problem the local-PTY POC surfaced: **`Drop` alone is not sufficient
cleanup for a backend whose child may outlive the session.** The local
backend's three std threads illustrate the gap:

- **Reader thread** — blocking `MasterPty::try_clone_reader()` reads. On
  `mpsc::Sender::drop` (the stdout channel's sender side drops when the
  handle drops), the reader's next `blocking_send` returns a `SendError`;
  the thread exits. ✓ `Drop` works.
- **Writer thread** — drains an `mpsc::Receiver<StdinCmd>`. On
  `Receiver::drop`, the writer's `recv()` returns `None`; the thread
  drops the master writer (EOF to the slave's stdin) and exits. ✓ `Drop`
  works.
- **Waiter thread** — blocking `Child::wait()`. This is a syscall that
  returns **only when the child is reaped**. It does not observe channel
  close. If the child ignores stdin EOF (a daemon, a long-lived process
  with no stdin reader, a process in an uninterruptible state), the
  waiter thread stays blocked indefinitely and the child is **orphaned**.
  ✗ `Drop` does not work here.

The same concern applies to any backend whose session target can outlive
the bidi stream: a docker container with `tty: true` whose process ignores
the channel close; an SSH exec whose remote process doesn't exit on
channel close. The unifying property is **a backend-allocated session
target that may outlive the client's interest in it.**

The earlier spec text (`tty-adapter.md` §"Connection and Stream
Lifecycle" pre-this-ADR) asserted:

> No explicit cleanup is needed — `Drop` is the cleanup.

That is wrong for the waiter thread and any backend in the same shape.
This ADR corrects the claim and commits a cleanup contract that closes
the gap.

### Why this is architectural, not implementation

The cleanup contract is part of the `TtyBackend` trait's behavioral
contract (ADR-053), not a backend-internal detail, for two reasons:

1. **The adapter depends on the property.** The adapter's session
   driver holds the `TtyHandle` and, on cancel, drops it. The adapter
   cannot itself call a backend-specific kill — it doesn't know the
   child's pid (the local backend owns it; the adapter never sees it).
   The kill must be wired into the backend's handle shape, specifically
   into the `exit_code` future the adapter drops on cancel. The
   contract is what makes "the adapter drops the handle" sufficient.
2. **A missing contract leaks processes.** An implementer writing a
   backend from the trait sketch, without this contract, would ship a
   backend that orphans processes on cancel. The bug is silent (the
   client got what it wanted; the orphaned process is a server-side
   leak), surfaces only under cancel-heavy workloads or long-lived
   sessions, and is expensive to attribute after the fact. The contract
   makes the property load-bearing at the seam, not a property each
   backend rediscovers.

## Decision

### 1. The `TtyBackend` cleanup contract: cancelling the `exit_code` future kills the session target

When the adapter cancels a session (drops the pump tasks, which drops the
`TtyHandle`), the backend's `exit_code` future — the
`BoxFuture<'static, Result<i32, TtyError>>` field of `TtyHandle`
(ADR-053) — is dropped *without being driven to completion*. The cleanup
contract:

> **Dropping the `exit_code` future MUST kill the session target (the
> child process, the docker exec, the SSH channel's process).** The kill
> is best-effort (the target may already be exiting; the kill is a
> no-op then), but it MUST be attempted. The kill MUST be delivered
> even when the session target is blocked in a state that ignores stdin
> EOF (a daemon, a process in uninterruptible sleep, a container whose
> process ignores channel close).

`exit_code`'s `Drop` is the cancel path. The adapter drives the future to
completion (the happy path — the child exits, the future resolves, the
adapter sends the exit chunk); on cancel, the adapter drops the future
(their `Drop`), which runs the kill.

This is a behavioral contract on the `TtyBackend` trait, not a new method.
The trait's `allocate()` returns a `TtyHandle` whose `exit_code` field is
a `Future` with a `Drop`-on-cancel that kills. The mechanism is
backend-specific (see §3 for the local backend); the contract is
backend-agnostic.

### 2. `exit_code`'s `Drop`-on-cancel MUST be safe to run after the future resolves

If the future resolved normally (the adapter awaited it, got the exit
code, sent the exit chunk), the `Drop` runs on an already-resolved
future. The kill MUST be a no-op in that case — the child is already
reaped, the kill is delivered to a nonexistent pid, etc. This is the
"best-effort" qualifier: a kill on an already-exited child is not an
error. Backends implement this with a guard that distinguishes
"resolved" from "cancelled" (a flag, an `Option` taken on resolve, an
`Arc`-shared state).

### 3. Local backend mechanism: `ChildKiller` held in the `exit_code` future's `Drop` guard

The local backend (`alknet-tty-local`, ADR-054) implements the contract
using `portable_pty::ChildKiller` — the kill handle `portable_pty`
exposes alongside `Child::wait()`. The pattern:

- `allocate()` spawns the child via `portable_pty`, obtaining a `Child`
  (with `wait()`) and a `ChildKiller` (with `kill()`). It moves the
  `Child` into the waiter thread (which blocks on `wait()`). It wraps
  the `ChildKiller` and the `oneshot::Receiver<i32>` (from the waiter
  thread) into a `Future` that becomes `TtyHandle.exit_code`.
- The `exit_code` future's `poll` delegates to the inner
  `oneshot::Receiver::poll` (resolves when the waiter thread sends the
  exit code).
- The `exit_code` future's `Drop` (runs on cancel only — on resolve, the
  `Drop` is a no-op via the guard) calls `ChildKiller::kill(SIGHUP)`
  (or the backend's configured cancel signal). The kill causes the
  child to exit; the waiter thread's `wait()` returns; the waiter
  thread's `oneshot::send` fails silently (the receiver was dropped
  with the future). The waiter thread then exits. The child is reaped
  by the waiter thread's `wait()`; no zombie.

For pipe mode (`terminal: None`), the same pattern applies with
`tokio::process::Child::start_kill()` (or `Child::kill()`) instead of
`ChildKiller`. The `exit_code` future's `Drop` guard calls
`start_kill()`; the `Child` is reaped by the future's `wait()` (or by
the waiter task).

### 4. Future backends (docker, SSH) follow the same contract

- **Docker (`DockerTtyBackend`)** — `bollard`'s exec stream is cancelled
  by dropping the `AttachContainer` / `start_exec` stream and calling
  `bollard::container::kill_container` (or `exec::kill_exec` if
  available). The `exit_code` future's `Drop` holds the container/exec
  id and the `bollard::Docker` client; on cancel, it issues the kill.
- **SSH (`SshTtyBackend`)** — russh's `Channel::close()` and/or
  `Channel::signal(SIGHUP)` terminate the remote process. The
  `exit_code` future's `Drop` holds the russh channel handle; on
  cancel, it closes the channel.

The docker and SSH backends are future work (out of scope for this spec
set); the contract is what they implement. A future backend that does
NOT fit the contract (e.g., a "recorded session replay" backend with no
live process) implements a no-op `Drop`-on-cancel — the contract is
"kill if there is a killable target; no-op if not."

### 5. The adapter does not call a backend kill method

The adapter has no `TtyBackend::cancel()` or `TtyHandle::kill()` method
to call — the cleanup is wired into the `exit_code` future's `Drop`,
which the adapter triggers by dropping the future. This keeps the trait
surface unchanged (no new method) and the cleanup in the backend (where
the kill handle lives). The adapter's only responsibility is to drop the
`TtyHandle` (and therefore the `exit_code` future) when the session is
cancelled — which it already does by virtue of dropping the pump tasks.

The `TtyControl::signal("HUP")` path (ADR-053) is the *client-initiated*
signal forwarding path — a client sends a `{"type":"signal","name":"HUP"}`
control chunk. It is NOT the cancel-cleanup path. The cancel-cleanup
path is server-internal (the adapter drops the handle) and does not
involve the wire format. These are two different signal paths; both end
in the child receiving SIGHUP (or the backend's configured cancel
signal), but they are triggered by different actors (client vs. server
cancel).

## Consequences

**Positive:**

- A backend that conforms to the contract cannot orphan a process on
  cancel. The local-PTY POC's waiter-thread gap is closed at the
  contract level, not left to each backend to rediscover.
- The cleanup is idiomatic Rust — `Drop`-on-cancel of a `Future` is the
  standard pattern for resource cleanup in async Rust (the same pattern
  `tokio::process::Child` uses; the same pattern `tokio::io` AsyncRead
  guards use). No new trait method; no adapter-side kill call.
- The contract is backend-agnostic — the mechanism (`ChildKiller` for
  local, `kill_container` for docker, `channel::close` for SSH) lives
  in the backend; the contract ("drop the future, the target dies")
  lives at the seam.
- The happy path (the child exits, the adapter drives `exit_code` to
  completion, sends the exit chunk, then drops the resolved future) is
  unaffected — the `Drop`-on-resolve is a no-op via the guard.

**Negative:**

- The `exit_code` future is no longer a trivial `oneshot::Receiver<i32>`
  wrapper; it carries a kill guard. This is a small implementation
  complexity increase (a struct with a `Drop` impl and a resolved-flag),
  but it is the cost of the contract. The POC's `LocalPty::exit_code`
  was a bare `oneshot::Receiver<i32>`; the spec'd `TtyHandle.exit_code`
  is a struct wrapping it. An implementer who copies the POC's bare
  shape without the kill guard violates the contract.
- The contract is behavioral, not type-enforced. Rust cannot require
  "the `Drop` of the future returned by `allocate()` kills the child"
  in the type system. The contract is documented in the `TtyBackend`
  trait's doc comment and in this ADR; conformance is the
  implementer's responsibility. A test (a "cancel mid-session" test
  that asserts the child is reaped after the session is dropped)
  should be part of each backend's integration suite.
- A backend whose session target genuinely cannot be killed (a
  backend that wraps an immutable shared resource, e.g., a "view a
  log stream" backend) implements the contract as a no-op. The
  contract is "best-effort kill if there is a killable target"; a
  no-op `Drop`-on-cancel is conformant for a non-killable target.

## Door type

**One-way.** The cleanup contract is part of the `TtyBackend` behavioral
contract. Clients (the adapter) depend on "drop the handle, the session
is cleaned up." Changing the contract after backends exist — e.g.,
adding a separate `TtyBackend::cancel()` method and migrating the
cleanup out of `exit_code`'s `Drop` — would require every backend to
change. The `exit_code`-future-`Drop`-on-cancel mechanism is the seam.

## Assumptions

1. **The `exit_code` future's `Drop` is the only cancel path.** The
   adapter does not call a separate kill method; it drops the handle.
   This means the cleanup runs in the same place the cancel happens (the
   pump task's `Drop`), not in a separate cancel call. This is the
   idiomatic Rust async cancel pattern and the one the trait commits.
2. **The kill signal is the backend's configured cancel signal
   (SIGHUP for the local backend, the docker/SSH equivalent).** This is
   a server-internal signal path, distinct from the
   client-initiated `TtyControl::signal()` path (ADR-053). The cancel
   signal is not configurable from the wire format in v1; a backend
   that needs a different cancel signal configures it internally.
3. **The waiter thread (local backend) reaps the killed child.** After
   the `Drop`-on-cancel calls `ChildKiller::kill(SIGHUP)`, the child
   exits; the waiter thread's `wait()` returns and reaps it (no
   zombie). The waiter thread then exits. The `oneshot::send` from the
   waiter thread fails silently (the receiver was dropped with the
   future) — this is expected and not an error.

## References

- `tty-adapter.md` §"Connection and Stream Lifecycle" — the cancel paths
  (connection drop, stream reset) that trigger the contract
- `tty-local.md` §"PTY Mode" — the local backend's three-thread bridge
  and the waiter-thread gap this ADR closes
- [ADR-052](052-alknet-tty-wire-format-and-two-carriage.md) — the wire
  format the cancel does not involve (the cleanup is server-internal)
- [ADR-053](053-ttybackend-trait-and-ttyhandle.md) — the `TtyBackend`
  trait this contract is part of; the `exit_code` field the cleanup
  wires into; the `TtyControl::signal()` path the cancel-cleanup path
  is distinct from
- [ADR-054](054-local-tty-backend-sibling-crate.md) — the local backend
  this ADR's reference mechanism (`ChildKiller`) is for
- [ADR-055](055-exit-code-on-control-chunk.md) — the happy-path
  exit-chunk sequence (the cancel path bypasses it; no exit chunk is
  sent on cancel — see `tty-adapter.md` §"Stream reset")
- `/workspace/alknet-tty-poc/src/local_pty.rs` — the POC's `LocalPty`
  shape (the reference the contract generalizes; the POC's bare
  `oneshot::Receiver<i32>` `exit_code` is the shape an implementer
  must NOT copy without adding the kill guard)
- `portable-pty` 0.9 `ChildKiller` — the kill handle the local
  backend's cancel-cleanup uses
- Spec: [crates/tty/tty-backend.md](../crates/tty/tty-backend.md),
  [crates/tty/tty-adapter.md](../crates/tty/tty-adapter.md),
  [crates/tty/tty-local.md](../crates/tty/tty-local.md)