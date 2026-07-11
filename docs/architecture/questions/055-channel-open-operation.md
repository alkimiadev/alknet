# OQ-55: channel/open Call-Protocol Operation

- **Origin**: [crates/hub/README.md](crates/hub/README.md)
- **Status**: open
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: Not yet decided.

The hub's channel model requires a `channel/open` operation on the call
protocol. This operation opens a new bidirectional QUIC stream on the
existing connection, wraps it as a `Connection` via
`Connection::from_bidi` (ADR-065), and hands it to the `HandlerRegistry`
for the requested ALPN. The operation is symmetric — either side can call it.

The operation shape is committed:

```
Request:  { "alpn": "alknet/tty", "channel": 1 }
Response: { "channel": 1, "status": "open" }
```

What is not yet decided:

- **Where the operation lives.** The `channel/open` handler needs access to
  the underlying QUIC connection to open a new stream. This is a
  `CallConnection` concern — the handler needs a reference to the connection
  it's running on. Options: (a) the handler is registered with a closure
  capturing the `CallConnection`, (b) the `OperationContext` carries a
  reference to the connection, (c) a new `ConnectionHandle` type that the
  handler can use to open streams.

- **Channel number assignment.** The caller suggests a channel number; the
  receiver may accept or reassign. The reassignment policy (first-available,
  caller-always-wins, receiver-always-wins) is not decided.

- **Handler dispatch on the receiving side.** When a `channel/open` request
  arrives, the receiver opens the stream, wraps it as `Connection::from_bidi`,
  and dispatches to the `HandlerRegistry` for the requested ALPN. The
  dispatch mechanism (spawn a task, register in the endpoint's accept loop,
  or a separate channel dispatch path) is not decided.

- **`channel/close` and `channel/list`.** These are companion operations.
  `channel/close` drops the `Connection` and cancels the handler task.
  `channel/list` returns the set of open channels. Their shapes are
  straightforward; the implementation depends on the `channel/open`
  mechanism.

The channel model is committed. The operation spec is deferred to the
call-protocol implementation phase — it depends on the `channel/open`
mechanism decision.

- **Cross-references**: ADR-065, [crates/hub/README.md](crates/hub/README.md)
