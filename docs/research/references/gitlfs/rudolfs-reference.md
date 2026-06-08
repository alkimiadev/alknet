# Rudolfs Reference Document

> **Source**: <https://github.com/jasonwhite/rudolfs> (cloned at `/workspace/rudolfs/`)
> **Version**: 0.3.8 (Cargo.toml)
> **License**: MIT
> **Date researched**: 2026-06-08
> **Purpose**: Evaluate rudolfs as a Git LFS server for the alknet git hosting stack (gitserver + rudolfs + rustfs)

---

## 1. Architecture Overview

### 1.1 What is Rudolfs?

Rudolfs is a **high-performance, caching Git LFS server** written in Rust with an AWS S3 storage backend. It implements the Git LFS batch API specification, providing a clean separation between the LFS protocol layer and pluggable storage backends. The storage system uses a **decorator (Russian doll) pattern** where backends are composed in layers — each adding a capability (encryption, verification, caching, retrying) — resulting in a flexible and composable architecture.

Key design principles:
- **Modular storage backends**: S3, local disk, and any combination with caching
- **Streaming everywhere**: All data flows as async byte streams; nothing is buffered entirely in memory
- **Decorator composition**: Storage capabilities are stacked via composable wrapper types
- **Corruption resilience**: SHA256 verification on upload and download; corrupted objects are auto-purged from cache
- **Optional encryption**: XChaCha20 stream cipher for data at rest (both cache and permanent storage)

### 1.2 Source Structure

```
src/
├── main.rs              # CLI entry point (structopt), server startup
├── lib.rs               # Public API: S3ServerBuilder, LocalServerBuilder, Cache, Server trait
├── app.rs               # Hyper service: HTTP routing, batch API, upload/download endpoints
├── lfs.rs               # LFS protocol types (BatchRequest/Response, Action, Oid, Transfer)
├── sha256.rs            # Sha256 type, hex serde, VerifyStream (streaming SHA256 checker)
├── lru.rs               # In-memory LRU cache (LinkedHashMap-based) for cache metadata
├── error.rs             # Error type (re-export of anyhow::Error)
├── hyperext.rs          # RequestExt trait (X-Forwarded-Proto, X-Forwarded-Host, Host headers)
├── logger.rs            # Logger middleware (request/response logging)
├── util.rs              # NamedTempFile (async temp file with auto-cleanup + atomic rename)
└── storage/
    ├── mod.rs            # Core traits: Storage, LFSObject, StorageKey, Namespace, ByteStream
    ├── s3.rs             # S3 backend (rusoto): multipart upload, head/get/put, presigned URLs
    ├── disk.rs           # Local disk backend: file-based storage with temp file pattern
    ├── cached.rs         # Caching decorator: LRU + disk cache → permanent storage
    ├── encrypt.rs        # Encryption decorator: XChaCha20 stream cipher
    ├── verify.rs         # Verification decorator: streaming SHA256 with auto-delete on corruption
    ├── retrying.rs       # Retry decorator: exponential backoff for size/delete operations
    └── faulty.rs         # Fault injection decorator (test-only, gated by "faulty" feature)
```

### 1.3 Key Dependencies

| Dependency | Version | Purpose |
|---|---|---|
| `hyper` | 0.14 | HTTP server (no frameworks like Axum/Actix) |
| `rusoto_s3` | 0.48 | S3 API client (PutObject, GetObject, HeadObject, multipart upload) |
| `rusoto_core` | 0.48 | AWS credential provider, HTTP client |
| `rusoto_sts` | 0.48 | Kubernetes WebIdentity credential provider |
| `rusoto_credential` | 0.48 | DefaultCredentialsProvider chain (env → file → IAM) |
| `tokio` | 1 | Async runtime (full features) |
| `tokio-util` | 0.7 | BytesCodec for file I/O streaming |
| `futures` | 0.3 | Stream combinators, try_join_all, channel mpsc |
| `serde` / `serde_json` | 1 / 1 | JSON serialization for LFS batch API |
| `sha2` | 0.11.0-rc.3 | SHA256 hashing for LFS object verification |
| `chacha` | 0.4 | XChaCha20 stream cipher for encryption |
| `linked-hash-map` | 0.5 | LRU cache data structure with O(1) get_refresh |
| `askama` | 0.14 | HTML template engine (index page) |
| `structopt` | 0.3 | CLI argument parsing |
| `backoff` | 0.4 | Exponential backoff for S3 retries |
| `uuid` | 1.1 | Temp file naming (v4) |
| `bytes` | 1 | Bytes/BytesMut for streaming |
| `derive_more` | 2 | Display/From derives for error types |
| `human-size` | 0.4 | Human-readable size parsing (CLI) |
| `humansize` | 2 | Human-readable size formatting (logging) |

**Note**: rudolfs uses `rusoto` (the older Rust AWS SDK), not `aws-sdk-rust`. This is important for compatibility with S3-compatible stores like MinIO and, by extension, rustfs.

### 1.4 Request Flow

#### LFS Batch API (Upload)

```
Client → POST /api/{org}/{project}/objects/batch
       → { "operation": "upload", "objects": [{ "oid": "...", "size": N }] }
       → Server: parse request, check storage.size() for each object
       ← { "objects": [{ "oid": "...", "size": N, "actions": { "upload": { "href": "..." }, "verify": { "href": "..." } } }] }

Client → PUT /api/{org}/{project}/object/{oid}
       → Raw LFS object data (Content-Length header required)
       → Server: stream through Verify→Encrypted→Cached→S3 (or disk)
       ← 200 OK (empty body)
```

#### LFS Batch API (Download)

```
Client → POST /api/{org}/{project}/objects/batch
       → { "operation": "download", "objects": [...] }
       ← { "objects": [{ "oid": "...", "actions": { "download": { "href": "..." } } }] }

Client → GET /api/{org}/{project}/object/{oid}
       → Server: lookup in cache (LRU metadata), fetch from storage if needed
       ← Streamed LFS object data with Content-Length
```

#### CDN Mode (Presigned URLs)

When `--cdn` is configured, the batch response for downloads returns a presigned S3 URL directly instead of routing through rudolfs. Uploads also use presigned S3 URLs. This bypasses encryption (since data never touches rudolfs).

---

## 2. Git LFS Protocol Implementation

### 2.1 LFS Protocol Overview

Git LFS uses an HTTP-based API where the client first contacts the LFS server to discover _how_ to transfer objects, then performs the actual transfers. Rudolfs implements the "basic" transfer adapter exclusively.

**Specification reference**: [git-lfs/docs/api/batch.md](https://github.com/git-lfs/git-lfs/blob/master/docs/api/batch.md)

### 2.2 Endpoints Implemented

| Route | Method | Purpose |
|---|---|---|
| `/` | GET | HTML index page with setup instructions |
| `/api/{org}/{project}/objects/batch` | POST | LFS batch API — core protocol endpoint |
| `/api/{org}/{project}/object/{oid}` | GET | Direct download of a single LFS object |
| `/api/{org}/{project}/object/{oid}` | PUT | Direct upload of a single LFS object |
| `/api/{org}/{project}/objects/verify` | POST | Verify object exists with correct size |

### 2.3 Batch API Detail

**Request** (`BatchRequest`):

```rust
pub struct BatchRequest {
    pub operation: Operation,       // "upload" or "download"
    pub transfers: Option<Vec<Transfer>>,  // Transfer adapters client supports
    pub refs: Option<BTreeMap<String, String>>,  // Git ref context (v2.4+)
    pub objects: Vec<RequestObject>,  // Objects to transfer
}
```

**Response** (`BatchResponse`):

```rust
pub struct BatchResponse {
    pub transfer: Option<Transfer>,  // Always Transfer::Basic
    pub objects: Vec<ResponseObject>,
}
```

**Each `ResponseObject`** contains:
- `oid`: SHA256 hash of the object
- `size`: Byte size
- `error`: Optional error (code + message)
- `authenticated`: Some(true) — signals the client that auth is handled
- `actions`: Optional Actions with href, header, expires_in/expires_at

### 2.4 Transfer Adapters

```rust
pub enum Transfer {
    Basic,                    // The basic HTTP transfer adapter
    LfsStandaloneFile,       // Parsed but not implemented
    Custom,                  // Catch-all via #[serde(other)]
}
```

Rudolfs always responds with `Transfer::Basic`. It parses `LfsStandaloneFile` and `Custom` from client requests but never selects them.

### 2.5 Upload Expiration

Upload actions include an `expires_in` field set to **30 minutes** (1800 seconds):

```rust
const UPLOAD_EXPIRATION: Duration = Duration::from_secs(30 * 60);
```

### 2.6 Auth Header Reflection

Rudolfs has **no built-in authentication**. However, it reflects any `Authorization` header from the incoming batch request back into the `header` field of all action URLs. This allows a reverse proxy (e.g., nginx with basic auth) to provide authentication:

```rust
fn extract_auth_header(headers: &HeaderMap) -> Option<BTreeMap<String, String>> {
    // Filters for "authorization" header and reflects it into action.header
}
```

This is critical for the alknet integration model — authentication is delegated entirely to an outer layer.

### 2.7 Namespace (org/project) Extraction

The URL path `/api/{org}/{project}/...` is parsed to extract the namespace:

```rust
let namespace = match (parts.next(), parts.next()) {
    (Some(org), Some(project)) => Namespace::new(org.into(), project.into()),
    _ => // 400 Bad Request
};
```

This provides **multi-tenancy by URL path**. Different orgs/projects are isolated in storage (S3 key prefix or disk directory), but objects with identical OIDs across projects can share storage if they use the same namespace path.

### 2.8 Verify Endpoint

```rust
pub struct VerifyRequest {
    pub oid: Oid,
    pub size: u64,
}
```

The verify endpoint (`POST .../objects/verify`) checks whether an uploaded object exists and has the correct size. It calls `storage.size(&key)` and compares against the request's size field.

---

## 3. Storage Architecture

### 3.1 The Storage Trait

The core abstraction is the `Storage` trait:

```rust
#[async_trait]
pub trait Storage {
    type Error: fmt::Display + Send;

    async fn get(&self, key: &StorageKey) -> Result<Option<LFSObject>, Self::Error>;
    async fn put(&self, key: StorageKey, value: LFSObject) -> Result<(), Self::Error>;
    async fn size(&self, key: &StorageKey) -> Result<Option<u64>, Self::Error>;
    async fn delete(&self, key: &StorageKey) -> Result<(), Self::Error>;
    fn list(&self) -> StorageStream<(StorageKey, u64), Self::Error>;
    async fn total_size(&self) -> Option<u64>;
    async fn max_size(&self) -> Option<u64>;
    fn public_url(&self, key: &StorageKey) -> Option<String>;
    async fn upload_url(&self, key: &StorageKey, expires_in: Duration) -> Option<String>;
}
```

**Key types**:

- `StorageKey = (Namespace, Oid)` — composite key for all storage operations
- `Namespace = (org: String, project: String)` — tenant/organization isolation
- `Oid = Sha256` — 32-byte SHA256 hash, displayed as hex
- `LFSObject` — holds `(len: u64, stream: ByteStream)` where `ByteStream` is a `Pin<Box<dyn Stream<Item = Result<Bytes, io::Error>> + Send>>`
- `ByteStream` — pinned, boxed, async byte stream

### 3.2 LFSObject — Fanout Pattern

`LFSObject` has a critical `fanout()` method that duplicates the byte stream into two identical streams using `mpsc::channel(0)`:

```rust
pub fn fanout(self) -> (impl Future<Output = Result<(), io::Error>>, Self, Self)
```

This is used in two places:
1. **Caching on download**: Stream flows to both the client and the cache simultaneously
2. **Caching on upload**: Stream flows to both the permanent storage and the cache, with a `oneshot` signal ensuring the cache only persists after the upload succeeds

### 3.3 Storage Composition

The decorator pattern produces these composition chains depending on configuration:

#### S3 with Cache and Encryption (fullest stack):

```
Client ↔ Verify ↔ Encrypted ↔ Cached ↔ Retrying(Disk → S3)
                                         ↑cache   ↑permanent
```

Actual call order during `put`:
1. **Verify**: wraps stream with SHA256 verification; rejects upload if hash mismatch
2. **Encrypted**: applies XChaCha20 cipher to the stream (nonce = first 24 bytes of OID)
3. **Cached**: fans out stream to both `cache` and `storage`; uses LRU to manage cache
4. **Retrying**: wraps S3 operations with exponential backoff (only for `size` and `delete`)
5. **S3**: multipart uploads to `s3://{bucket}/{prefix}/{org}/{project}/{sha256_path}`

#### S3 with CDN (least processing):

```
Client ↔ App (batch API generates presigned S3 URLs; no data flows through rudolfs)
```

#### Local Disk with Cache and Encryption:

```
Client ↔ Verify ↔ Encrypted ↔ Cached(Disk_cache → Disk_storage)
```

#### Local Disk only:

```
Client ↔ Verify ↔ Disk
```

### 3.4 S3 Backend Deep Dive

#### Object Key Format

```rust
fn key_to_path(&self, key: &StorageKey) -> String {
    // With prefix "lfs":   "lfs/{org}/{project}/{sha256_path}"
    // Without prefix:       "{org}/{project}/{sha256_path}"
}
```

Where `sha256_path` is formatted as `{first_two_hex}/{next_two_hex}/{full_hex}`:

```rust
// Sha256Path Display:
write!(f, "{:02x}/{:02x}/{}", self.0.bytes()[0], self.0.bytes()[1], self.0)
```

Example: an OID `b1fbeefc23e6a1496f7d0c2fb635bfc78f7ddc2da963ea9c6a63eb324260e6d` in namespace `myorg/myproject` with default prefix becomes:

```
lfs/myorg/myproject/b1/fe/b1fbeefc23e6a1496f7d0c2fb635bfc78f7ddc2da963ea9c6a63eb324260e6d
```

The two-level hex prefix distributes objects across S3's flat namespace for better I/O performance.

#### S3 Operations

| Operation | S3 API | Notes |
|---|---|---|
| Get object | `GetObject` | Streams response body |
| Put object | `CreateMultipartUpload` → `UploadPart` (100MB chunks) → `CompleteMultipartUpload` | Handles files >5GB |
| Check size | `HeadObject` | Uses content_length; handles Rusoto bug for 404 |
| Delete | No-op | Always returns `Ok(())` — never deletes from S3 |
| List | Empty stream | Returns no entries (S3 listing not implemented) |
| Presigned download | `GetPreSignedUrl` | Only when CDN is configured |
| Presigned upload | `PutObject` presigned URL | Only when CDN is configured |

#### HeadObject 404 Bug Workaround

Rusoto has a known bug where `HeadObject` for a missing key returns `RusotoError::Unknown` with status 404 instead of `HeadObjectError::NoSuchKey`. Rudolfs checks for this:

```rust
Err(RusotoError::Unknown(e)) if e.status == 404 => Ok(None)
```

#### Initialization Validation

On startup, the S3 backend performs a `HeadBucket` request with exponential backoff to validate:
1. The bucket exists
2. Credentials are valid

This ensures fast failures for misconfiguration.

#### Credential Provider Chain

```rust
// Try Kubernetes WebIdentity first
let k8s_provider = WebIdentityProvider::from_k8s_env();
if k8s_provider.credentials().await.is_ok() {
    // Use K8s credentials
} else {
    // Fall back to default provider chain:
    // 1. AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY env vars
    // 2. ~/.aws/credentials file
    // 3. IAM instance profile
}
```

#### Custom S3 Endpoints

The `AWS_S3_ENDPOINT` environment variable enables custom S3-compatible endpoints:

```rust
let region = if let Ok(endpoint) = std::env::var("AWS_S3_ENDPOINT") {
    // Must also set AWS_DEFAULT_REGION or AWS_REGION
    Region::Custom { name, endpoint }
} else {
    Region::default()
}
```

This is how MinIO (and rustfs) integration works — set `AWS_S3_ENDPOINT` to the MinIO/rustfs URL.

### 3.5 Disk Backend Deep Dive

#### On-Disk Layout

```
{root}/
├── objects/
│   └── {org}/
│       └── {project}/
│           └── {sha256_prefix}/
│               └── {sha256}/
│                   └── {sha256_hex_full}       # Actual object file
└── incomplete/                                  # Temp directory for in-progress uploads
    └── {uuid}                                  # NamedTempFile, auto-cleaned on failure
```

#### Write Pattern (Atomic Put)

1. Create a `NamedTempFile` in `{root}/incomplete/{uuid}`
2. Stream the entire LFS object to the temp file via `Framed` codec
3. Verify byte count matches expected `len`
4. `fs::create_dir_all` for the target directory
5. `file.persist(path)` — atomic `rename()` from temp to final location

This ensures that partially written objects never appear at their final path.

#### SHA256 Path Structure

The same `{first_two_hex}/{next_two_hex}/{full_hex}` structure as S3 is used, creating a 2-level directory hierarchy that prevents any single directory from having too many entries.

---

## 4. Caching Layer

### 4.1 Architecture

The `Cached` backend wraps an inner cache storage (typically `Disk`) and an outer permanent storage (typically `S3`), connected by an in-memory LRU index:

```
        ┌─────────────┐
get()──→│ LRU Index    │──→ cache hit? ──→ Disk.get()
        │ (in-memory)  │                   └── miss? ──→ S3.get() + background cache
        └─────────────┘
```

```rust
pub struct Backend<C, S> {
    lru: Arc<Mutex<Cache>>,   // In-memory LRU metadata index
    max_size: u64,             // Maximum cache size in bytes (0 = unlimited)
    cache: Arc<C>,             // Cache storage (usually Disk)
    storage: Arc<S>,           // Permanent storage (usually S3)
}
```

### 4.2 LRU Data Structure

```rust
pub struct Cache<K> {
    map: LinkedHashMap<K, u64>,  // Key → size in bytes
    size: u64,                     // Total size of all entries
}
```

Uses `linked_hash_map` which maintains insertion order and provides `get_refresh()` to move an accessed entry to the most-recently-used position. This gives O(1) access and O(1) eviction from the front.

### 4.3 Cache Hit/Miss Behavior

#### `get()` — Download

1. **LRU hit**: If `lru.get_refresh(key)` returns size → query `cache.get(key)`
   - If cache has the object → return it (fast path)
   - If cache miss (file deleted but LRU not updated) → remove from LRU, fall through to storage
2. **LRU miss**: Query `storage.get(key)`
   - If storage has it → `fanout()` the stream: one copy to client, one copy to cache in background
   - If storage doesn't have it → return `None`

Background caching is fire-and-forget — errors are logged but don't affect the client response.

#### `put()` — Upload

1. `fanout()` the uploaded stream into two copies
2. One copy goes to permanent storage (`storage.put()`)
3. When permanent storage finishes receiving, a `oneshot` signal is sent
4. The cache copy appends an empty `Bytes::new()` chunk that only resolves after this signal
5. Both streams complete in parallel via `try_join3(f, cache, store)`

This ensures **the cache only persists data that was successfully stored permanently**.

#### `size()` — Batch API Check

Only checks the LRU index (without perturbing LRU order). Falls through to permanent storage if not cached. This is critical for the batch API, which checks which objects already exist.

#### `delete()` — Cache Only

Only deletes from the cache, never from permanent storage. Called by the `Verify` decorator when corruption is detected.

### 4.4 Eviction Policy

When the cache exceeds `max_size` bytes:
1. `pop()` the least-recently-used entry from the LRU
2. Delete the corresponding file from cache storage
3. Repeat until `lru.size() <= max_size`

**Important**: `max_size = 0` means **unlimited** cache — no eviction occurs.

### 4.5 Startup Prepopulation

On startup, the LRU index is rebuilt from the cache storage's `list()` method:

```rust
let lru = Cache::from_stream(cache.list()).await?;
```

Then immediately pruned if the current cache exceeds the configured `max_size`:

```rust
let count = prune_cache(lru.clone(), max_size, cache.clone()).await?;
```

**For S3 backend**: `list()` returns an empty stream (not implemented), so the LRU starts empty and builds up as objects are accessed.

**For Disk backend**: `list()` walks the filesystem to rebuild the LRU index, making the disk cache survive restarts.

### 4.6 Corruption Resilience via Verify

The `Verify` decorator wraps the byte stream with a streaming SHA256 hash:

```rust
// On download:
let stream = VerifyStream::new(stream.map_err(Error::from), len, *key.oid());
// If SHA256 mismatches → automatically delete corrupted object from cache
//                                    → return error to client

// On upload:
let stream = VerifyStream::new(stream.map_err(Error::from), len, *key.oid());
// If SHA256 mismatches → reject the upload entirely
```

The `VerifyStream` hashes every chunk and compares the final digest against the expected OID when `len >= total`. This catches:
- Bit rot in the cache
- Encryption key changes (different hash)
- Network transfer errors

---

## 5. Encryption

### 5.1 Algorithm

XChaCha20 stream cipher (extended nonce variant of ChaCha20), implemented via the `chacha` crate.

### 5.2 Key and Nonce

- **Key**: 32 bytes, provided via `--key` CLI flag or `RUDOLFS_KEY` env var (hex-encoded)
- **Nonce**: First 24 bytes of the SHA256 OID used as the XChaCha20 nonce

```rust
let mut nonce: [u8; 24] = [0; 24];
nonce.copy_from_slice(&key.oid().bytes()[0..24]);
let chacha = ChaCha::new_xchacha20(&self.key, &nonce);
```

**Security implication**: The nonce is deterministic (derived from the OID). Since OIDs are content-addressable (SHA256 of content), identical content always produces the same nonce. This is safe because each (key, nonce) pair is only ever used to encrypt one message.

### 5.3 Streaming Encryption

Encryption/decryption is applied to the byte stream in-flight using `xor_stream()`:

```rust
fn xor_stream<S>(mut chacha: ChaCha, stream: S) -> impl Stream<Item = Result<Bytes, io::Error>>
```

No buffering of the entire object — each chunk is encrypted/decrypted as it passes through. This means encryption works on objects of arbitrary size (including those exceeding available memory).

### 5.4 Encryption + CDN Incompatibility

From `lib.rs`:

```rust
if self.cdn.is_some() {
    tracing::warn!("A CDN was specified. Since uploads and downloads do not flow \
                     through Rudolfs in this case, they will *not* be encrypted.");
    // Cache is also disabled when CDN is used
}
```

When CDN mode is active, presigned URLs bypass rudolfs entirely, so encryption cannot be applied.

---

## 6. Retry and Fault Injection

### 6.1 Retry (Retrying Decorator)

Only applies exponential backoff to `size()` and `delete()` operations. `get()` and `put()` cannot be retried because their streaming nature means the stream is consumed:

```rust
async fn get(&self, key: &StorageKey) -> ... {
    // Cannot retry — stream already consumed
    self.storage.get(key).await
}

async fn put(&self, key: StorageKey, value: LFSObject) -> ... {
    // Cannot retry — stream already consumed
    self.storage.put(key, value).await
}

async fn size(&self, key: &StorageKey) -> ... {
    retry(ExponentialBackoff::default(), || async {
        Ok(self.storage.size(key).await?)
    }).await
}
```

### 6.2 Fault Injection (Faulty Decorator)

Gated behind the `faulty` feature flag. Injects random stream errors by checking if `rand() == 0` on each chunk:

```rust
fn faulty_stream(stream: ByteStream) -> ByteStream {
    Box::pin(stream.map(|item| {
        if rand::thread_rng().random::<u8>() == 0 {
            Err(io::Error::other("injected fault"))
        } else {
            item
        }
    }))
}
```

---

## 7. Authentication

**Rudolfs has no built-in authentication.** This is explicitly stated as a "non-feature" in the README:

> There is no client authentication. This is meant to be run in an internal network with clients you trust, not on the internet with malicious actors.

However, the design intentionally supports **proxy-based authentication**:

1. The `extract_auth_header()` function reflects the `Authorization` header from incoming requests into LFS action URLs
2. The `hyperext.rs` module supports `X-Forwarded-Proto` and `X-Forwarded-Host` headers for reverse proxy deployment
3. The `authenticated: Some(true)` field in batch responses tells the git-lfs client that auth has been handled

This means authentication is expected to be provided by an outer layer (nginx, a reverse proxy, or in our case, alknet's identity system).

---

## 8. Locking API

**Rudolfs does NOT implement the LFS Locking API.** The endpoints `POST /api/{org}/{project}/locks`, `GET /api/{org}/{project}/locks`, etc. are not implemented. The `Transfer::LfsStandaloneFile` variant exists in the enum but is never selected or handled.

---

## 9. Configuration

### 9.1 CLI Arguments (structopt)

| Flag | Env Var | Default | Description |
|---|---|---|---|
| `--host` | `RUDOLFS_HOST` | — | Host/address to bind (e.g., `0.0.0.0:8080`) |
| `--port` | `PORT` | 8080 | Port (only used if `--host` not set) |
| `--key` | `RUDOLFS_KEY` | None | 32-byte hex encryption key; omit for no encryption |
| `--cache-dir` | `RUDOLFS_CACHE_DIR` | None | Local disk cache directory; omit for no cache |
| `--max-cache-size` | `RUDOLFS_MAX_CACHE_SIZE` | 50 GiB | Maximum cache size (0 = unlimited) |
| `--log-level` | `RUDOLFS_LOG` | info | Log level (trace/debug/info/warn/error) |

**S3 subcommand**:

| Flag | Env Var | Default | Description |
|---|---|---|---|
| `--bucket` | `RUDOLFS_S3_BUCKET` | (required) | S3 bucket name |
| `--prefix` | `RUDOLFS_S3_PREFIX` | `lfs` | S3 key prefix |
| `--cdn` | `RUDOLFS_S3_CDN` | None | CDN base URL for presigned URLs |

**Local subcommand**:

| Flag | Env Var | Default | Description |
|---|---|---|---|
| `--path` | `RUDOLFS_LOCAL_PATH` | (required) | Local directory for LFS data |

### 9.2 AWS Credential Chain

1. `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` environment variables
2. `~/.aws/credentials` file
3. IAM instance profile (EC2)
4. Kubernetes WebIdentity (`WebIdentityProvider::from_k8s_env()`)

Custom endpoints: `AWS_S3_ENDPOINT` + `AWS_DEFAULT_REGION` (for MinIO/rustfs)

---

## 10. Docker / Deployment

### 10.1 Dockerfile

Multi-stage build producing a **scratch-based image** (very small, <10MB):

1. Build stage: `rust:1.91.1` with `x86_64-unknown-linux-musl` target
2. Uses `tini` as PID 1
3. Copies `ca-certificates.crt` for TLS
4. Final image: `FROM scratch` with only the binary + certs + tini

### 10.2 Docker Compose Variants

| File | Backend | Notes |
|---|---|---|
| `docker-compose.yml` | S3 | AWS S3 backend with cache and encryption |
| `docker-compose.local.yml` | Local disk | Local storage with cache and encryption |
| `docker-compose.minio.yml` | S3 (MinIO) | Uses `AWS_S3_ENDPOINT=http://minio:9000` |

All variants:
- Expose port 8080 (mapped to 8081)
- Use a Docker volume `data` for cache
- Pass encryption key and config via environment variables
- Recommend nginx as a TLS-terminating reverse proxy

---

## 11. Relevance to Alknet

### 11.1 The Complete Git Hosting Stack

```
┌──────────────────────────────────────────────────────┐
│                    Alknet Layer                       │
│  ┌─────────┐  ┌─────────────┐  ┌─────────────────┐  │
│  │ Identity │  │ Call Protocol│  │ Operation Reg.  │  │
│  └────┬─────┘  └──────┬──────┘  └────────┬────────┘  │
│       │               │                   │           │
│  ┌────▼───────────────▼───────────────────▼────────┐ │
│  │              HTTP MessageInterface               │ │
│  └────┬───────────────┬──────────────────┬─────────┘ │
│       │               │                  │           │
└───────┼───────────────┼──────────────────┼───────────┘
        │               │                  │
  ┌─────▼─────┐  ┌──────▼──────┐  ┌────────▼────────┐
  │ gitserver  │  │  rudolfs    │  │     rustfs      │
  │ (git HTTP) │  │ (Git LFS)  │  │   (S3 storage)  │
  └─────┬─────┘  └──────┬──────┘  └────────┬────────┘
        │               │                  │
        │           ┌───▼──────────────────▼───┐
        │           │        Object Store       │
        │           │    (S3 API / rustfs)      │
        │           └──────────────────────────┘
        │
  ┌─────▼─────┐
  │   Git Repo │
  │  (bare)    │
  └───────────┘
```

- **gitserver**: Handles `git-upload-pack`, `git-receive-pack`, `info/refs` (smart HTTP)
- **rudolfs**: Handles `POST /objects/batch`, `GET/PUT /object/{oid}`, `POST /objects/verify` (LFS)
- **rustfs**: Provides the S3-compatible object store that rudolfs uses as backend

### 11.2 Authentication: Alknet Identity → Rudolfs Auth

Rudolfs has **no auth**, but it **reflects auth headers**. This maps perfectly to alknet's approach:

1. **alknet HTTP MessageInterface** terminates the connection, authenticates via alknet Identity
2. The authenticated request is reverse-proxied to rudolfs
3. Alknet injects an `Authorization` header (e.g., a Bearer token or internal auth)
4. Rudolfs reflects this header in LFS action URLs, propagating auth to subsequent client requests

**Integration approach**:
- Add a middleware layer before `App` that validates alknet Identity tokens
- The `extract_auth_header()` function already handles the `Authorization` → `action.header` propagation
- For fine-grained access control (per-org/project), add namespace validation against alknet's permission model

**Code location to modify**: `app.rs` — add auth middleware in the `Service::call()` method, or wrap `App` in a new `AuthMiddleware` service.

### 11.3 Embedding Rudolfs as a Library

Rudolfs exposes a clean library API via `lib.rs`:

```rust
// S3 backend
let mut builder = S3ServerBuilder::new(bucket);
builder.prefix("lfs".into());
builder.key(encryption_key);
builder.cache(Cache::new(cache_dir, max_cache_size));
builder.run(addr).await?;

// Local backend
let mut builder = LocalServerBuilder::new(path);
builder.key(encryption_key);
builder.run(addr).await?;
```

**Feasibility**: The `Storage` trait is fully abstract, and the `App` struct is generic over any `S: Storage`. This means:

1. **Option A — Run as a separate service**: Use `S3ServerBuilder` or `LocalServerBuilder` directly; minimal integration work, just deploy alongside gitserver and rustfs
2. **Option B — Embed as a library**: Import `rudolfs` as a crate dependency, construct the storage stack, and call `spawn_server(storage, &addr)`.
3. **Option C — Extract the storage layer**: The `storage` module and `lfs` module can be extracted and used within alknet's own HTTP handler (e.g., integrating LFS endpoints into the same Axum server as gitserver)

**Recommendation**: Option A is simplest for initial deployment. Option C is best for long-term integration — merge LFS endpoints into alknet's unified HTTP server so there's one service, one port, one auth layer.

### 11.4 LFS HTTP Endpoints → Alknet MessageInterface Mapping

| Rudolfs Endpoint | Alknet Mapping |
|---|---|
| `POST /api/{org}/{project}/objects/batch` | `http.handle("lfs.batch", org, project, batch_request)` |
| `GET /api/{org}/{project}/object/{oid}` | `http.handle("lfs.download", org, project, oid)` |
| `PUT /api/{org}/{project}/object/{oid}` | `http.handle("lfs.upload", org, project, oid)` |
| `POST /api/{org}/{project}/objects/verify` | `http.handle("lfs.verify", org, project, verify_request)` |
| `GET /` (index page) | Could be served via alknet's web UI or removed |

The namespace extraction (`{org}/{project}`) maps directly to alknet's namespace model. In the LFS URL format `http://gitlfs.example.com:8080/api/my-org/my-project`, the `my-org/my-project` path naturally aligns with alknet's org/project hierarchy.

### 11.5 Caching Layer: Alknet-Managed vs Rudolfs-Managed

**Current rudolfs approach**: Each rudolfs instance manages its own LRU cache in memory + local disk. There's no distributed cache coordination.

**Options for alknet**:

| Approach | Pros | Cons |
|---|---|---|
| **Let rudolfs manage its cache** | Simple, battle-tested, no coordination overhead | No cross-instance cache sharing; each node has its own cache |
| **Alknet-managed distributed cache** | Shared cache across nodes; better hit rates | Significant complexity; rudolfs' `Cached` is tightly coupled to its `Disk` backend |
| **Replace local cache with rustfs-backed cache** | Unified storage; no local disk dependency | Adds S3 round-trip latency for "cache" reads; defeats purpose of local cache |

**Recommendation**: Start with rudolfs-managed local caching. Each alknet node runs a local rudolfs instance with its own cache on local SSD. The LRU prepopulation is a no-op for S3 backend (since S3 `list()` returns empty), but caches warm up quickly. If cross-node cache sharing is needed later, consider a Redis/memcached metadata layer while keeping local disk for byte storage.

### 11.6 CredentialProvider::S3AccessKey → Rudolfs S3 Configuration

**rustfs** uses a `CredentialProvider` enum that includes `S3AccessKey`. The mapping to rudolfs:

| rustfs CredentialProvider | rudolfs Equivalent | How |
|---|---|---|
| `S3AccessKey { access_key, secret_key }` | `AWS_ACCESS_KEY_ID` + `AWS_SECRET_ACCESS_KEY` env vars | Set these in the environment before starting rudolfs |
| `Keystone { ... }` | Not directly supported | Would need custom credential provider for rudolfs |
| `OIDC { ... }` | Kubernetes WebIdentity | Already supported via `WebIdentityProvider::from_k8s_env()` |
| `IAMRole` | Default credential chain | Falls through to IAM instance profile |

**For integration with rustfs**: Set `AWS_S3_ENDPOINT=http://rustfs:9000` (or wherever rustfs listens) and `AWS_DEFAULT_REGION=us-east-1` along with the rustfs access credentials. This is exactly how the MinIO compose file works:

```yaml
environment:
  - AWS_S3_ENDPOINT=http://minio:9000
  - AWS_DEFAULT_REGION=us-east-1
  - AWS_ACCESS_KEY_ID=${RUSTFS_ACCESS_KEY}
  - AWS_SECRET_ACCESS_KEY=${RUSTFS_SECRET_KEY}
```

**Note**: rudolfs uses `rusoto` (not `aws-sdk-rust`), and `rusoto` is in maintenance mode. For long-term maintenance, migrating to `aws-sdk-rust` or a generic S3 client would be necessary. However, `rusoto` works well with rustfs/MinIO.

### 11.7 Encryption Considerations for Alknet

If rudolfs encryption is enabled:
- LFS objects are encrypted **before** they reach S3/rustfs
- This means **rustfs cannot deduplicate** across different encryption keys
- If the encryption key rotates, all existing objects become invalid (SHA256 verification fails)
- The nonce is derived from the OID, so same content → same encrypted form (no random IV)

**For alknet**: Consider whether encryption at the rudolfs layer is needed if rustfs already provides encryption at rest. Running encryption twice is wasteful. If rustfs provides server-side encryption (SSE), disable rudolfs `--key` and rely on rustfs's encryption instead.

### 11.8 Key Gaps and Limitations

| Gap | Impact | Mitigation |
|---|---|---|
| **No authentication** | Must be behind a trusted network or reverse proxy | Alknet provides auth at the MessageInterface layer |
| **No Locking API** | Cannot use `git lfs lock` | Would need to implement LFS Locking API |
| **No LFS listing API** | `list()` returns empty for S3 backend | S3 doesn't need listing for LFS protocol, but it prevents cache prepopulation |
| **No rate limiting** | Vulnerable to abuse | Add rate limiting middleware |
| **No multi-instance coordination** | Each instance has independent cache | Accept as-is for now; add Redis metadata layer later |
| **rusoto is unmaintained** | Long-term maintenance risk | Migrate to aws-sdk-rust or generic S3 client |
| **No content-type on responses** | `GET /object/{oid}` returns `application/octet-stream` | Fine for LFS; browsers wouldn't render binary |
| **No HTTPS/TLS** | Must use reverse proxy for TLS | Fine for deployment behind nginx/alknet |

### 11.9 Integration Blueprint

**Phase 1 — Sidecar Deployment**:

```yaml
# docker-compose.yml (development)
services:
  rustfs:
    image: rustfs:latest
    ports: ["9000:9000"]
    
  rudolfs:
    image: jasonwhite0/rudolfs:latest
    environment:
      - AWS_S3_ENDPOINT=http://rustfs:9000
      - AWS_DEFAULT_REGION=us-east-1
      - AWS_ACCESS_KEY_ID=minioadmin
      - AWS_SECRET_ACCESS_KEY=minioadmin
      - RUDOLFS_S3_BUCKET=lfs-data
    ports: ["8080:8080"]
    
  gitserver:
    image: gitserver:latest
    ports: ["8081:8080"]
```

**Phase 2 — Library Integration**:

```rust
// In alknet's git service module
use rudolfs::{S3ServerBuilder, Cache};

async fn start_lfs(config: &LfsConfig) -> Result<()> {
    let mut builder = S3ServerBuilder::new(config.s3_bucket.clone());
    builder.prefix(config.s3_prefix.clone());
    
    if let Some(key) = &config.encryption_key {
        builder.key(*key);
    }
    
    if let Some(cache) = &config.cache {
        builder.cache(Cache::new(cache.dir.clone(), cache.max_size));
    }
    
    builder.run(config.listen_addr).await?;
    Ok(())
}
```

**Phase 3 — Deep Integration**:

1. Extract `storage/` and `lfs.rs` from rudolfs
2. Integrate LFS endpoints into alknet's HTTP router (alongside gitserver's git endpoints)
3. Replace `rusoto` with a generic S3 client targeting rustfs
4. Add alknet Identity-based auth middleware
5. Implement LFS Locking API using alknet's operation registry

---

## 12. Test Coverage

### 12.1 Local Backend Tests

`tests/test_local.rs`: Exercises the local disk backend with encryption enabled and disabled. Creates temp git repos, pushes LFS objects (4MB, 8MB, 16MB), pulls them, clones repos, and verifies data integrity.

### 12.2 S3 Backend Tests

`tests/test_s3.rs`: Same test pattern as local but targets S3. Requires `tests/.test_credentials.toml` to run. Skips silently if credentials are missing.

### 12.3 Test Infrastructure

`tests/common/mod.rs` provides:
- `GitRepo` — temp directory with a git repo configured for LFS
- `GitRepo::init(addr)` — initializes repo, sets `lfs.url` to the test server
- `GitRepo::add_random(path, size, rng)` — creates random binary files
- `GitRepo::lfs_push()` / `lfs_pull()` — push/pull LFS objects
- `GitRepo::clean_lfs()` — clears `.git/lfs/` to force re-download
- `init_logger()` — sets up tracing for test output

---

## 13. Summary

Rudolfs is a well-architected, composable Git LFS server that maps cleanly onto alknet's requirements:

| Criterion | Assessment |
|---|---|
| **Protocol correctness** | Implements LFS batch API correctly; basic transfer adapter |
| **Storage composability** | Excellent — decorator pattern allows arbitrary stacking |
| **Caching** | Solid LRU + disk cache with background writes and corruption resilience |
| **Encryption** | XChaCha20 streaming, deterministic nonce from OID, optional |
| **S3 compatibility** | Works with any S3-compatible store (MinIO, rustfs) via `AWS_S3_ENDPOINT` |
| **Auth story** | No auth, but reflects headers — perfect for proxy-based auth |
| **Embeddability** | Good library API via `S3ServerBuilder`/`LocalServerBuilder` |
| **Maintenance risk** | `rusoto` is unmaintained; need migration path |
| **Missing features** | Locking API, listing API, rate limiting, multi-instance cache |

**Bottom line**: Rudolfs is a strong foundation for the LFS layer in the alknet git hosting stack. Its decorator pattern, streaming architecture, and S3 compatibility make it an excellent match for rustfs. The main gaps (auth, locking, rusoto maintenance) are well-understood and have clear integration paths within alknet.