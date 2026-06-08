# RustFS Event Notification System & S3 Select Reference

> **Companion document**: This extends [rustfs-reference.md](./rustfs-reference.md) which covers auth, architecture, and credential mapping. This document focuses on the **event notification system** and **S3 Select** feature.

**Date**: 2026-06-08  
**RustFS version**: Based on source at `/workspace/rustfs/` (commit-level snapshot)  
**Purpose**: Evaluate rustfs event notification and S3 Select for alknet integration

---

## Table of Contents

1. [Event Notification System](#1-event-notification-system)
2. [Event Types & Structure](#2-event-types--structure)
3. [Notification Targets](#3-notification-targets)
4. [Configuration & Rule Engine](#4-configuration--rule-engine)
5. [Pipeline & Delivery](#5-pipeline--delivery)
6. [Live Event Stream](#6-live-event-stream)
7. [S3 Select](#7-s3-select)
8. [Mapping to alknet](#8-mapping-to-alknet)
9. [References](#9-references)

---

## 1. Event Notification System

### 1.1 Architecture Overview

RustFS implements a full S3-compatible bucket notification system. The architecture follows a layered pattern:

```
┌──────────────────────────────────────────────────────────┐
│                    S3 API Layer                           │
│  (PutObject, DeleteObject, CopyObject, etc.)              │
└─────────────┬────────────────────────────────────────────┘
              │ emits EventArgs
              ▼
┌──────────────────────────────────────────────────────────┐
│              ECStore (event_notification.rs)               │
│  - send_event() hook (global OnceLock dispatch)           │
│  - registers dispatch callback during init                │
└─────────────┬────────────────────────────────────────────┘
              │ converts EventArgs → NotifyEventArgs
              ▼
┌──────────────────────────────────────────────────────────┐
│          rustfs_notify (NotificationSystem)              │
│  ┌──────────────┐   ┌──────────────┐  ┌───────────────┐ │
│  │ NotifyPipeline│──▶│ NotifyRuleEngine│─▶│ EventNotifier │ │
│  │  (broadcast   │   │  (match rules)  │  │ (send to     │ │
│  │   + history)  │   │                  │  │  targets)     │ │
│  └──────────────┘   └──────────────┘  └──────┬────────┘ │
│                                               │          │
│  ┌──────────────┐   ┌──────────────┐  ┌──────▼────────┐ │
│  │BucketConfigM │   │ NotifyConfigM │  │  TargetList    │ │
│  │  anager       │   │  anager        │  │  (Webhook,    │ │
│  └──────────────┘   └──────────────┘  │  Kafka, AMQP,  │ │
│                                        │  NATS, Redis,  │ │
│                                        │  MQTT, MySQL,  │ │
│                                        │  Postgres,     │ │
│                                        │  Pulsar)       │ │
│                                        └───────────────┘ │
└──────────────────────────────────────────────────────────┘
```

### 1.2 Key Crates

| Crate | Purpose |
|-------|---------|
| `rustfs_notify` | Core notification orchestration: `Event`, `EventArgs`, `EventNotifier`, `NotifyPipeline`, `NotificationSystem`, rule engine, bucket config management |
| `rustfs_targets` | Target implementations (Webhook, Kafka, AMQP, NATS, Redis, MQTT, MySQL, PostgreSQL, Pulsar) + `Target` trait, `QueueStore`, TLS hot-reload |
| `rustfs_s3_types` | `EventName` enum with all S3 event type definitions, serialization, mask/bitfield support |
| `rustfs_ecstore` | Storage layer; `event_notification.rs` provides the dispatch hook that bridges ecstore events to the notify system |
| `rustfs_config` | Configuration for each target type (Env vars, KVS parsing, subsystem names) |

### 1.3 Initialization Flow

1. `rustfs/server/event.rs::init_event_notifier()` runs at startup
2. If notify module is enabled (`RUSTFS_NOTIFY_ENABLE=true`), it calls `rustfs_notify::initialize(config)` which:
   - Creates a `NotificationSystem` with `EventNotifier`, `TargetRegistry`, and config
   - Loads all target configurations from the config store
   - Initializes each target (connects, health-checks, starts stream replay workers)
3. An ECStore dispatch hook is installed via `register_event_dispatch_hook()` which:
   - Converts `ecstore::EventArgs` → `notify::EventArgs`
   - Parses `EventName` from string
   - Spawns an async task to call `notifier_global::notify(args)`

### 1.4 Module Toggle

The notification system respects a module enable/disable flag:
- Environment variable: `RUSTFS_NOTIFY_ENABLE` (default: `DEFAULT_NOTIFY_ENABLE`)
- When disabled, only the **live event stream** is initialized (no targets are loaded)
- This allows in-process event subscription without external delivery

---

## 2. Event Types & Structure

### 2.1 EventName Enum

Defined in `rustfs_s3_types::EventName`. All S3-standard event types plus RustFS extensions:

| Category | Events |
|----------|--------|
| **ObjectAccessed** | `s3:ObjectAccessed:Get`, `s3:ObjectAccessed:Head`, `s3:ObjectAccessed:GetRetention`, `s3:ObjectAccessed:GetLegalHold`, `s3:ObjectAccessed:Attributes` |
| **ObjectCreated** | `s3:ObjectCreated:Put`, `s3:ObjectCreated:Post`, `s3:ObjectCreated:Copy`, `s3:ObjectCreated:CompleteMultipartUpload`, `s3:ObjectCreated:PutRetention`, `s3:ObjectCreated:PutLegalHold` |
| **ObjectRemoved** | `s3:ObjectRemoved:Delete`, `s3:ObjectRemoved:DeleteMarkerCreated`, `s3:ObjectRemoved:DeleteAllVersions`, `s3:ObjectRemoved:NoOP` |
| **ObjectTagging** | `s3:ObjectTagging:Put`, `s3:ObjectTagging:Delete` |
| **ObjectAcl** | `s3:ObjectAcl:Put` |
| **ObjectReplication** | `s3:Replication:OperationFailedReplication`, `s3:Replication:OperationCompletedReplication`, `s3:Replication:OperationMissedThreshold`, `s3:Replication:OperationReplicatedAfterThreshold`, `s3:Replication:OperationNotTracked` |
| **ObjectRestore** | `s3:ObjectRestore:Post`, `s3:ObjectRestore:Completed` |
| **ObjectTransition** | `s3:ObjectTransition:Failed`, `s3:ObjectTransition:Complete` |
| **Lifecycle** | `s3:LifecycleExpiration:Delete`, `s3:LifecycleExpiration:DeleteMarkerCreated`, `s3:LifecycleDelMarkerExpiration:Delete`, `s3:LifecycleTransition` |
| **Bucket** | `s3:BucketCreated:*`, `s3:BucketRemoved:*` |
| **Scanner** | `s3:Scanner:ManyVersions`, `s3:Scanner:LargeVersions`, `s3:Scanner:BigPrefix` |
| **IntelligentTiering** | `s3:IntelligentTiering` |
| **Compound (wildcard)** | `s3:ObjectAccessed:*`, `s3:ObjectCreated:*`, `s3:ObjectRemoved:*`, `s3:ObjectTagging:*`, `s3:Replication:*`, `s3:ObjectRestore:*`, `s3:LifecycleExpiration:*`, `s3:ObjectTransition:*`, `s3:Scanner:*`, `Everything` |
| **Internal** | `ObjectRemovedAbortMultipartUpload`, `ObjectCreatedCreateMultipartUpload`, `ObjectRemovedDeleteObjects` |

### 2.2 Event Schema Versioning

The `event_schema_version` function returns different versions based on event type:

| Version | Events |
|---------|--------|
| `2.1` | ObjectCreated/Removed/Accessed base events |
| `2.2` | Replication events |
| `2.3` | Tagging, ACL, Restore, Lifecycle, IntelligentTiering events |

### 2.3 Event Record Structure (`rustfs_notify::Event`)

```rust
pub struct Event {
    pub event_version: String,        // e.g., "2.1", "2.2", "2.3"
    pub event_source: String,         // "rustfs:s3"
    pub aws_region: String,
    pub event_time: DateTime<Utc>,
    pub event_name: EventName,
    pub user_identity: Identity,      // { principal_id: String }
    pub request_parameters: HashMap<String, String>,
    pub response_elements: HashMap<String, String>,
    pub s3: Metadata,                 // See below
    pub glacier_event_data: Option<GlacierEventData>,
    pub source: Source,               // { host, port, user_agent }
}

pub struct Metadata {
    pub schema_version: String,       // "1.0"
    pub configuration_id: String,
    pub bucket: Bucket,               // { name, owner_identity, arn }
    pub object: Object,              // See below
}

pub struct Object {
    pub key: String,                   // URL-encoded object key
    pub size: Option<i64>,
    pub e_tag: Option<String>,
    pub content_type: Option<String>,
    pub user_metadata: Option<HashMap<String, String>>,
    pub version_id: Option<String>,
    pub sequencer: String,            // Monotonic event sequence ID
}
```

- The `key` field is URL-encoded (form-urlencoded)
- `sequencer` is derived from `ObjectInfo.mod_time` nanosecond timestamp, ensuring ordering
- `user_metadata` filters out keys starting with `x-amz-meta-internal-`
- For removed events, `size`, `e_tag`, `content_type`, and `user_metadata` are omitted

### 2.4 EventArgs Builder

Events are constructed via `EventArgsBuilder`:

```rust
let args = EventArgsBuilder::new(EventName::ObjectCreatedPut, "my-bucket", object_info)
    .host("10.0.0.1")
    .port(9000)
    .user_agent("alknet-storage/1.0")
    .req_param("principalId", "user-123")
    .version_id("v2")
    .build();
let event = Event::new(args);
```

The builder pattern ensures all required fields are provided and allows optional fields.

---

## 3. Notification Targets

### 3.1 Target Trait

All targets implement `rustfs_targets::Target<E>`:

```rust
#[async_trait]
pub trait Target<E>: Send + Sync + 'static
where E: Send + Sync + 'static + Clone + Serialize + DeserializeOwned
{
    fn id(&self) -> TargetID;
    fn name(&self) -> String;
    async fn is_active(&self) -> Result<bool, TargetError>;
    async fn save(&self, event: Arc<EntityTarget<E>>) -> Result<(), TargetError>;
    async fn send_raw_from_store(&self, key: Key, body: Vec<u8>, meta: QueuedPayloadMeta) -> Result<(), TargetError>;
    async fn send_from_store(&self, key: Key) -> Result<(), TargetError>;
    async fn close(&self) -> Result<(), TargetError>;
    fn store(&self) -> Option<&(dyn Store<QueuedPayload, ...>)>;
    fn clone_dyn(&self) -> Box<dyn Target<E> + Send + Sync>;
    async fn init(&self) -> Result<(), TargetError>;
    fn is_enabled(&self) -> bool;
    fn delivery_snapshot(&self) -> TargetDeliverySnapshot;
    fn record_final_failure(&self);
}
```

### 3.2 Supported Targets

| Target | Crate Module | Protocol | Queue Store | TLS/mTLS | SASL | Notes |
|--------|-------------|----------|-------------|----------|------|-------|
| **Webhook** | `targets::webhook` | HTTP POST | Yes (file) | Yes (CA, client cert, skip_verify) | Bearer token | Health check via HEAD to `/`; TLS hot-reload |
| **Kafka** | `targets::kafka` | Kafka Produce | Yes (file) | Yes (CA, client cert) | PLAIN, SCRAM-SHA-256, SCRAM-SHA-512 | Uses `rustfs_kafka_async`; acknowledgments configurable (-1, 0, 1) |
| **AMQP** | `targets::amqp` | AMQP 0-9-1 | Yes (file) | Yes (CA, client cert via amqps://) | Username/password (in URL or config) | Uses `lapin`; publisher confirms; persistent delivery mode |
| **NATS** | `targets::nats` | NATS Publish | Yes (file) | Yes (CA, client cert) | Token, username/password, credentials file | Subject-based routing |
| **Redis** | `targets::redis` | Redis Pub/Sub | Yes (file) | Yes (CA, client cert, insecure) | Password | Channel publish; connection pooling |
| **MQTT** | `targets::mqtt` | MQTT v5 | Yes (file) | Yes (CA, client cert) | Username/password | Uses `rumqttc`; QoS 0/1; WebSocket path allowlist |
| **MySQL** | `targets::mysql` | MySQL INSERT | Yes (file) | Yes (CA, client cert) | Username/password | Namespace or access format; connection pooling |
| **PostgreSQL** | `targets::postgres` | PostgreSQL INSERT/UPSERT | Yes (file) | Yes (CA, client cert) | Username/password (DSN) | Namespace (UPSERT) or access (append) format; `deadpool-postgres` pooling |
| **Pulsar** | `targets::pulsar` | Pulsar Produce | Yes (file) | Yes (CA, client cert) | Token, OAuth2 | Topic-based; persistent or non-persistent |

**Note**: Elasticsearch is listed as a subsystem constant (`notify_elasticsearch`) but marked `#[allow(dead_code)]`, indicating it's planned but not yet implemented.

### 3.3 Target Identification (ARN)

Each target has a `TargetID` (format: `ID:Name`, e.g., `1:webhook`) and an `ARN` (format: `arn:rustfs:sqs:{region}:{id}:{name}`, e.g., `arn:rustfs:sqs:us-east-1:1:webhook`).

Default partition: `rustfs`, default service: `sqs`.

### 3.4 Queue Store (Persistent Delivery)

Targets that have a `queue_dir` configured use a persistent store for at-least-once delivery:

- Events are first persisted to the queue store, then sent
- If the target is unreachable, events remain in the store and are replayed when connectivity recovers
- Queue store format: `RQP1` magic + metadata length (LE u32) + JSON metadata + raw body
- `QueuedPayload` structure includes: event_name, bucket_name, object_name, content_type, queued_at_unix_ms, payload_len
- Extension: `notify_store` (`.nqs`) for notification events, `audit_store` for audit logs

### 3.5 Delivery Payload Format (`TargetLog`)

```rust
// Serialized as JSON when delivering to targets
struct TargetLog {
    event_name: EventName,
    key: String,        // "{bucket}/{decoded_object_name}"
    records: Vec<E>,    // For AMQP/NATS: includes full EntityTarget records
                         // For others: includes serialized Event data
}
```

For AMQP and NATS targets, `build_queued_payload_with_records()` is used, which includes cloned `EntityTarget` records. For other targets, `build_queued_payload()` serializes just the event data.

### 3.6 Concurrency Controls

| Parameter | Default | Env Var |
|-----------|---------|---------|
| Target stream concurrency | 20 | `RUSTFS_NOTIFY_TARGET_STREAM_CONCURRENCY` |
| Send concurrency (inflight limit) | 64 | `RUSTFS_NOTIFY_SEND_CONCURRENCY` |

### 3.7 TLS Hot-Reload

All targets that support TLS (webhook, Kafka, AMQP, NATS, MySQL, PostgreSQL, MQTT) implement `ReloadableTargetTls`:

- A background coordinator polls TLS files for changes
- When fingerprint changes are detected, new material (HTTP client, producer, connection) is built
- Applied via `apply_tls_material()` without requiring a restart
- Supports CA certificates, client certificates, and client keys

---

## 4. Configuration & Rule Engine

### 4.1 Bucket Notification Configuration (XML)

Configuration follows the S3 `NotificationConfiguration` XML schema:

```xml
<NotificationConfiguration xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
  <QueueConfiguration>
    <Id>my-notification</Id>
    <Queue>arn:rustfs:sqs:us-east-1:1:webhook</Queue>
    <Event>s3:ObjectCreated:*</Event>
    <Event>s3:ObjectRemoved:Delete</Event>
    <Filter>
      <S3Key>
        <FilterRule>
          <Name>prefix</Name>
          <Value>uploads/</Value>
        </FilterRule>
        <FilterRule>
          <Name>suffix</Name>
          <Value>.csv</Value>
        </FilterRule>
      </S3Key>
    </Filter>
  </QueueConfiguration>
</NotificationConfiguration>
```

The XML is parsed via `quick_xml` into `NotificationConfiguration` → `QueueConfig` → validated → converted to `BucketNotificationConfig` → `RulesMap`.

Key validation rules:
- Lambda and Topic configurations are **not supported** (return `UnsupportedConfiguration` error)
- Only `QueueConfiguration` is supported (maps to all target types, not just SQS)
- One prefix filter and one suffix filter maximum
- Filter values: ≤1024 chars, no `.` or `..` segments, no `\`, valid UTF-8
- No duplicate event names within a queue config
- ARN must exist in the configured target list

### 4.2 RulesMap

`RulesMap` maps `EventName` → `PatternRules` → `TargetIdSet`:

- Compound events (like `ObjectCreatedAll`) are **expanded** into specific events on insertion
- Pattern matching: prefix/suffix wildcards (e.g., `uploads/*.csv`)
- URL-encoded keys are matched against both encoded and decoded patterns
- Bitmask-based fast path: `total_events_mask` enables O(1) `has_subscriber()` checks

### 4.3 Dynamically Reconfigurable

- `NotificationSystem::set_target_config()` — add/update a target
- `NotificationSystem::remove_target_config()` — remove a target
- `NotificationSystem::load_bucket_notification_config()` — load per-bucket rules
- `NotificationSystem::remove_bucket_notification_config()` — remove per-bucket rules
- `NotificationSystem::reload_config()` — reload from a new `Config` object
- All changes trigger automatic re-initialization of affected targets

---

## 5. Pipeline & Delivery

### 5.1 Event Flow

```
ECStore operation
    ↓
ecstore::event_notification::send_event(EventArgs)
    ↓ (OnceLock dispatch hook)
convert EventArgs → notify::EventArgs
    ↓ spawn
notifier_global::notify(EventArgs)
    ↓
NotificationSystem::send_event(Arc<Event>)
    ↓
NotifyPipeline::send_event()
    ├── LiveEventHistory::record()    (in-memory, last 1024 events)
    ├── broadcast::send()             (tokio broadcast channel, capacity 1024)
    └── EventNotifier::send()       (async, rule-matched delivery)
         ├── RuleEngine::match_targets(bucket, event_name, object_key)
         └── For each matched target:
              ├── EntityTarget construction
              ├── If queue_store: persist then async send
              └── If no queue_store: immediate async send
```

### 5.2 Live Event Stream

The `NotifyPipeline` provides an in-process event stream via `tokio::sync::broadcast`:

```rust
// Subscribe to live events
let rx = system.subscribe_live_events();

// Check if there are live listeners
system.has_live_listeners();

// Get recent events since a sequence number
system.recent_live_events_since(after_sequence, limit) → LiveEventBatch
```

- Broadcast channel capacity: 1024
- `LiveEventHistory` stores last 1024 events with monotonic sequence numbers
- `LiveEventBatch` includes `events: Vec<Arc<Event>>`, `next_sequence: u64`, `truncated: bool`

### 5.3 Metrics

`NotificationMetrics` tracks:
- Processing count (in-flight)
- Processed count (completed)
- Failed count
- Skipped count (no matching targets)

Per-target `TargetDeliverySnapshot`:
- `total_messages`
- `failed_messages`
- `queue_length`

---

## 6. Live Event Stream

### 6.1 In-Process Subscription

The live event stream is useful for alknet because it provides a **push-based** event feed without requiring external message brokers:

```rust
// This can be used from within the same process
let mut rx = notification_system.subscribe_live_events();
while let Ok(event) = rx.recv().await {
    // event: Arc<Event> — full S3 event record
    println!("Event: {} on {}/{}", event.event_name, event.s3.bucket.name, event.s3.object.key);
}
```

### 6.2 Event History Replay

The `LiveEventHistory` supports catch-up subscriptions:

```rust
// Get events since sequence number 42
let batch = system.recent_live_events_since(42, 100).await;
// batch.next_sequence → next sequence to request
// batch.truncated → whether there are more events
// batch.events → Vec<Arc<Event>>
```

---

## 7. S3 Select

### 7.1 Architecture Overview

RustFS implements S3 Select using **Apache DataFusion** as the SQL engine:

```
SelectObjectContentRequest
    ↓ validation (expression type, input/output format, scan range)
    ↓ preflight (get object info, validate SSE headers)
    ↓ create EcObjectStore (DataFusion ObjectStore adapter)
    ↓ get_global_db(input) → QueryDispatcher
    ↓ Query::new(Context, expression) → execute
    ↓ DataFusion SQL parser → logical plan → optimized → physical plan → RecordBatch stream
    ↓ SelectOutputEncoder → CSV or JSON → chunked (128KB) → event stream
```

### 7.2 Key Crates

| Crate | Purpose |
|-------|---------|
| `rustfs_s3select_api` | Query error types, `Context`, `Query`, `QueryResult`, `DatabaseManagerSystem` trait, object store |
| `rustfs_s3select_query` | SQL implementation: parser, analyzer, optimizer, function manager, execution, dispatcher |

### 7.3 SQL Engine

- **Parser**: Custom `RustFsDialect` + `ExtParser` extending DataFusion's SQL parser
- **Supports**: Single SELECT statements only (multi-statement is rejected)
- **Optimizer**: `CascadeOptimizerBuilder` (DataFusion's default rule set)
- **Scheduler**: `LocalScheduler` (single-node execution)
- **Functions**: All of DataFusion's built-in scalar, aggregate, and window functions

### 7.4 Input Formats

| Format | Support | Notes |
|--------|---------|-------|
| **CSV** | ✅ Full | `FileHeaderInfo` (NONE, USE, IGNORE), custom delimiters, quote chars, comment chars, record delimiters |
| **JSON (LINES)** | ✅ Full | NDJSON line-by-line streaming |
| **JSON (DOCUMENT)** | ✅ Limited | Max 128 MiB (OOM guard); no scan range support |
| **Parquet** | ✅ Full | Columnar format |
| **Compression** | ❌ Not supported | Only `NONE` compression currently accepted |

### 7.5 Output Formats

| Format | Options |
|--------|---------|
| **CSV** | Custom field delimiter, quote character, quote escape, record delimiter, quote fields (ALWAYS/ASNEEDED) |
| **JSON** | Line-delimited (NDJSON); custom record delimiter |

### 7.6 Expression Limitations

- Max expression size: 256 KiB (`MAX_SELECT_EXPRESSION_BYTES`)
- Expression type must be `SQL`
- No `AllowQuotedRecordDelimiter` support for CSV
- Scan ranges:
  - CSV: supported
  - JSON LINES: supported
  - JSON DOCUMENT: **not supported**
  - Parquet: supported
  - Range must be valid (start < end, start < object size)

### 7.7 Object Store Integration

`EcObjectStore` implements DataFusion's `ObjectStore` trait, adapting rustfs's ECStore for query execution:
- Handles `GET` with optional byte ranges (scan range)
- JSON DOCUMENT mode: entire file buffered for DOM parsing, then flattened to NDJSON
- JSON sub-path extraction: `FROM s3object.some.path` navigates to the key before flattening
- Respects SSE-C headers for encrypted objects

### 7.8 Streaming Response

Results are streamed as S3 event types:
1. `Cont` event (continuation marker)
2. `Records` events (128KB chunks)
3. `Progress` events (if `RequestProgress.Enabled=true`) — currently only `BytesReturned` populated
4. `Stats` event (final)
5. `End` event

### 7.9 Error Mapping

| QueryError | S3 Error |
|-----------|----------|
| `Parser` | `ParseSelectFailure` (400) |
| `MultiStatement` | `UnsupportedSqlStructure` |
| `NotImplemented` | `NotImplemented` |
| `Datafusion` (scan range) | `InvalidRequestParameter` |
| `Datafusion` (missing binding) | `EvaluatorBindingDoesNotExist` |
| `Datafusion` (other) | `UnsupportedSqlOperation` |
| `StoreError` (bucket not found) | `NoSuchBucket` |
| `StoreError` (object not found) | `NoSuchKey` |
| `StoreError` (other) | `InternalError` |

---

## 8. Mapping to alknet

### 8.1 rustfs Events → alknet Integration Events

rustfs events are **integration events from rustfs's perspective** and remain **integration events from alknet's perspective**. This is the correct cross-boundary classification per ADR-032.

#### Event Projection: `rustfs::BucketNotificationEvent` → `alknet::EventEnvelope`

Suggested namespace and operation mapping:

| rustfs EventName | alknet Namespace | alknet Operation |
|------------------|-----------------|-----------------|
| `s3:ObjectCreated:Put` | `storage.object` | `created.put` |
| `s3:ObjectCreated:Post` | `storage.object` | `created.post` |
| `s3:ObjectCreated:Copy` | `storage.object` | `created.copy` |
| `s3:ObjectCreated:CompleteMultipartUpload` | `storage.object` | `created.multipart-complete` |
| `s3:ObjectRemoved:Delete` | `storage.object` | `removed.delete` |
| `s3:ObjectRemoved:DeleteMarkerCreated` | `storage.object` | `removed.delete-marker-created` |
| `s3:ObjectAccessed:Get` | `storage.object` | `accessed.get` |
| `s3:ObjectAccessed:Head` | `storage.object` | `accessed.head` |
| `s3:BucketCreated:*` | `storage.bucket` | `created` |
| `s3:BucketRemoved:*` | `storage.bucket` | `removed` |

The full `Event` record from rustfs should be preserved in the `EventEnvelope.payload` field for traceability, while a normalized `metadata` extraction provides fast-path access:

```rust
// Pseudocode for mapping
fn project_rustfs_event(event: &rustfs_notify::Event) -> alknet::EventEnvelope {
    let namespace = if event.event_name == EventName::BucketCreated || event.event_name == EventName::BucketRemoved {
        "storage.bucket"
    } else {
        "storage.object"
    };
    
    let operation = event.event_name.as_str()  // "s3:ObjectCreated:Put"
        .strip_prefix("s3:")                   // "ObjectCreated:Put"
        .unwrap_or("unknown")
        .to_lowercase()
        .replace(':',, "."); 

    EventEnvelope {
        id: uuid::Uuid::new_v4(),
        namespace: namespace.into(),
        operation: operation.into(),     // e.g., "objectcreated.put"
        timestamp: event.event_time,
        source: "rustfs".into(),
        metadata: json!({
            "bucket": event.s3.bucket.name,
            "key": event.s3.object.key,
            "size": event.s3.object.size,
            "eTag": event.s3.object.e_tag,
            "versionId": event.s3.object.version_id,
            "sequencer": event.s3.object.sequencer,
            "principalId": event.user_identity.principal_id,
        }),
        payload: serde_json::to_value(event).ok(),
    }
}
```

### 8.2 Subscription Architecture

#### Option A: In-Process Live Event Stream (Recommended)

Since alknet and rustfs share the same process, alknet can subscribe to the live event stream directly:

```rust
// In alknet's initialization
let notification_system = rustfs_notify::notification_system().unwrap();
let mut event_rx = notification_system.subscribe_live_events();

// In alknet's event loop
tokio::spawn(async move {
    while let Ok(event) = event_rx.recv().await {
        let envelope = project_rustfs_event(&event);
        alknet::honker::publish(envelope).await;
    }
});
```

**Advantages**:
- Zero-latency, zero-serialization overhead
- No network hop
- Direct access to `Arc<Event>` in-process
- alknet's Honker streams get events immediately

**Considerations**:
- `has_live_listeners()` can be checked before performing expensive event construction
- The broadcast channel capacity is 1024; slow consumers will miss events (acceptable for integration events)
- `recent_live_events_since()` allows catch-up after reconnection

#### Option B: External Target via Webhook/Kafka/etc.

If alknet runs as a separate process, configure a webhook or Kafka target pointing to alknet's event ingestion endpoint:

```json
{
  "notify_webhook": {
    "1": {
      "enable": true,
      "endpoint": "https://alknet.internal/events/rustfs",
      "auth_token": "Bearer alknet-secret"
    }
  }
}
```

**Advantages**:
- Decoupled deployment
- RustFS's queue store provides at-least-once delivery

**Considerations**:
- Network latency and serialization overhead
- Need to handle deduplication (at-least-once means possible duplicates)
- Queue store provides durability if alknet is temporarily unavailable

#### Option C: Hybrid — Live Stream + Webhook Fallback

For maximum reliability:
1. In-process live stream for low-latency event propagation
2. Webhook/Kafka target as a fallback for events missed during restarts
3. Use `sequentor` ordering to detect gaps

### 8.3 S3 Select → alknet Operations

S3 Select can be exposed as an alknet operation:

| alknet Operation | Description |
|-----------------|-------------|
| `storage.select` | Run an S3 Select SQL query on an object |
| `storage.select-status` | Check Select availability (optional) |

```rust
// Example alknet call protocol operation
fn handle_storage_select(params: StorageSelectParams) -> Result<StorageSelectResult, Error> {
    // 1. Construct SelectObjectContentInput
    // 2. Call existing rustfs SelectObjectContent handler
    // 3. Stream results back through alknet call protocol
}
```

#### Use Cases for alknet

1. **Metagraph Queries**: Query stored metagraph JSON/CSV objects without downloading them entirely
   ```sql
   SELECT s.name, s.version FROM S3Object s WHERE s.type = 'service'
   ```

2. **Log Analytics**: Query structured log data stored in S3
   ```sql
   SELECT COUNT(*) as cnt, s.level FROM S3Object s WHERE s.timestamp > '2026-01-01' GROUP BY s.level
   ```

3. **Ad-hoc Data Exploration**: Quick data inspection without full downloads
   ```sql
   SELECT * FROM S3Object s LIMIT 100
   ```

4. **Aggregation Pipelines**: Pre-process data before moving to alknet's internal stores

### 8.4 ADR-032 Implications: Cross-Boundary Event Flow

Per ADR-032, rustfs events are **integration events** — they represent facts about state changes that have already happened in the storage system boundary. When alknet consumes them:

```
┌─────────────┐                      ┌─────────────┐
│   rustfs     │                      │   alknet    │
│  (bounded    │    integration       │  (bounded    │
│   context)   │───── event ─────────▶│   context)  │
│             │                      │             │
│  S3 Object  │   EventEnvelope      │  Honker     │
│  Created/   │   namespace:         │  Stream      │
│  Removed/   │   "storage.object"   │  Subscriber  │
│  Accessed    │   operation:         │             │
│             │   "created.put"      │  Call        │
│             │                      │  Protocol    │
│  S3 Select   │   storage.select    │  Operation   │
│  Results     │◀──── call ──────────│             │
└─────────────┘                      └─────────────┘
```

Key points:
1. **Events flow inward**: rustfs → alknet (integration events entering alknet's boundary)
2. **Calls flow outward**: alknet → rustfs (alknet initiates S3 Select as a call)
3. **No shared domain model**: alknet shouldn't reference rustfs's `Event` struct directly in its domain; it projects into its own `EventEnvelope` format
4. **Eventual consistency**: rustfs notifications may arrive out of order; `sequentor` field provides ordering within a bucket
5. **At-least-once delivery**: If using webhook/Kafka targets, duplicate events are possible; alknet must be idempotent
6. **No orchestration across boundaries**: alknet doesn't tell rustfs to emit events; it subscribes to events rustfs naturally produces

### 8.5 Implementation Recommendations

1. **Short-term**: Use the **in-process live event stream** to subscribe to rustfs events and re-emit them through alknet's Honker system. This gives immediate value with minimal integration work.

2. **Medium-term**: Add a **webhook notification target** pointing at an alknet HTTP endpoint for redundancy. Configure bucket notification rules via the S3 API (PutBucketNotificationConfiguration).

3. **Long-term**: Consider implementing an **alknet NATS target** that directly publishes events into alknet's NATS infrastructure, bypassing the HTTP layer entirely for lower latency.

4. **S3 Select**: Expose via alknet's call protocol as `storage.select`. The existing `execute_select_object_content` function can be called directly as a library function since alknet and rustfs share the same process.

5. **Event schema versioning**: Store the `event_version` field from rustfs events in alknet's `EventEnvelope.metadata` to handle future schema evolution.

---

## 9. References

### Source Code Locations

| Component | Path |
|-----------|------|
| Event structure | `/crates/notify/src/event.rs` |
| EventName enum | `/crates/s3-types/src/event_name.rs` |
| NotifyPipeline + LiveEventHistory | `/crates/notify/src/pipeline.rs` |
| EventNotifier + TargetList | `/crates/notify/src/notifier.rs` |
| NotificationSystem | `/crates/notify/src/integration.rs` |
| Rule engine | `/crates/notify/src/rule_engine.rs` |
| RulesMap | `/crates/notify/src/rules/rules_map.rs` |
| Bucket notification config | `/crates/notify/src/rules/config.rs` |
| XML notification config | `/crates/notify/src/rules/xml_config.rs` |
| Target trait + QueuedPayload | `/crates/targets/src/target/mod.rs` |
| Webhook target | `/crates/targets/src/target/webhook.rs` |
| Kafka target | `/crates/targets/src/target/kafka.rs` |
| AMQP target | `/crates/targets/src/target/amqp.rs` |
| NATS target | `/crates/targets/src/target/nats.rs` |
| Redis target | `/crates/targets/src/target/redis.rs` |
| MQTT target | `/crates/targets/src/target/mqtt.rs` |
| MySQL target | `/crates/targets/src/target/mysql.rs` |
| PostgreSQL target | `/crates/targets/src/target/postgres.rs` |
| Pulsar target | `/crates/targets/src/target/pulsar.rs` |
| ARN + TargetID | `/crates/targets/src/arn.rs` |
| ECStore event dispatch | `/crates/ecstore/src/event_notification.rs` |
| Server event init | `/rustfs/src/server/event.rs` |
| S3 Select handler | `/rustfs/src/app/select_object.rs` |
| S3 Select query engine | `/crates/s3select-query/src/` |
| S3 Select API | `/crates/s3select-api/src/` |
| S3 Select object store | `/crates/s3select-api/src/object_store.rs` |
| Config subsystem names | `/crates/config/src/notify/mod.rs` |

### AWS S3 Documentation

- [S3 Event Notification Configuration](https://docs.aws.amazon.com/AmazonS3/latest/userguide/EventNotifications.html)
- [S3 Select Documentation](https://docs.aws.amazon.com/AmazonS3/latest/userguide/selecting-content-from-objects.html)

### Internal References

- `/workspace/@alkdev/alknet/docs/research/references/rustfs/rustfs-reference.md` — Companion document covering auth, architecture, and credential mapping