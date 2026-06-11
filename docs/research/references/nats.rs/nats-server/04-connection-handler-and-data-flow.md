# Connection Handler and Data Flow

This document covers the internal `ConnectionHandler` that drives all protocol I/O, and the data flow through the system.

## ConnectionHandler

**Location**: `lib.rs`

The `ConnectionHandler` is the heart of the client. It runs as a single Tokio task and manages all communication with the NATS server.

```rust
pub(crate) struct ConnectionHandler {
    connection: Connection,          // Low-level I/O
    connector: Connector,           // Server pool, reconnection
    subscriptions: HashMap<u64, Subscription>,  // Active subscriptions
    multiplexer: Option<Multiplexer>,           // Request-reply multiplexer
    pending_pings: usize,           // Unanswered PINGs
    info_sender: tokio::sync::watch::Sender<Option<ServerInfo>>,
    ping_interval: Interval,       // Periodic PING timer
    should_reconnect: bool,         // Flag for forced reconnect
    flush_observers: Vec<oneshot::Sender<()>>,   // Pending flush callbacks
    is_draining: bool,              // Connection is draining
    drain_pings: VecDeque<u64>,     // SIDs being drained
}
```

## Data Flow: Publish

```
Application
    │
    │  client.publish("events.data", payload)
    │
    ▼
Client
    │  validates subject & payload size
    │  sends Command::Publish(OutboundMessage) via mpsc channel
    │
    ▼
ConnectionHandler::handle_command(Command::Publish)
    │  increments out_messages, out_bytes statistics
    │  calls connection.enqueue_write_op(&ClientOp::Publish { ... })
    │
    ▼
Connection::enqueue_write_op
    │  serializes to wire format:
    │    "PUB events.data 11\r\n"  or  "HPUB events.data 23 34\r\n"
    │  appends to flattened_writes or write_buf
    │
    ▼
Connection::poll_write
    │  uses vectored writes (64 chunks) if supported
    │  or sequential writes otherwise
    │
    ▼
Connection::poll_flush
    │  flushes the TCP/TLS/WS stream
    │  notifies flush_observers
    │
    ▼
NATS Server (TCP/TLS/WebSocket)
```

## Data Flow: Subscribe

```
Application
    │
    │  client.subscribe("events.>")
    │
    ▼
Client::subscribe
    │  validates subject (always, regardless of skip_subject_validation)
    │  allocates next sid via AtomicU64
    │  creates mpsc channel for messages
    │  sends Command::Subscribe { sid, subject, sender }
    │  returns Subscriber { sid, receiver }
    │
    ▼
ConnectionHandler::handle_command(Command::Subscribe)
    │  creates Subscription { subject, sender, delivered: 0, max: None }
    │  inserts into subscriptions HashMap
    │  calls connection.enqueue_write_op(&ClientOp::Subscribe { sid, subject, queue_group })
    │
    ▼
Connection::enqueue_write_op
    │  serializes: "SUB events.> 42\r\n"
    │
    ▼
Server sends MSG for matching subjects:
    │
    ▼
ConnectionHandler::handle_server_op(ServerOp::Message { sid, subject, ... })
    │  looks up sid in subscriptions HashMap
    │  constructs Message { subject, reply, payload, headers, status, description }
    │  tries subscription.sender.try_send(message)
    │
    ├── Ok → increments subscription.delivered, checks max
    ├── Full → emits Event::SlowConsumer(sid)
    └── Closed → removes subscription, sends ClientOp::Unsubscribe
    │
    ▼
Subscriber::poll_next (Stream impl)
    │  receives from mpsc::Receiver
    │
    ▼
Application processes Message
```

## Data Flow: Request-Response

The request-response pattern uses the **multiplexer** — a single wildcard subscription that routes responses to their waiting requesters.

```
Application
    │
    │  client.request("service", payload)
    │
    ▼
Client::send_request
    │  validates subject & payload size
    │  creates oneshot channel for response
    │  generates unique inbox: "_INBOX.<nuid>.<token>"
    │  sends Command::Request { subject, payload, respond, sender }
    │
    ▼
ConnectionHandler::handle_command(Command::Request)
    │  extracts token from respond subject (after last '.')
    │  if no multiplexer exists:
    │     creates Multiplexer with wildcard sub "_INBOX.<id>.*" (SID 0)
    │     sends ClientOp::Subscribe { sid: 0, subject: "_INBOX.<id>.*" }
    │  inserts token → oneshot::Sender in multiplexer.senders
    │  sends ClientOp::Publish { subject, payload, respond: "<prefix><token>" }
    │
    ▼
Server routes request to service:
    │
    ▼
Service responds by publishing to the reply subject:
    │
    ▼
ConnectionHandler::handle_server_op(ServerOp::Message { sid: 0, ... })
    │  sid == MULTIPLEXER_SID (0), so enters multiplexer path
    │  extracts token by stripping prefix from subject
    │  looks up token in multiplexer.senders
    │  sends Message via oneshot::Sender
    │
    ▼
Client::send_request receives via oneshot::Receiver
    │  applies timeout (default 10s)
    │  checks for NO_RESPONDERS status (503)
    │
    ▼
Application receives Message
```

### Custom Inbox Request

If the `Request` builder specifies a custom `inbox`, the flow is different:
- The client subscribes to the inbox directly (not via multiplexer)
- Publishes with the inbox as the reply subject
- Waits for the message on that subscription
- No multiplexer involvement

## Data Flow: Flush

```
Application
    │
    │  client.flush()
    │
    ▼
Client::flush
    │  creates oneshot channel
    │  sends Command::Flush { observer }
    │
    ▼
ConnectionHandler::handle_command(Command::Flush)
    │  pushes observer into flush_observers Vec
    │
    ▼
ProcessFut::poll (main loop)
    │  after writing all pending data...
    │  checks should_flush():
    │    Yes (write buffers empty, not yet flushed) → poll_flush
    │    May  (write buffers not empty) → poll_flush
    │    No   (already flushed) → skip
    │  on successful flush:
    │    drains flush_observers, sending () to each
    │
    ▼
Client::flush receives via oneshot::Receiver
```

## Data Flow: Drain

```
Application
    │
    │  client.drain()  or  subscriber.drain()
    │
    ▼
Client::drain / Subscriber::drain
    │  sends Command::Drain { sid: None } (whole client)
    │  or Command::Drain { sid: Some(n) } (single subscription)
    │
    ▼
ConnectionHandler::handle_command(Command::Drain)
    │  if sid is Some:
    │     pushes sid to drain_pings
    │     sends ClientOp::Unsubscribe { sid, max: None }
    │  if sid is None (whole client):
    │     sets is_draining = true
    │     emits Event::Draining
    │     for each subscription: drain_pings.push(sid), Unsubscribe
    │  sends ClientOp::Ping (to flush the UNSUB messages)
    │
    ▼
ProcessFut::poll (main loop)
    │  processes any remaining server messages
    │  removes drained subscriptions from HashMap
    │  if is_draining: returns ExitReason::Closed
    │
    ▼
ConnectionHandler exits, emits Event::Closed
```

## Main Processing Loop

The `ConnectionHandler::process` method implements the core event loop via a custom `Future` (`ProcessFut`):

```rust
impl Future for ProcessFut<'_> {
    type Output = ExitReason;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // 1. Check ping interval — send PING if due, disconnect if too many pending
        while self.handler.ping_interval.poll_tick(cx).is_ready() {
            if let Poll::Ready(exit) = self.ping() { return Poll::Ready(exit); }
        }

        // 2. Read all available server operations
        loop {
            match self.handler.connection.poll_read_op(cx) {
                Poll::Pending => break,
                Poll::Ready(Ok(Some(server_op))) => self.handler.handle_server_op(server_op),
                Poll::Ready(Ok(None)) => return Poll::Ready(ExitReason::Disconnected(None)),
                Poll::Ready(Err(err)) => return Poll::Ready(ExitReason::Disconnected(Some(err))),
            }
        }

        // 3. Clean up drained subscriptions
        while let Some(sid) = self.handler.drain_pings.pop_front() {
            self.handler.subscriptions.remove(&sid);
        }

        // 4. If draining, exit
        if self.handler.is_draining { return Poll::Ready(ExitReason::Closed); }

        // 5. Process client commands (batch of up to 16)
        //    while write buffer not full
        loop {
            while !self.handler.connection.is_write_buf_full() {
                match receiver.poll_recv_many(cx, recv_buf, 16) {
                    Poll::Pending => break,
                    Poll::Ready(1..) => { for cmd in recv_buf.drain(..) { handler.handle_command(cmd); } }
                    Poll::Ready(0) => return Poll::Ready(ExitReason::Closed),
                }
            }

            // 6. Write pending data to stream
            match self.handler.connection.poll_write(cx) {
                Poll::Pending => break,
                Poll::Ready(Ok(())) => continue,  // write buffer empty, try more commands
                Poll::Ready(Err(err)) => return Poll::Ready(ExitReason::Disconnected(Some(err))),
            }
        }

        // 7. Flush stream and notify observers
        match self.handler.connection.poll_flush(cx) { ... }

        // 8. Check for forced reconnect
        if mem::take(&mut self.handler.should_reconnect) {
            return Poll::Ready(ExitReason::ReconnectRequested);
        }

        Poll::Pending
    }
}
```

### Exit Reasons

The main loop exits for three reasons:

| Reason | Action |
|--------|--------|
| `Disconnected(Option<io::Error>)` | Attempt reconnection via `handle_disconnect()` |
| `ReconnectRequested` | Force reconnect (user-triggered) |
| `Closed` | Connection handler terminates, emit `Event::Closed` |

On disconnection, `handle_disconnect()` is called which:
1. Resets `pending_pings` to 0
2. Emits `Event::Disconnected`
3. Updates connection state to `Disconnected`
4. Calls `handle_reconnect()` which uses `Connector::connect()`
5. On successful reconnect, re-subscribes all active subscriptions
6. Re-subscribes the multiplexer wildcard if present

## Slow Consumer Handling

When a subscription's `mpsc::Sender` channel is full (the application isn't consuming messages fast enough):

1. `try_send` returns `TrySendError::Full`
2. The `ConnectionHandler` emits `Event::SlowConsumer(sid)`
3. The message is **dropped** (not queued)
4. The subscription remains active

When a subscription's receiver is dropped (application closed the stream):

1. `try_send` returns `TrySendError::Closed`
2. The subscription is removed from the HashMap
3. An `UNSUB` command is sent to the server

## Ping/Pong Health Check

The `ConnectionHandler` maintains a periodic PING interval (default 60 seconds):

1. `ping_interval` fires every N seconds
2. A `ClientOp::Ping` is enqueued
3. `pending_pings` counter increments
4. If `pending_pings > MAX_PENDING_PINGS (2)`, the connection is considered dead
5. When `ServerOp::Pong` is received, `pending_pings` decrements
6. Any server operation resets the ping interval timer

## Batched Command Processing

Commands from the `Client` are received in batches of up to 16 (`RECV_CHUNK_SIZE`) using `poll_recv_many`. This amortizes the cost of waking the task and enables pipelining multiple operations (e.g., publishing many messages) in a single poll cycle.