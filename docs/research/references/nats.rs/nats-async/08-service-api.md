# async-nats: Service API

## Overview

The Service API provides a microservice request/reply pattern with built-in service discovery, health checking, and statistics. It follows the [NATS Micro v1 specification](https://github.com/nats-io/nats-architecture-design/blob/main/adr/ADR-33.md).

The `service` feature is required.

## Service

```rust
#[derive(Debug)]
pub struct Service {
    endpoints_state: Arc<Mutex<Endpoints>>,
    info: Info,
    client: Client,
    handle: JoinHandle<Result<(), Error>>,
    shutdown_tx: Sender<()>,
    subjects: Arc<Mutex<Vec<String>>>,
    queue_group: String,
}
```

## Creating a Service

Via the `ServiceExt` trait on `Client`:

```rust
use async_nats::service::ServiceExt;

// Builder pattern
let mut service = client
    .service_builder()
    .description("product service")
    .stats_handler(|endpoint, stats| serde_json::json!({ "endpoint": endpoint }))
    .metadata(HashMap::from([("version".into(), "v2".into())]))
    .queue_group("products-group")
    .start("products", "1.0.0")
    .await?;

// Direct config
let mut service = client
    .add_service(service::Config {
        name: "products".to_string(),
        version: "1.0.0".to_string(),
        description: Some("product service".to_string()),
        stats_handler: None,
        metadata: None,
        queue_group: None,
    })
    .await?;
```

Service name must match `^[A-Za-z0-9\-_]+$`. Version must be valid SemVer.

## Service Verbs

Every service automatically subscribes to three verb subjects for discovery and monitoring:

| Verb | Subject Pattern | Purpose |
|------|----------------|---------|
| PING | `$SRV.PING`, `$SRV.PING.<name>`, `$SRV.PING.<name>.<id>` | Lightweight health check |
| INFO | `$SRV.INFO.<name>`, `$SRV.INFO.<name>.<id>` | Service metadata |
| STATS | `$SRV.STATS.<name>`, `$SRV.STATS.<name>.<id>` | Service + endpoint statistics |

A background task handles these verb requests and responds with JSON payloads.

## Service Config

```rust
#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    pub name: String,
    pub description: Option<String>,
    pub version: String,
    pub stats_handler: Option<StatsHandler>,
    pub metadata: Option<HashMap<String, String>>,
    pub queue_group: Option<String>,
}
```

## Adding Endpoints

```rust
// Simple endpoint
let mut endpoint = service.endpoint("get-products").await?;

// Endpoint with custom name and metadata
let endpoint = service
    .endpoint_builder()
    .name("api")
    .metadata(HashMap::from([("auth".into(), "required".into())]))
    .queue_group("custom-group")
    .add("products")
    .await?;

// Grouped endpoints
let v1 = service.group("v1");
let products = v1.endpoint("products").await?;
let orders = v1.endpoint("orders").await?;

// Nested groups
let v1_api = service.group("api").group("v1");
```

## Endpoint

```rust
pub struct Endpoint {
    requests: Subscriber,
    stats: Arc<Mutex<Endpoints>>,
    client: Client,
    endpoint: String,
    shutdown: Option<ShutdownRx>,
    shutdown_future: Option<ShutdownReceiverFuture>,
}
```

Implements `futures_util::Stream<Item = Request>`.

```rust
while let Some(request) = endpoint.next().await {
    request.respond(Ok("response data".into())).await?;
}
```

## Service Request

```rust
#[derive(Debug)]
pub struct Request {
    issued: Instant,
    client: Client,
    pub message: Message,
    endpoint: String,
    stats: Arc<Mutex<Endpoints>>,
}
```

### Responding

```rust
// Success
request.respond(Ok("result".into())).await?;

// Success with headers
request.respond_with_headers(Ok("result".into()), headers).await?;

// Error
request.respond(Err(service::error::Error {
    code: 500,
    status: "internal error".to_string(),
})).await?;
```

Error responses always include `Nats-Service-Error` and `Nats-Service-Error-Code` headers. If user-supplied headers contain these headers, they are overridden by the error values.

### Stats Tracking

Each response updates endpoint statistics:
- `requests` — total requests
- `processing_time` — cumulative processing time
- `average_processing_time` — average per request
- `errors` — error count
- `last_error` — last error details

## Service Info Types

### PingResponse

```rust
pub struct PingResponse {
    pub kind: String,    // "io.nats.micro.v1.ping_response"
    pub name: String,
    pub id: String,
    pub version: String,
    pub metadata: HashMap<String, String>,
}
```

### Info

```rust
pub struct Info {
    pub kind: String,    // "io.nats.micro.v1.info_response"
    pub name: String,
    pub id: String,
    pub description: String,
    pub version: String,
    pub metadata: HashMap<String, String>,
    pub endpoints: Vec<endpoint::Info>,
}
```

### Stats

```rust
pub struct Stats {
    pub kind: String,    // "io.nats.micro.v1.stats_response"
    pub name: String,
    pub id: String,
    pub version: String,
    pub started: DateTime,
    pub endpoints: Vec<endpoint::Stats>,
}
```

### Endpoint Stats

```rust
pub struct endpoint::Stats {
    pub name: String,
    pub subject: String,
    pub queue_group: String,
    pub data: Option<serde_json::Value>,  // Custom data from stats_handler
    pub errors: u64,
    pub processing_time: Duration,
    pub average_processing_time: Duration,
    pub requests: u64,
    pub last_error: Option<error::Error>,
}
```

## Service Groups

Groups provide subject prefixing for endpoint organization:

```rust
let service = client.service_builder().start("api", "1.0.0").await?;

// Endpoints subscribe to "products" and "orders"
let products = service.endpoint("products").await?;
let orders = service.endpoint("orders").await?;

// Grouped: subscribe to "v1.products" and "v1.orders"
let v1 = service.group("v1");
let products = v1.endpoint("products").await?;
let orders = v1.endpoint("orders").await?;

// Nested: subscribe to "api.v1.products"
let api_v1 = service.group("api").group("v1");
let products = api_v1.endpoint("products").await?;
```

Each group can have its own queue group:

```rust
let v1 = service.group_with_queue_group("v1", "v1-workers");
```

## Stopping a Service

```rust
service.stop().await?;
```

Sends a shutdown signal and aborts the verb-handling task. Other service instances with the same name continue running.

## Resetting Stats

```rust
service.reset().await?;
```

Resets all endpoint statistics (errors, processing time, requests, average processing time) to zero.

## Querying Service State

```rust
let stats: HashMap<String, endpoint::Stats> = service.stats().await?;
let info: Info = service.info().await?;
```