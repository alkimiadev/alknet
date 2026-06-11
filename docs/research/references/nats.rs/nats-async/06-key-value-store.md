# async-nats: Key-Value Store

## Overview

The Key-Value (KV) store is an abstraction built on top of JetStream streams. Each KV bucket is backed by a JetStream stream with the naming convention `KV_<bucket_name>`. Keys are mapped to subjects under the `$KV.<bucket>.<key>` prefix.

The KV feature requires `kv` (which implies `jetstream`).

## Store Handle

```rust
#[derive(Debug, Clone)]
pub struct Store {
    pub name: String,
    pub stream_name: String,
    pub prefix: String,           // $KV.<bucket>.
    pub put_prefix: Option<String>, // For mirrored buckets
    pub use_jetstream_prefix: bool, // Whether to prepend JS API prefix
    pub stream: Stream,
}
```

## Bucket Config

```rust
#[derive(Debug, Clone, Default)]
pub struct Config {
    pub bucket: String,
    pub description: String,
    pub max_value_size: i32,
    pub history: i64,              // Max historical entries per key (1-64)
    pub max_age: Duration,         // Max age of any entry
    pub max_bytes: i64,            // Total bucket size limit
    pub storage: StorageType,       // File or Memory
    pub num_replicas: usize,
    pub republish: Option<Republish>,
    pub mirror: Option<Source>,     // Mirror another bucket
    pub sources: Option<Vec<Source>>,
    pub mirror_direct: bool,
    pub compression: bool,          // server_2_10+
    pub placement: Option<Placement>,
    pub limit_markers: Option<Duration>, // server_2_11+
}
```

## Creating/Accessing Buckets

```rust
// Create a new bucket
let kv = jetstream.create_key_value(kv::Config {
    bucket: "my-bucket".to_string(),
    history: 10,
    max_age: Duration::from_secs(3600),
    ..Default::default()
}).await?;

// Get an existing bucket
let kv = jetstream.get_key_value("my-bucket").await?;

// Create or update
let kv = jetstream.create_or_update_key_value(kv::Config { ... }).await?;

// Delete a bucket
jetstream.delete_key_value("my-bucket").await?;
```

## KV Operations

### Put

```rust
let revision: u64 = kv.put("key", "value".into()).await?;
```

Publishes to `$KV.<bucket>.<key>` (or with JS prefix). The JetStream stream stores it, and the returned sequence number serves as the revision.

### Get

```rust
let value: Option<Bytes> = kv.get("key").await?;
```

Returns `None` if the key doesn't exist or was deleted/purged. Uses either direct get (if `allow_direct`) or the standard message API.

### Entry

```rust
let entry: Option<Entry> = kv.entry("key").await?;
let entry: Option<Entry> = kv.entry_for_revision("key", 2).await?;
```

Returns full entry metadata:

```rust
pub struct Entry {
    pub bucket: String,
    pub key: String,
    pub value: Bytes,
    pub revision: u64,
    pub created: DateTime,
    pub delta: u64,
    pub operation: Operation,
    pub seen_current: bool,
}
```

### Create (Put if not exists)

```rust
let revision: u64 = kv.create("key", "value".into()).await?;
```

Uses `update` with `expected_last_subject_sequence = 0` (create-only). If the key exists and is deleted/purged, it's re-created.

### Update (Conditional Put)

```rust
let revision: u64 = kv.update("key", "value".into(), last_revision).await?;
```

Uses the `Nats-Expected-Last-Subject-Sequence` header for optimistic concurrency control. Only succeeds if the key's current revision matches.

### Delete

```rust
kv.delete("key").await?;
kv.delete_expect_revision("key", Some(revision)).await?;
```

Non-destructive — publishes a `DEL` marker message. The key appears deleted to `get()`, but history is preserved (up to `history` limit).

### Purge

```rust
kv.purge("key").await?;
kv.purge_with_ttl("key", Duration::from_secs(10)).await?;
kv.purge_expect_revision("key", Some(revision)).await?;
```

Destructive — publishes a `PURGE` marker with rollup header, removing all previous revisions of the key. Leaves a single purge entry.

### Watch

```rust
// Watch for new changes
let mut watch = kv.watch("key").await?;
// Watch with initial value
let mut watch = kv.watch_with_history("key").await?;
// Watch from specific revision
let mut watch = kv.watch_from_revision("key", 5).await?;
// Watch all keys
let mut watch = kv.watch_all().await?;
// Watch multiple keys (server_2_10+)
let mut watch = kv.watch_many(["foo", "bar"]).await?;
```

`Watch` implements `futures_util::Stream<Item = Result<Entry, WatcherError>>`.

Under the hood, each watch creates an **ordered push consumer** on the KV stream with:
- `filter_subject` matching `$KV.<bucket>.<key>`
- `replay_policy: Instant`
- Appropriate `deliver_policy`

### History

```rust
let mut history = kv.history("key").await?;
```

Returns a `Stream` of all past `Entry` values for a key (including deletes/purges).

### Keys

```rust
let mut keys = kv.keys().await?;
```

Returns a `Stream<String>` of all current keys. Uses a headers-only consumer with `LastPerSubject` deliver policy to efficiently scan the bucket.

## Entry Operations

```rust
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum Operation {
    Put,    // Value was put
    Delete, // Value was deleted (DEL marker)
    Purge,  // Value was purged (PURGE marker with rollup)
}
```

The operation type is determined from the `KV-Operation` header (`PUT`, `DEL`, `PURGE`) or the `Nats-Marker-Reason` header (fallback for server-generated markers like `MaxAge`, `Purge`, `Remove`).

## Key and Bucket Name Validation

```rust
// Bucket: alphanumeric, dash, underscore only
VALID_BUCKET_RE: \A[a-zA-Z0-9_-]+\z

// Key: alphanumeric, dash, slash, underscore, equals, dot; no leading/trailing dots
VALID_KEY_RE: \A[-/_=\.a-zA-Z0-9]+\z
```

## Bucket Status

```rust
let status: Status = kv.status().await?;
```

Wraps stream info to provide bucket-level statistics (bucket name, message count, byte count, etc.).

## Mirrored Buckets

When a bucket is configured as a mirror of another (potentially in a different account/domain):

- `prefix` is set to `$KV.<mirror_bucket>.`
- `put_prefix` may be set to the source bucket's API prefix for cross-domain writes
- `use_jetstream_prefix` is adjusted based on whether the mirror is in the same domain

## KV → Stream Config Mapping

When creating a KV bucket, the `Config` is converted to a JetStream `stream::Config`:

| KV Config | Stream Config |
|-----------|---------------|
| `bucket` | `name = "KV_<bucket>"` |
| `subjects` | `["$KV.<bucket>.>"]` |
| `max_messages_per_subject` | `history` (max 64) |
| `max_age` | `max_age` |
| `max_bytes` | `max_bytes` |
| `storage` | `storage` |
| `num_replicas` | `num_replicas` |
| `republish` | `republish` |
| `mirror` | `mirror` |
| `discard` | `DiscardPolicy::New` |
| `allow_direct` | `true` |
| `allow_rollup_hdrs` | `true |
| `max_msg_size` | `max_value_size` |