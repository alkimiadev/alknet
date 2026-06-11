# async-nats: Object Store

## Overview

The Object Store provides large-object storage built on JetStream. Objects are chunked and stored as messages in a JetStream stream, with metadata stored separately. The stream is named `OBJ_<bucket_name>`.

The object-store feature requires `object-store` (which implies `jetstream` + `crypto`).

## ObjectStore Handle

```rust
#[derive(Clone)]
pub struct ObjectStore {
    pub(crate) name: String,
    pub(crate) stream: Stream,
}
```

## Object Store Config

```rust
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Config {
    pub bucket: String,
    pub description: Option<String>,
    pub max_age: Duration,
    pub max_bytes: i64,
    pub storage: StorageType,
    pub num_replicas: usize,
    pub compression: bool,
    pub placement: Option<Placement>,
}
```

## Creating/Accessing Object Stores

```rust
// Create
let bucket = jetstream.create_object_store(object_store::Config {
    bucket: "my-bucket".to_string(),
    ..Default::default()
}).await?;

// Get existing
let bucket = jetstream.get_object_store("my-bucket").await?;

// Delete
jetstream.delete_object_store("my-bucket").await?;
```

## Object Store Operations

### Put

```rust
let info: ObjectInfo = bucket.put("file.txt", &mut async_read).await?;
```

The put operation:
1. Reads data from any `AsyncRead + Unpin` source in chunks (default 128KB)
2. Each chunk is published to `$O.<bucket>.C.<nuid>` (chunk subject)
3. SHA-256 digest is computed incrementally
4. After all chunks, metadata is published to `$O.<bucket>.M.<encoded_name>` with a rollup header
5. If the object previously existed, old chunks are purged

### Get

```rust
let mut object: Object = bucket.get("file.txt").await?;
```

Returns an `Object` that implements `tokio::io::AsyncRead`:

```rust
let mut bytes = Vec::new();
object.read_to_end(&mut bytes).await?;
```

On read, the Object:
1. Creates an ordered push consumer on `$O.<bucket>.C.<nuid>`
2. Streams chunk messages, feeding bytes to the reader
3. Verifies SHA-256 digest after the last chunk
4. If digest doesn't match, returns `io::ErrorKind::InvalidData`

### Delete

```rust
bucket.delete("file.txt").await?;
```

Marks the object as deleted in metadata (sets `deleted = true`, `chunks = 0`, `size = 0`) with a rollup, then purges all chunk messages.

### Info

```rust
let info: ObjectInfo = bucket.info("file.txt").await?;
```

Fetches the last metadata message for the object (from `$O.<bucket>.M.<encoded_name>`).

### Watch

```rust
let mut watcher = bucket.watch().await?;
let mut watcher = bucket.watch_with_history().await?;
```

Returns a `Stream<Item = Result<ObjectInfo, WatcherError>>`. Uses an ordered push consumer on `$O.<bucket>.M.>`.

### List

```rust
let mut list = bucket.list().await?;
```

Returns a `Stream<Item = Result<ObjectInfo, ListerError>>`. Lists all non-deleted objects. Uses `DeliverPolicy::All` to replay all metadata.

### Seal

```rust
bucket.seal().await?;
```

Sets the underlying stream's `sealed = true`, preventing any further modifications.

### Links

```rust
// Link to another object (same or different bucket)
let info = bucket.add_link("link_name", &object).await?;

// Link to another bucket
let info = bucket.add_bucket_link("link_name", "other_bucket").await?;
```

Links are followed automatically when `get()` is called (one level deep). Cannot link to a deleted object or create a link to a link.

### Update Metadata

```rust
bucket.update_metadata("object", object_store::UpdateMetadata {
    name: "new_name".to_string(),
    description: Some("updated description".to_string()),
    ..Default::default()
}).await?;
```

If the name changes, old metadata is purged and new metadata is published.

## Object Types

### ObjectInfo

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ObjectInfo {
    pub name: String,
    pub description: Option<String>,
    pub metadata: HashMap<String, String>,
    pub headers: Option<HeaderMap>,
    pub options: Option<ObjectOptions>,
    pub bucket: String,
    pub nuid: String,
    pub size: usize,
    pub chunks: usize,
    pub modified: Option<DateTime>,
    pub digest: Option<String>,   // Format: "SHA-256=<base64url-digest>"
    pub deleted: bool,
}
```

### ObjectMetadata

```rust
#[derive(Debug, Default, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ObjectMetadata {
    pub name: String,
    pub description: Option<String>,
    pub chunk_size: Option<usize>,
    pub metadata: HashMap<String, String>,
    pub headers: Option<HeaderMap>,
}
```

### ObjectLink

```rust
#[derive(Debug, Default, Clone, Serialize, Deserialize, Eq, PartialEq)]
pub struct ObjectLink {
    pub name: Option<String>,  // None = bucket link, Some = object link
    pub bucket: String,
}
```

### Object

```rust
pub struct Object {
    pub info: ObjectInfo,
    remaining_bytes: VecDeque<u8>,
    has_pending_messages: bool,
    digest: Option<Sha256>,
    subscription: Option<crate::jetstream::consumer::push::Ordered>,
    subscription_future: Option<BoxFuture<'static, Result<Ordered, StreamError>>>,
    stream: Stream,
}
```

Implements `tokio::io::AsyncRead`. Lazy-creates the consumer on first read.

## Subject Naming Convention

| Purpose | Subject Pattern |
|---------|----------------|
| Chunks | `$O.<bucket>.C.<nuid>` |
| Metadata | `$O.<bucket>.M.<base64url-encoded-name>` |

Object names are base64url-encoded in metadata subjects to allow arbitrary characters (the raw name might contain characters invalid in NATS subjects).

## Validation

```rust
// Bucket: alphanumeric, dash, underscore only
BUCKET_NAME_RE: \A[a-zA-Z0-9_-]+\z

// Object name: alphanumeric, dash, slash, underscore, equals, dot; no leading/trailing dots
OBJECT_NAME_RE: \A[-/_=\.a-zA-Z0-9]+\z
```

## Data Integrity

The object store uses SHA-256 hashing (from the `crypto` module) to verify data integrity:

1. On `put()`: SHA-256 is computed incrementally as chunks are read. The digest is stored in `ObjectInfo.digest` as `"SHA-256=<base64url>"`.
2. On `get()` (via `AsyncRead`): SHA-256 is verified after the last chunk is read. If the computed digest doesn't match the stored digest, `io::ErrorKind::InvalidData` is returned.

```rust
// crypto module
pub(crate) struct Sha256 { ... }
impl Sha256 {
    pub fn new() -> Self;
    pub fn update(&mut self, data: &[u8]);
    pub fn finish(self) -> [u8; 32];
}
```