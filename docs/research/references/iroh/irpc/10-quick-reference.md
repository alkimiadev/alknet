# irpc: Quick Reference

## Crate Info

- **Name:** `irpc`
- **Version:** 0.13.0
- **License:** Apache-2.0 OR MIT
- **Repository:** https://github.com/n0-computer/irpc
- **MSRV:** 1.89

## Feature Flags

| Feature | Default | Dependencies Added |
|---|---|---|
| `rpc` | ✅ | noq, postcard, smallvec, tracing, tokio/io-util |
| `derive` | ✅ | irpc-derive |
| `spans` | ✅ | tracing |
| `stream` | ✅ | futures-util |
| `noq_endpoint_setup` | ✅ | rustls, rcgen, futures-buffered |
| `varint-util` | ❌ | postcard, smallvec, tokio/io-util |

## Type Quick Reference

### Core Types

```
Service          trait — implemented on protocol enum, defines Message type
Channels<S>      trait — implemented on request types, defines Tx/Rx types
RpcMessage       trait — blanket impl for Debug+Serialize+DeserializeOwned+Send+Sync+Unpin+'static
Sender           trait — sealed marker for sender types
Receiver         trait — sealed marker for receiver types
WithChannels<I,S> struct — wraps request I with tx/rx/span for service S
Client<S>        struct — client to service S (local or remote)
LocalSender<S>   struct — local sender wrapping mpsc::Sender<S::Message>
Request<L,R>     enum  — Local(L) or Remote(R) request
RemoteSender<S>  struct — holds QUIC stream pair for sending initial message
```

### Channel Types

```
oneshot::Sender<T>    — Tokio or Boxed; single value; async send
oneshot::Receiver<T>  — Tokio or Boxed; single value; Future impl
mpsc::Sender<T>       — Tokio or Arc<DynSender>; stream; async send/try_send
mpsc::Receiver<T>    — Tokio or Box<DynReceiver>; stream; async recv
NoSender              — No-op sender
NoReceiver            — No-op receiver
```

### Remote Types (rpc feature)

```
RemoteConnection     trait — open_bi(), zero_rtt_accepted(), clone_boxed()
NoqLazyRemoteConnection  — lazy noq connection with cache
Handler<R>           type  — Arc<dyn Fn(R, RecvStream, SendStream) -> ...>
```

### irpc-iroh Types

```
IrohRemoteConnection       — wraps iroh::Connection
IrohZrttRemoteConnection   — wraps iroh::OutgoingZeroRttConnection
IrohLazyRemoteConnection   — lazy iroh connection with cache
IrohProtocol<R>            — ProtocolHandler for iroh Router
Iroh0RttProtocol<R>        — ProtocolHandler with 0-RTT support
IncomingRemoteConnection   trait — abstraction over Connection and ZeroRttConnection
```

## Interaction Patterns Cheatsheet

```rust
// ═══════════════════════════════════════════
// Protocol Definition
// ═══════════════════════════════════════════

#[rpc_requests(message = MyMessage)]
#[derive(Debug, Serialize, Deserialize)]
enum MyProtocol {
    // Unary RPC
    #[rpc(tx=oneshot::Sender<Response>)]
    #[wrap(GetReq)]
    Get(String),

    // Server streaming
    #[rpc(tx=mpsc::Sender<Item>)]
    #[wrap(ListReq)]
    List(ListParams),

    // Client streaming
    #[rpc(tx=oneshot::Sender<Count>, rx=mpsc::Receiver<Item>)]
    #[wrap(UploadReq)]
    Upload,

    // Bidirectional streaming
    #[rpc(tx=mpsc::Sender<Result>, rx=mpsc::Receiver<Update>)]
    #[wrap(ProcessReq)]
    Process(ProcessConfig),

    // Fire and forget
    #[rpc]
    #[wrap(LogReq)]
    Log(String),
}

// ═══════════════════════════════════════════
// Client Usage
// ═══════════════════════════════════════════

// Local
let (tx, rx) = tokio::sync::mpsc::channel(16);
tokio::task::spawn(actor(rx));
let client: Client<MyProtocol> = Client::local(tx);

// Remote (noq)
let client: Client<MyProtocol> = Client::noq(endpoint, addr);

// Remote (iroh)
let client: Client<MyProtocol> = irpc_iroh::client(endpoint, addr, alpn);

// ═══════════════════════════════════════════
// Making Requests
// ═══════════════════════════════════════════

// Unary
let result: Response = client.rpc(GetReq("key".into())).await?;

// Server streaming
let mut rx: mpsc::Receiver<Item> = client.server_streaming(ListReq(params), 16).await?;
while let Some(item) = rx.recv().await? { ... }

// Client streaming
let (update_tx, response_rx): (mpsc::Sender<Item>, oneshot::Receiver<Count>) =
    client.client_streaming(Upload, 4).await?;
update_tx.send(item).await?;
let count = response_rx.await?;

// Bidirectional
let (update_tx, mut result_rx): (mpsc::Sender<Update>, mpsc::Receiver<Result>) =
    client.bidi_streaming(ProcessReq(config), 4, 16).await?;
update_tx.send(update).await?;
while let Some(result) = result_rx.recv().await? { ... }

// Fire and forget
client.notify(LogReq("message".into())).await?;

// ═══════════════════════════════════════════
// Server Setup
// ═══════════════════════════════════════════

// noq
let handler = MyProtocol::remote_handler(local_sender);
irpc::rpc::listen(endpoint, handler).await;

// iroh
let protocol = IrohProtocol::with_sender(local_sender);
Router::builder(endpoint).accept(ALPN, protocol).spawn();

// ═══════════════════════════════════════════
// Actor Message Handling
// ═══════════════════════════════════════════

async fn handle(&mut self, msg: MyMessage) {
    match msg {
        MyMessage::Get(wc) => {
            let WithChannels { inner, tx, .. } = wc;
            let result = self.db.get(&inner.0).cloned();
            tx.send(result).await.ok();
        }
        MyMessage::List(wc) => {
            let WithChannels { tx, .. } = wc;
            for item in &self.items {
                if tx.send(item.clone()).await.is_err() { break; }
            }
        }
        MyMessage::Upload(wc) => {
            let WithChannels { tx, mut rx, .. } = wc;
            let mut count = 0;
            while let Ok(Some(item)) = rx.recv().await {
                self.process(item);
                count += 1;
            }
            tx.send(count).await.ok();
        }
        MyMessage::Process(wc) => {
            let WithChannels { tx, mut rx, inner, .. } = wc;
            tokio::task::spawn(async move {
                while let Ok(Some(update)) = rx.recv().await {
                    if let Some(result) = process(update, &inner) {
                        if tx.send(result).await.is_err() { break; }
                    }
                }
            });
        }
        MyMessage::Log(wc) => {
            let WithChannels { inner, .. } = wc;
            println!("{}", inner.0);
        }
    }
}
```

## Error Handling Quick Reference

```rust
// Client-side errors
use irpc::{Error, RequestError, Result};

// Request errors (connection/stream open failures)
match client.rpc(GetReq("key".into())).await {
    Ok(result) => { ... }
    Err(Error::Request { source }) => { ... }  // Connection failed
    Err(Error::OneshotRecv { source }) => { ... }  // Response channel error
}

// Channel errors
use irpc::channel::{SendError, mpsc::RecvError, oneshot::RecvError};

// SendError: ReceiverClosed | MaxMessageSizeExceeded | Io
// RecvError (oneshot): SenderClosed | MaxMessageSizeExceeded | Io
// RecvError (mpsc): MaxMessageSizeExceeded | Io
```

## Constants

```rust
pub const MAX_MESSAGE_SIZE: u64 = 16 * 1024 * 1024;  // 16 MiB
pub const ERROR_CODE_MAX_MESSAGE_SIZE_EXCEEDED: u32 = 1;
pub const ERROR_CODE_INVALID_POSTCARD: u32 = 2;
// Connection close code 0 = clean shutdown
```