# ADR-063: Exit Code on a Terminal `call.responded` for Non-Interactive Exec

## Status

Accepted

## Context

The alknet-docker POC (`docs/research/alknet-docker/poc-summary.md`
§"POC Target 3") validated a completion shape for streaming exec:
the exit code rides on a final `call.responded` frame before
`call.completed`. This keeps `call.completed`'s payload empty
(matching ADR-012's wire format — no core protocol change). The POC
used `{ "exitCode": N }` on the final `call.responded`.

The docker spec (`docker-operations.md` §"docker/container/exec")
carries this forward and adds a `"terminal": true` field to mark the
exit-code `call.responded` as the final value before completion. The
`terminal` flag is a docker-operation convention, not a call-protocol
wire-format change — it's a field in the `call.responded` payload
(the `Value`), not a new event type or a `call.completed` payload.

A reader asked: "why `terminal: true` and not a dedicated `call.exit`
event or an exit code on `call.completed`?" This ADR records the
decision.

### Why not an exit code on `call.completed`

ADR-012 defines `call.completed` with an empty payload (`{}`). The
POC's completion-shape decision (POC §"The completion-shape decision
this validates") chose to keep `call.completed` empty and put the
exit code on the preceding `call.responded`. Changing `call.completed`
to carry a payload would be a wire-format change (ADR-012 amendment,
new ALPN per ADR-006) — a one-way door that affects every
`call.completed` consumer, not just docker. The POC deliberately
avoided this.

### Why not a dedicated `call.exit` event

A new event type (`call.exit`) would be a wire-format addition (new
`EventEnvelope` variant, ADR-012 amendment). It would also duplicate
the "terminal value before completion" semantics that
`call.completed` already provides — `call.completed` is the
"stream end" signal; an exit code is a "final value" that rides
*before* the stream end. A `EventEnvelope` variant conflates these
two: `call.exit` would be both the final value and the stream-end
signal, but `call.completed` already handles stream end. Keeping
the exit code as a `call.responded` (a normal value) and letting
`call.completed` be the stream end preserves the
one-value-per-`call.responded` invariant: the exit code is just the
last value.

### Why `terminal: true`

A client consuming a `docker/container/exec` stream sees a sequence
of `call.responded` frames (stdout/stderr lines) and then a final
`call.responded` with `{ "exitCode": N, "terminal": true }`, then
`call.completed`. The `terminal: true` flag tells the client "this
is the last value; the next event is `call.completed`." Without it,
the client would have to infer "this `call.responded` has an
`exitCode` field, so it's the exit" — a content-sniffing heuristic
that breaks if a future stdout line happens to carry an `exitCode`
field.

The `terminal` flag is a docker-operation convention — a field in
the `call.responded` payload, not a protocol-level marker. It's
declared in the `docker/container/exec` `output_schema` (the exit
frame is a documented part of the output stream). Other
`Subscription` operations that produce a "terminal result before
completion" can adopt the same convention (a `terminal: true` field
on their final `call.responded`); it's not docker-specific, but
docker is the first operation to need it.

## Decision

### 1. The exec exit code rides on a final `call.responded` with `terminal: true`

`docker/container/exec` (non-interactive, `tty: false`) emits:

1. Zero or more `call.responded` frames with stdout/stderr output
   (`{ "stream": "stdout"|"stderr", "text": "..." }`).
2. A final `call.responded` with `{ "exitCode": N, "terminal": true }`.
3. `call.completed` (empty payload, per ADR-012).

The `terminal: true` field marks the exit-code frame as the final
value before completion. The `exitCode` is `i32` (matching
`ExecInspectResponse.exit_code`; negative for signal-terminated,
though docker exec exit codes are typically non-negative).

### 2. `call.completed` stays empty

No `call.completed` payload change. ADR-012's wire format is
unchanged. The exit code is on the preceding `call.responded`, not
on `call.completed`.

### 3. `terminal: true` is an operation-level convention, not a protocol field

The `terminal` flag is a field in the `call.responded` payload (the
`Value`), declared in the operation's `output_schema`. It is not a
new `EventEnvelope` field, not a new event type, and not a
`call.completed` payload. The call protocol's wire format is
unchanged. Other `Subscription` operations that produce a terminal
result before completion may adopt the same convention; it's not
docker-specific, but docker is the first to use it.

The `OperationSpec.output_schema` for `docker/container/exec`
documents the two response shapes: the streaming output frames
(`{ "stream": ..., "text": ... }`) and the terminal exit frame
(`{ "exitCode": N, "terminal": true }`). A client reading the schema
knows to expect the terminal frame.

### 4. The exit-code frame is the last `call.responded` before `call.completed`

The handler ensures the exit-code `call.responded` is the last one
the stream produces. The `StreamingHandler` (ADR-049) pumps the
output stream, then emits the exit-code frame, then the stream ends
(the dispatcher writes `call.completed` on natural stream end). The
ordering is the handler's responsibility (the handler controls the
stream's item order); the dispatcher's `pump_stream` writes them in
order. This mirrors the POC's validated pattern.

## Consequences

**Positive:**

- The exit code propagates through the streaming completion path
  without a wire-format change. `call.completed` stays empty
  (ADR-012 unchanged); the exit code is a normal `call.responded`
  value that happens to be the last one.
- The `terminal: true` flag gives clients a deterministic "this is
  the exit" signal without content-sniffing the `exitCode` field. A
  client can stop reading output frames when it sees `terminal: true`
  and read the exit code.
- The convention is reusable: any `Subscription` operation with a
  terminal result before completion can use `terminal: true` on its
  final `call.responded`. No protocol change; just an
  operation-level field.

**Negative:**

- The `terminal` flag is a convention, not enforced by the wire
  format. A `Subscription` operation that emits a terminal result
  *without* `terminal: true` would be non-conformant but not
  protocol-violating. The `output_schema` documents the convention;
  clients that read the schema know to expect it.
- A client that doesn't check `terminal: true` and instead
  content-sniffs `exitCode` would work for docker exec but break on
  a hypothetical stdout line that carries an `exitCode` field. The
  flag is the robust path; content-sniffing is the fragile path. The
  spec recommends the flag; the schema documents it.

## Door type

**One-way.** The exit-code-on-`call.responded`-before-`call.completed`
shape is the completion contract `docker/container/exec` commits to.
Clients consuming the exec stream depend on this ordering; changing
it (moving the exit code to `call.completed`, or adding a `call.exit`
event) would be a break for those clients.

The `terminal: true` field name is two-way-door within the one-way
shape — it could be renamed (e.g., `final: true`) without a wire-
format change, since it's a payload field, not a protocol marker.
But the *presence* of a terminal marker on the final `call.responded`
is the one-way commitment: clients depend on "the last
`call.responded` before `call.completed` is marked as terminal and
carries the exit code."

## References

- `docs/research/alknet-docker/poc-summary.md` §"POC Target 3"
  (exec with exit code — the validated pattern this ADR formalizes)
- `docs/research/alknet-docker/poc-summary.md` §"The completion-shape
  decision this validates" (the POC's reasoning for exit-on-
  `call.responded`, not on `call.completed`)
- [ADR-012](012-call-protocol-stream-model.md) — the call protocol
  wire format this ADR keeps unchanged (`call.completed` stays empty)
- [ADR-049](049-streaming-handler-for-subscriptions.md) —
  `StreamingHandler`, the dispatch path that pumps the
  `call.responded` stream and writes `call.completed` on stream end
- [ADR-023](023-operation-error-schemas.md) — `output_schema`
  documents the terminal frame shape
- [ADR-058](058-alknet-docker-on-alknet-call.md) — the operation
  taxonomy (exec is a `Subscription`)
- Spec: [docker-operations.md](../crates/docker/docker-operations.md)
  §"docker/container/exec" (the operation this ADR's shape serves)