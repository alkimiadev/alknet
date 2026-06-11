# Service API and Higher-Level Abstractions

This document covers the Service API and other higher-level abstractions built on top of the core NATS client.

## Service API

**Location**: `service/` (feature: `service`)

The Service API provides a framework for building NATS-based microservices with built-in monitoring, health checks, and statistics.

### Service

```rust
#[derive(Debug)]
pub struct Service {
    client: Client,
    info: Info,
    endpoints: HashMap<String, Endpoint>,
    started: DateTime,
    stats_handler: Arc<dyn Fn(&str, &Stats) -> serde_json::Value + Send + Sync>,
    stop_sender: mpsc::Sender<()>,
    stop_receiver: Option<mpsc::Receiver<()>>,
}
```

### Creating a Service

```rust
use async_nats::service::ServiceExt;

let mut service = client
    .service_builder()
    .description("Product service")
    .stats_handler(|endpoint, stats| {
        serde_json::json!({
            "endpoint": endpoint,
            "requests": stats.num_requests,
            "errors": stats.num_errors,
        })
    })
    .start("products", "1.0.0")
    .await?;
```

### ServiceBuilder

```rust
impl ServiceBuilder {
    pub fn description(mut self, description: impl Into<String>) -> Self
    pub fn stats_handler<F>(mut self, handler: F) -> Self
    pub async fn start(self, name: impl Into<String>, version: impl Into<String>) -> Result<Service, ServiceError>
}
```

### Endpoints

A service exposes one or more endpoints, each handling requests on a specific subject:

```rust
// Add an endpoint
let mut endpoint = service
    .endpoint("get_product")
    .await?;

// Process requests
while let Some(request) = endpoint.next().await {
    let request = request?;
    // Handle the request
    request.respond(serde_json::json!({ "id": 1, "name": "Widget" })).await?;
}
```

### Endpoint

**Location**: `service/endpoint.rs`

```rust
pub struct Endpoint {
    subject: Subject,
    queue_group: Option<String>,
    info: EndpointInfo,
    stats: Stats,
    subscriber: Subscriber,
}
```

Implements `futures::Stream` yielding `ServiceRequest` objects.

### ServiceRequest

```rust
pub struct ServiceRequest {
    pub subject: Subject,
    pub payload: Bytes,
    pub headers: Option<HeaderMap>,
    pub reply: Option<Subject>,
    pub client: Client,
}
```

Methods:
- `respond(payload)` — send a response to the requester
- `respond_with_headers(payload, headers)` — send a response with headers

### Monitoring Subjects

The Service API automatically creates monitoring endpoints:

| Subject | Description |
|---------|-------------|
| `$SRV.PING` | Ping all services (returns service info) |
| `$SRV.PING.<name>` | Ping specific service by name |
| `$SRV.PING.<name>.<id>` | Ping specific service instance |
| `$SRV.INFO` | Get service info |
| `$SRV.STATS` | Get service statistics |

### Service Info

```rust
pub struct Info {
    pub name: String,
    pub id: String,
    pub version: String,
    pub description: String,
    pub endpoints: Vec<EndpointInfo>,
}
```

### Stats

```rust
pub struct Stats {
    pub num_requests: u64,
    pub num_errors: u64,
    pub last_error: Option<String>,
    pub processing_time: Duration,
    pub average_processing_time: Duration,
}
```

## ID Generation

**Location**: `id_generator.rs`

The client needs unique IDs for inbox subjects and other purposes.

### With `nuid` Feature (Default)

Uses the NUID library for high-performance, cryptographically strong, collision-resistant IDs:

```rust
pub(crate) fn next() -> String {
    nuid::next().to_string()
}
```

NUID generates 22-character alphanumeric strings using a combination of a random prefix and a sequential counter.

### Without `nuid` Feature

Falls back to `rand`-based generation:

```rust
pub(crate) fn next() -> String {
    rng()
        .sample_iter(Alphanumeric)
        .take(22)
        .map(char::from)
        .collect()
}
```

Both approaches produce 22-character alphanumeric strings, but NUID is more performant and has better collision resistance.

## Inbox Generation

The `Client::new_inbox()` method generates globally unique inbox subjects for request-reply:

```rust
pub fn new_inbox(&self) -> String {
    format!("{}.{}", self.inbox_prefix, crate::id_generator::next())
}
```

Default prefix is `_INBOX`, producing subjects like `_INBOX.UaBG3f3q5NxX3KdNcRmF2f`.

Custom prefix via `ConnectOptions::custom_inbox_prefix()`:
```rust
let client = ConnectOptions::new()
    .custom_inbox_prefix("MYAPP")
    .connect("demo.nats.io")
    .await?;
// Inbox subjects: MYAPP.UaBG3f3q5KdNcRmF2f
```

## DateTime Helpers

**Location**: `datetime.rs` (feature: `jetstream` or `service` or `chrono`)

Provides date/time types for JetStream and Service API timestamps:

- Uses the `time` crate by default
- Optionally uses `chrono` via the `chrono` feature flag
- Supports RFC 3339 formatting and parsing
- `DateTime` type wraps either `time::OffsetDateTime` or `chrono::DateTime<Utc>`

## Crypto Module

**Location**: `crypto.rs` (feature: `crypto`)

Provides encryption/decryption support used by the Object Store for server-side encryption.

## Subject Validation

**Location**: `lib.rs`

The client provides two levels of subject validation:

### is_valid_publish_subject

```rust
pub(crate) fn is_valid_publish_subject<T: AsRef<str>>(subject: T) -> bool
```

Checks for protocol safety only:
- Not empty
- No whitespace (space, tab, CR, LF) which would break protocol framing

Used for publish operations. Can be disabled with `skip_subject_validation`.

### is_valid_subject

```rust
pub(crate) fn is_valid_subject<T: AsRef<str>>(subject: T) -> bool
```

Checks structural validity:
- Not empty
- No leading/trailing dots
- No consecutive dots (`..`)
- No whitespace

Used for subscribe operations (always runs, matching Go/Java behavior).

### is_valid_queue_group

```rust
pub(crate) fn is_valid_queue_group(queue_group: &str) -> bool
```

Checks:
- Not empty
- No whitespace

## JetStream Name Validation

**Location**: `jetstream/mod.rs`

```rust
pub(crate) fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|c| !c.is_ascii_whitespace() && c != b'.' && c != b'*' && c != b'>')
}
```

JetStream names (stream names, consumer names) must not contain:
- Whitespace
- Dots (`.`) — would conflict with subject delimiters
- Wildcards (`*`, `>`) — would conflict with subject wildcards

## CallbackArg1

**Location**: `options.rs`

A type-erased async callback wrapper used throughout the crate:

```rust
pub(crate) type AsyncCallbackArg1<A, T> =
    Arc<dyn Fn(A) -> Pin<Box<dyn Future<Output = T> + Send + Sync + 'static>> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct CallbackArg1<A, T>(AsyncCallbackArg1<A, T>);

impl<A, T> CallbackArg1<A, T> {
    pub(crate) async fn call(&self, arg: A) -> T {
        (self.0.as_ref())(arg).await
    }
}
```

Used for:
- `event_callback` — `CallbackArg1<Event, ()>`
- `auth_callback` — `CallbackArg1<Vec<u8>, Result<Auth, AuthError>>`
- `reconnect_to_server_callback` — `CallbackArg1<(Vec<Server>, ServerInfo), Option<ReconnectToServer>>`
- `signature_callback` — `CallbackArg1<String, Result<String, AuthError>>`

## Version Compatibility Checking

The `Client::is_server_compatible` method checks if the server version meets a minimum requirement:

```rust
pub fn is_server_compatible(&self, major: i64, minor: i64, patch: i64) -> bool
```

This parses the server version string from `ServerInfo::version` using a regex and compares major/minor/patch components. Note: this checks the directly-connected server, not necessarily the JetStream leader.

The `server_2_10`, `server_2_11`, `server_2_12`, and `server_2_14` feature flags enable version-specific API fields and methods without runtime checks.