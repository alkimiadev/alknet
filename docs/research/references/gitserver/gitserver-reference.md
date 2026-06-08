# Gitserver Reference Document

> **Source**: <https://github.com/WJQSERVER/gitserver> (cloned at `/workspace/gitserver/`)
> **Version**: 0.0.3 (workspace Cargo.toml)
> **License**: MPL-2.0 (primary); upstream portions MIT (preserved in UPSTREAM-LICENSE)
> **Upstream origin**: <https://github.com/ggueret/git-server>
> **Date researched**: 2026-06-08
> **Purpose**: Evaluate gitserver as a basis for a git service adapter within alknet

---

## 1. Architecture Overview

### 1.1 What is gitserver?

Gitserver is a **Rust-native Git Smart HTTP server** that does not require an installed `git` binary at runtime. All Git operations (ref advertisement, pack generation, receive-pack) are implemented via the [gitoxide](https://github.com/GitoxideLabs/gitoxide) (`gix`) crate. It supports both Git protocol v1 and v2, including shallow clones and multi-ack negotiation.

The project follows a **library-first design**: `gitserver-core` and `gitserver-http` are reusable libraries, while the `gitserver` binary is a thin CLI wrapper for standalone deployment.

### 1.2 Crate Structure

```
crates/
├── gitserver-core/     # Git protocol operations (no HTTP dependency)
│   ├── backend.rs      # GitBackend: unified interface for refs/pack/receive-pack
│   ├── discovery.rs     # RepoStore: filesystem-based repo discovery
│   ├── dynamic_registry.rs  # DynamicRepoRegistry, RepoResolver, MutableRepoRegistry traits
│   ├── error.rs        # Error types (RepoNotFound, PathTraversal, Protocol, Git, Io)
│   ├── pack.rs         # UploadPackRequest parsing, pack generation with side-band-64k
│   ├── path.rs         # Path safety: resolve_repo_path (normalize + canonicalize)
│   ├── pktline.rs      # pkt-line encoding/decoding utilities
│   ├── protocol_v2.rs  # Git protocol v2: ls-refs, fetch, shallow, stateless-rpc
│   ├── receive_pack.rs # receive-pack: ref advertisement, pack reception, fast-forward validation
│   └── refs.rs         # Protocol v1 ref advertisement
├── gitserver-http/     # Axum HTTP layer
│   ├── error.rs        # AppError enum → HTTP status codes
│   ├── handlers.rs     # Route handlers: info_refs, upload_pack, receive_pack, healthz, list
│   ├── lib.rs          # router() function + public re-exports
│   └── state.rs        # SharedState (RepoMode, AuthConfig, ServicePolicy, draining flag)
├── gitserver/          # CLI binary (thin wrapper)
│   └── main.rs         # CLI args, RepoStore discovery, Axum server, graceful shutdown
└── gitserver-bench/    # Performance benchmarks (not published)
```

### 1.3 Key Dependencies

| Dependency | Version | Purpose |
|---|---|---|
| `gix` | 0.80.0 | Native Git repository operations (open refs, object store, rev-walk) |
| `gix-pack` | 0.67.0 | Pack file writing (receive-pack) |
| `axum` | 0.8.8 | HTTP routing and handlers |
| `tokio` | 1.50.0 | Async runtime, channels, IO |
| `miniz_oxide` | 0.8 | Zlib compression for pack objects |
| `sha1` | 0.10 | Pack checksum |
| `flate2` | 1 | Gzip response compression |
| `zstd` | 0.13 | Zstd response compression |
| `base64` | 0.22 | HTTP Basic auth decoding |
| `subtle` | 2 | Constant-time comparison (auth) |
| `clap` | 4.6.0 | CLI argument parsing |

### 1.4 Request Flow

#### Clone/Fetch (Protocol v1)

```
Client → GET /{repo}/info/refs?service=git-upload-pack
       → Server: resolve repo, verify auth, advertise_refs()
       ← Ref advertisement response

Client → POST /{repo}/git-upload-pack
       → Server: parse UploadPackRequest, generate_pack()
       ← Streamed side-band-64k pack response
```

#### Clone/Fetch (Protocol v2)

```
Client → GET /{repo}/info/refs (git-protocol: version=2)
       ← Capabilities advertisement

Client → POST /{repo}/git-upload-pack (git-protocol: version=2)
       → Server: parse_command_request() → ls-refs or fetch
       ← ls-refs result or streamed packfile
```

#### Push (receive-pack, must be enabled)

```
Client → GET /{repo}/info/refs?service=git-receive-pack
       ← Ref advertisement

Client → POST /{repo}/git-receive-pack
       → Server: parse commands, write pack, validate fast-forward, update refs
       ← Status report (ok/ng per ref)
```

---

## 2. Protocol Support

### 2.1 Smart HTTP Git Protocol

Gitserver implements the **Git Smart HTTP protocol** (RFC-like, de facto standard). This is the standard protocol used by `git clone http://...`, `git fetch`, and `git push` over HTTP.

**Supported endpoints:**

| Method | Endpoint | Protocol Version | Description |
|---|---|---|---|
| GET | `/healthz` | — | Health check (no auth) |
| GET | `/` | — | JSON repository listing (auth required if configured) |
| GET | `/{repo}/info/refs?service=git-upload-pack` | v1 | Ref advertisement for clone/fetch |
| GET | `/{repo}/info/refs?service=git-receive-pack` | v1 | Ref advertisement for push (disabled by default) |
| POST | `/{repo}/git-upload-pack` | v1 | Pack negotiation and transfer |
| POST | `/{repo}/git-receive-pack` | v1 | Push operations (disabled by default) |
| GET | `/{repo}/info/refs` with `git-protocol: version=2` | v2 | Capabilities advertisement |
| POST | `/{repo}/git-upload-pack` with `git-protocol: version=2` | v2 | `ls-refs` and `fetch` commands |

### 2.2 Git Operations

| Operation | Supported | Notes |
|---|---|---|
| `git clone` | ✓ | Both v1 and v2 |
| `git fetch` | ✓ | Multi-ack, multi-ack-detailed negotiation |
| `git push` | ✓ (opt-in) | Via `--enable-receive-pack` or `ServicePolicy.receive_pack: true` |
| Shallow clone | ✓ | Protocol v2 `fetch` with `deepen` |
| OFS_DELTA | ✓ | Offset delta compression in packs |
| Side-band-64k | ✓ | Multiplexed progress/pack data |
| Response compression | ✓ | Gzip and Zstd on ref advertisement |

### 2.3 Push Restrictions

When receive-pack is enabled, the following restrictions apply:

- **Fast-forward only**: Branch updates under `refs/heads/*` must be fast-forward (old commit is ancestor of new)
- **No ref deletion**: New OID cannot be the zero OID
- **No tag overwrite**: Updating an existing tag is rejected
- **Commits only**: Branch tips must point to commit objects
- **Timeouts**: 300s total, 30s idle

### 2.4 SSH Git Protocol

Gitserver does **not** support SSH Git protocol. It is HTTP-only. SSH git access would require a separate implementation or integration layer (see Section 6).

---

## 3. Interface Pattern Analysis

### 3.1 HTTP Handler Architecture

Gitserver's HTTP layer follows a clean handler pattern:

```rust
// gitserver-http/src/lib.rs
pub fn router(state: SharedState) -> Router {
    Router::new()
        .route("/healthz", get(handlers::healthz))
        .route("/", get(handlers::list_repos))
        .route("/{*path}", get(handlers::info_refs_dispatch))
        .route("/{*path}", post(handlers::rpc_dispatch))
        .with_state(state)
}
```

The `SharedState` is an Axum state object containing:
- `RepoMode` — either `Discovered(Arc<RwLock<RepoStore>>)` or `Dynamic { resolver, registry }`
- `AuthConfig` — optional Basic and/or Bearer authentication
- `ServicePolicy` — toggle for upload_pack, upload_pack_v2, receive_pack
- `draining: Arc<AtomicBool>` — graceful shutdown flag

Each handler follows this pattern:
1. Check `draining` flag → 503 if shutting down
2. Check `ServicePolicy` → 404 if service disabled
3. Authenticate request via `require_auth()` → 401 if credentials missing/invalid
4. Resolve repository via `SharedState::resolve()` → 404 if not found
5. Execute git operation via `GitBackend`
6. Return streaming or buffered response

### 3.2 Mapping to alknet's MessageInterface

Gitserver's `SharedState` + handler pattern maps closely to alknet's proposed `MessageInterface` trait:

```rust
// alknet's proposed MessageInterface
async fn handle_request(&self, request: InterfaceRequest) -> Result<InterfaceResponse>;
```

Gitserver's handler flow is essentially:
1. Receive HTTP request (analogous to `InterfaceRequest`)
2. Extract operation path, auth, and body
3. Dispatch to the appropriate Git operation
4. Return HTTP response (analogous to `InterfaceResponse`)

### 3.3 Low-Level Handler API

Gitserver also exposes handler functions that can be called directly without going through the Axum router:

```rust
use gitserver_http::handlers::{info_refs_endpoint, ServiceKind};

let response = info_refs_endpoint(
    &state,
    "my-project.git",
    ServiceKind::UploadPack,
    HeaderMap::new(),
).await?;
```

This is significant for alknet integration — it means the git logic can be invoked programmatically without HTTP routing.

---

## 4. Authentication

### 4.1 Current Auth Model

Gitserver supports two HTTP authentication mechanisms, both optional:

```rust
pub struct AuthConfig {
    pub basic: Option<BasicAuthConfig>,
    pub bearer_token: Option<String>,
}

pub struct BasicAuthConfig {
    pub username: String,
    pub password: String,
}
```

**Key characteristics:**
- Both can be configured simultaneously; **either one passing is sufficient**
- Basic auth uses **constant-time comparison** (`subtle` crate) to prevent timing attacks
- Bearer token is compared directly (suitable for generated tokens)
- Failed auth returns `401 Unauthorized` with `WWW-Authenticate: Basic realm="gitserver", Bearer`
- `GET /healthz` is **unauthenticated** (always accessible)
- Auth is **global** (same credentials for all repositories) — no per-repo or per-user ACL

### 4.2 Auth Flow in Handlers

```rust
fn require_auth(store: &SharedState, headers: &HeaderMap) -> Result<(), AppError> {
    let auth = store.auth();
    if auth.basic.is_none() && auth.bearer_token.is_none() {
        return Ok(());  // No auth configured → allow all
    }
    let value = headers.get(AUTHORIZATION)...;
    // Try Bearer first, then Basic
    // Constant-time comparison for Basic
}
```

### 4.3 Mapping to alknet Identity

alknet's `IdentityProvider` resolves credentials to an `Identity`. The mapping would be:

| gitserver auth | alknet equivalent | Resolution path |
|---|---|---|
| No auth | `Identity::anonymous()` or reject | Configurable policy |
| Basic auth (username/password) | `IdentityProvider::resolve_from_token()` | Map to AuthToken or direct lookup |
| Bearer token | `IdentityProvider::resolve_from_token()` | Token is already in the right format |

The key gap is that gitserver's auth is **single-credential, global**, while alknet needs **per-identity, per-repository** access control. Integration would require:

1. Replacing `AuthConfig` with alknet's `IdentityProvider`
2. Extracting identity from the `Authorization` header
3. Checking per-repo ACL based on resolved `Identity`

---

## 5. Storage

### 5.1 Filesystem-Based Storage

Gitserver currently stores repositories as **bare Git repositories on the local filesystem**. The storage model is:

```
ROOT/
├── project-a.git/       # bare repository
│   ├── HEAD
│   ├── objects/
│   ├── refs/
│   └── description
├── org/
│   └── project-b.git/   # nested repository (up to max_depth)
└── ...
```

The `RepoStore::discover(root, max_depth)` function:
1. Canonicalizes the root path
2. Recursively walks subdirectories up to `max_depth`
3. Attempts `gix::open(path)` on each directory
4. If `repo.is_bare()`, adds it as a `RepoInfo`
5. Path traversal protection via lexical normalization + `canonicalize()` double-check

The `DynamicRepoRegistry` allows programmatic registration/unregistration of repos at runtime, validated by `gix::open()` confirming the path is a bare repo.

### 5.2 Storage Abstraction Points

The key storage interaction points in the codebase are:

| Component | Storage Pattern |
|---|---|
| `RepoStore::discover()` | Filesystem scan (local directory tree) |
| `DynamicRepoRegistry` | In-memory registry with filesystem-backed paths |
| `GitBackend::new(repo_path)` | Opens a local bare repo via `gix::open()` |
| `receive_pack::write_pack()` | Writes pack to `objects/pack/` via `gix_pack::Bundle::write_to_directory()` |
| `path::resolve_repo_path()` | Canonical path resolution + traversal protection |

**All storage operations assume a local filesystem path.** There is no abstraction for remote or object storage backends.

### 5.3 Rustfs (S3-Compatible) Integration Feasibility

Git operations fundamentally require **a local filesystem** — `gix::open()` expects a directory with the standard `.git` layout (objects, refs, HEAD, etc.). Rustfs (S3-compatible) cannot serve as a **direct** storage backend for gitoxide's repository operations because:

1. `gix::open()` requires a local path — it reads `HEAD`, refs, and object packs from the filesystem
2. Pack generation (`generate_pack()`) streams objects from the local ODB
3. Receive-pack writes pack files to the local `objects/pack/` directory
4. Reference updates use `gix::Repository::edit_references()` which operates on the local refstore

However, rustfs **could** be used in several supporting roles:

| Integration Approach | Description | Feasibility |
|---|---|---|
| **Repo sync backend** | Store bare repo tarballs in rustfs; sync to local disk on demand | High — sync from S3 to local FS before serving |
| **Backup/archive** | Push repo backups to rustfs buckets | High — out-of-band backup |
| **Git LFS storage** | Store large file objects in rustfs via Git LFS | Medium — requires LFS server implementation |
| **Object store proxy** | Cache layer: serve from local FS, sync to/from rustfs | Medium — needs repo lifecycle management |
| **Direct S3 repo** | Custom `gix` object backend reading from S3 | Low — would require deep gitoxide customization |

The most practical approach: **use rustfs as a backing store for repository synchronization**. Gitserver would always operate on local filesystem paths, but a separate component would manage syncing repos to/from rustfs buckets.

---

## 6. SSH Support

### 6.1 Current State

Gitserver has **no SSH transport capability**. It only implements the HTTP Smart Git protocol. Adding SSH support would require implementing the Git SSH protocol, which is a different wire format:

| Aspect | Smart HTTP | SSH |
|---|---|---|
| Transport | HTTP (request/response) | Persistent SSH channel |
| Service discovery | `GET /info/refs?service=git-upload-pack` | `ssh://host/git-upload-pack 'repo'` |
| Protocol framing | pkt-line over HTTP | pkt-line over SSH channel |
| Authentication | HTTP Authorization header | SSH key-based |
| Multiplexing | HTTP/2 or separate connections | Multiple SSH channels |

### 6.2 How Git over SSH Works

The Git SSH protocol uses SSH as a transport for the same `git-upload-pack` and `git-receive-pack` commands:

```
Client connects via SSH → server executes git-upload-pack or git-receive-pack
Client ← SSH channel → Server (bidirectional pkt-line stream)
```

### 6.3 Integration with alknet's SSH Interface

alknet's SSH interface (`SshInterface`) is a `StreamInterface` — it accepts a persistent byte stream and multiplexes it into channels. This maps naturally to Git over SSH:

**Approach: Git as an alknet operation over SSH**

```
 alknet SSH session
      │
      ├─ Channel: call protocol (operations)
      │
      └─ Channel: git-upload-pack
           OR git-receive-pack
               │
               ▼
         gitserver-core protocol logic
         (ref advertisement, pack generation, receive-pack)
```

This would work by:

1. The SSH interface receives a connection with a request like `git-upload-pack '/repos/project.git'`
2. alknet resolves the identity from the SSH key fingerprint
3. Checks ACL: does this identity have read/write access to this repo?
4. Invokes `gitserver-core` functions directly (no HTTP needed):
   - `refs::advertise_refs()` → send over SSH channel
   - `pack::generate_pack()` → stream over SSH channel
   - `receive_pack::receive_pack()` → read/write over SSH channel

**Key advantage**: Since `gitserver-core` has no HTTP dependency, it can be used directly over SSH channels without the HTTP overhead. The `GitBackend` API is transport-agnostic.

### 6.4 Alternative: Dedicated Git SSH Adapter

A simpler approach that doesn't require modifying the SSH channel multiplexing:

```
alknet SSH session → call protocol → operation "git/upload-pack" → 
  → GitAdapter::upload_pack(repo, wants, haves) → streaming response
```

This treats Git operations as alknet call operations, where the SSH interface is the transport but Git operations are invoked via the call protocol rather than raw SSH channels. This is more aligned with alknet's architecture but requires adapting the Git protocol to the call protocol's request/response model (potentially with streaming).

---

## 7. Relevance to alknet

### 7.1 Mapping to alknet's Interface Model

Gitserver is a textbook **`MessageInterface`** implementation:

| alknet MessageInterface | Gitserver Equivalent |
|---|---|
| `handle_request(InterfaceRequest)` | `info_refs_dispatch()` / `rpc_dispatch()` |
| `InterfaceRequest.operation_path` | URL path (`/{repo}/info/refs`, `/{repo}/git-upload-pack`) |
| `InterfaceRequest.auth_token` | `Authorization` header → `require_auth()` |
| `InterfaceRequest.input` | Request body (pack negotiation data) |
| `InterfaceResponse.result` | HTTP response body (ref advertisement, pack data) |
| `InterfaceResponse.status` | HTTP status code |
| `InterfaceResponse.headers` | Content-Type, Cache-Control, etc. |

However, gitserver **manages its own transport** (Axum HTTP server), which is exactly the `MessageInterface` pattern described in alknet's interface model: "MessageInterface implementations manage their own transport. They don't need the Transport trait because they're not wrapping a generic byte stream — they ARE the transport+interface combined."

### 7.2 Git as an alknet Operation

Git operations could be mapped to alknet's call protocol namespace:

```
Namespace: "git"
Operations:
  - git/list          → List available repositories
  - git/info-refs     → Get ref advertisement for a repo
  - git/upload-pack   → Clone/fetch (streaming response)
  - git/receive-pack  → Push (streaming request+response)
  - git/ls-refs       → Protocol v2 ls-refs
  - git/fetch         → Protocol v2 fetch
```

**Challenge**: Git operations are **streaming and bidirectional** (especially fetch negotiation and receive-pack), while alknet's call protocol is currently defined as request→response. This needs design consideration:

| Operation | Direction | Stream Duration | alknet Fit |
|---|---|---|---|
| `git/list` | Request → Response | Short | Direct fit |
| `git/info-refs` | Request → Response | Short | Direct fit |
| `git/upload-pack` | Request → Streaming Response | Long | Needs streaming response support |
| `git/receive-pack` | Streaming Request → Streaming Response | Long | Needs bidirectional streaming |

### 7.3 Proposed GitAdapter Architecture

```
┌─────────────────────────────────────────────────────────┐
│                     alknet node                          │
│                                                         │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐  │
│  │  HttpInterface│  │ SshInterface │  │  DNS/other   │  │
│  │  (Message)    │  │  (Stream)    │  │  (Message)   │  │
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘  │
│         │                  │                  │          │
│         ▼                  ▼                  ▼          │
│  ┌──────────────────────────────────────────────────┐   │
│  │              OperationRegistry                    │   │
│  │  "git/list"  → GitAdapter::list_repos()         │   │
│  │  "git/upload-pack" → GitAdapter::upload_pack()   │   │
│  │  "git/receive-pack" → GitAdapter::receive_pack()  │   │
│  └──────────────┬───────────────────────────────────┘   │
│                 │                                        │
│                 ▼                                        │
│  ┌──────────────────────────────┐                       │
│  │         GitAdapter            │                       │
│  │  - SharedState (repos, auth)  │                       │
│  │  - GitBackend (protocol ops)  │                       │
│  │  - IdentityProvider (auth)    │                       │
│  │  - RepoResolver (filesystem)  │                       │
│  └──────────────┬───────────────┘                       │
│                 │                                        │
│                 ▼                                        │
│  ┌──────────────────────────────┐   ┌────────────────┐ │
│  │    Local filesystem           │   │  Rustfs sync   │ │
│  │    (bare git repos)           │   │  (S3 backend)  │ │
│  └──────────────────────────────┘   └────────────────┘ │
└─────────────────────────────────────────────────────────┘
```

### 7.4 Auth Integration: alknet Identity → Gitserver Auth

**Current gitserver auth** (single global credential):
```rust
AuthConfig {
    basic: Option<BasicAuthConfig>,   // one username/password
    bearer_token: Option<String>,     // one token
}
```

**Proposed alknet integration** (per-identity, per-repo):
```rust
struct GitAdapter {
    identity_provider: Arc<dyn IdentityProvider>,
    repo_resolver: Arc<dyn RepoResolver>,
    backend_factory: Arc<dyn GitBackendFactory>,
    acl: Arc<dyn GitAcl>,
}

impl GitAdapter {
    async fn handle_request(
        &self,
        request: InterfaceRequest,
    ) -> Result<InterfaceResponse> {
        // 1. Resolve identity from auth token
        let identity = self.identity_provider
            .resolve_from_token(request.auth_token)?;
        
        // 2. Parse git operation from path
        let operation = parse_git_operation(&request.operation_path)?;
        
        // 3. Check ACL
        self.acl.check_access(&identity, &operation.repo, operation.access_type)?;
        
        // 4. Dispatch to gitserver-core logic
        // ...
    }
}
```

**ACL design** (per-repo, per-operation):
```rust
enum GitAccess {
    Read,      // clone, fetch
    Write,     // push
}

trait GitAcl: Send + Sync {
    fn check_access(
        &self,
        identity: &Identity,
        repo: &str,
        access: GitAccess,
    ) -> Result<()>;
}
```

### 7.5 Storage Integration with Rustfs

**Recommended approach**: Rustfs as a sync backend:

```rust
trait RepoStorage: Send + Sync {
    /// Ensure a local working copy exists for the given repo.
    /// May involve syncing from S3 (rustfs) to local disk.
    async fn ensure_local(&self, repo: &str) -> Result<PathBuf>;
    
    /// Sync local changes back to S3 (rustfs) after a push.
    async fn sync_to_remote(&self, repo: &str) -> Result<()>;
    
    /// List available repos (may consult S3 bucket listing).
    async fn list_repos(&self) -> Result<Vec<RepoInfo>>;
}
```

The flow would be:
1. `GitAdapter` receives a request for repo `X`
2. `RepoStorage::ensure_local("X")` checks if the repo exists on local disk; if not, syncs from rustfs
3. Git operations run on the local filesystem (using `gitserver-core` directly)
4. After push operations, `RepoStorage::sync_to_remote("X")` pushes updates to rustfs

This maintains gitserver's requirement for a local filesystem while leveraging rustfs for durability and distribution.

### 7.6 Operation Mapping

| Git Operation | alknet Namespace | alknet Op | Input | Output | Stream? |
|---|---|---|---|---|---|
| List repos | `git` | `list` | `{}` | `[RepoInfo]` | No |
| Ref advertisement (v1) | `git` | `info-refs` | `{repo, service: "upload-pack" \| "receive-pack"}` | Binary ref advertisement | No |
| Ref capabilities (v2) | `git` | `capabilities` | `{repo}` | Binary capabilities | No |
| Ls-refs (v2) | `git` | `ls-refs` | `{repo, peel, symrefs, ref_prefixes}` | Binary ref listing | No |
| Clone/Fetch | `git` | `upload-pack` | `{repo, wants, haves, done, ...}` | Streamed pack data | Yes (response) |
| Push | `git` | `receive-pack` | `{repo, commands, pack_data}` | Status report | Yes (both) |

### 7.7 What gitserver-core Provides Directly

The most valuable integration point is `gitserver-core` — the HTTP-free protocol library:

```rust
// Direct usage without HTTP
use gitserver_core::backend::GitBackend;
use gitserver_core::discovery::RepoStore;
use gitserver_core::pack::{UploadPackRequest, UploadPackCapabilities, ShallowRequest};
use gitserver_core::protocol_v2;

// Repository discovery
let store = RepoStore::discover("./repos".into(), 3)?;
let repo = store.resolve("my-project.git")?;

// Protocol v1 ref advertisement
let backend = GitBackend::new(repo.absolute_path.clone());
let refs = backend.advertise_refs()?;

// Pack generation (streaming)
let request = UploadPackRequest { wants, haves, done, ... };
let pack_stream = backend.upload_pack(&request).await?;

// Receive-pack (push)
let result = backend.receive_pack(request_stream).await?;

// Protocol v2
let capabilities = protocol_v2::advertise_capabilities();
let ls_refs_output = protocol_v2::ls_refs(&repo_path, &ls_refs_request)?;
let fetch_output = backend.upload_pack(&fetch_request.upload_request).await?;
```

These functions can be called from any async context — SSH channel handler, alknet operation handler, HTTP handler — without going through the Axum HTTP layer.

---

## 8. Integration Recommendations

### 8.1 Recommended Integration Strategy

**Phase 1: HTTP Gateway (MessageInterface)**

Embed gitserver-http's Axum router into alknet's HTTP interface. This provides immediate Git-over-HTTP capability:

```rust
// In alknet's HttpInterface::handle_request()
// Route: /git/* → gitserver router
let git_app = gitserver_http::router(git_state);
let app = Router::new()
    .nest("/git", git_app)  // Mount git under /git
    .route("/v1/{namespace}/{op}", post(operation_handler));
```

This works because gitserver is designed to be nested into existing Axum apps. Auth integration would replace `AuthConfig` with alknet's `IdentityProvider`.

**Phase 2: SSH Git Adapter (StreamInterface)**

Use `gitserver-core` directly within alknet's SSH interface for Git-over-SSH:

```rust
// In alknet's SshInterface channel handler
// SSH channel request: "git-upload-pack '/repos/project.git'"
let backend = GitBackend::new(repo_path);
let refs = backend.advertise_refs()?;
// Send refs over SSH channel
// Stream pack data over SSH channel
```

**Phase 3: Call Protocol Operations (OperationRegistry)**

Register Git operations in the operation registry for access via any interface:

```rust
registry.register(GitListRepos::new(adapter.clone()));
registry.register(GitUploadPack::new(adapter.clone()));
registry.register(GitReceivePack::new(adapter.clone()));
```

### 8.2 Key Modifications Needed

1. **Auth replacement**: Replace `AuthConfig` with `IdentityProvider`-based auth in `handlers.rs`'s `require_auth()` function
2. **ACL addition**: Add per-repo, per-identity access control (gitserver currently has none)
3. **RepoResolver abstraction**: Replace `RepoStore`/`DynamicRepoRegistry` with alknet's `RepoResolver` that integrates with rustfs sync
4. **Streaming response support**: Adapt alknet's call protocol for streaming (large pack files)
5. **Bidirectional streaming**: For receive-pack, the call protocol needs to support bidirectional streaming

### 8.3 Risks and Mitigations

| Risk | Mitigation |
|---|---|
| gitserver requires local filesystem | Use rustfs as sync backend; maintain local working copies |
| Auth is global (single credential) | Fork/modify `require_auth()` to use `IdentityProvider` |
| No per-repo ACL | Add `GitAcl` trait in the adapter layer |
| MPL-2.0 license requires modifications to be under MPL-2.0 | Acceptable for alknet (MPL-2.0 is file-level copyleft) |
| Large pack files may not fit alknet's message size limits | Implement streaming response in the call protocol |
| gitoxide version coupling | Pin `gix = "0.80.0"` as gitserver does |

### 8.4 License Considerations

- **Primary license**: MPL-2.0 (file-level copyleft)
- **Upstream portions**: MIT (preserved in UPSTREAM-LICENSE)
- **Implication**: Modifications to gitserver's `.rs` files must remain under MPL-2.0. Linking from alknet code is unrestricted.
- **Recommendation**: Use gitserver as a library dependency. If alknet-specific auth/ACL modifications are needed, contribute them upstream or maintain them as separate files under MPL-2.0.

---

## 9. Summary

### 9.1 Key Findings

1. **gitserver is a well-structured, library-first Rust Git Smart HTTP server** with clean separation between protocol logic (`gitserver-core`) and HTTP transport (`gitserver-http`).
2. **Protocol support is comprehensive**: Git Smart HTTP v1 and v2, clone, fetch, push (opt-in), shallow clones, delta compression, streaming pack generation.
3. **No SSH support exists**, but `gitserver-core` is transport-agnostic and can serve Git operations over any channel.
4. **Auth is simple but limited**: single global Basic/Bearer credential, no per-repo or per-user ACL.
5. **Storage is local-filesystem only**: `gix::open()` requires a local path. S3/rustfs integration requires a sync-to-local approach.
6. **The library design enables direct integration**: `GitBackend` and protocol functions can be called without HTTP.

### 9.2 Recommendation

**Use `gitserver-core` as alknet's Git protocol engine.** The core crate provides all Git protocol operations (ref advertisement, pack generation, receive-pack, protocol v2) without any HTTP dependency. This allows alknet to expose Git services through any interface (HTTP, SSH, call protocol) while maintaining a single protocol implementation.

**Use `gitserver-http` as alknet's Git HTTP interface** by nesting its Axum router under alknet's HTTP interface, with auth replaced by `IdentityProvider`.

**Design a `GitAdapter`** that wraps `gitserver-core` and integrates with alknet's `OperationRegistry`, `IdentityProvider`, and rustfs-backed storage.

### 9.3 Next Steps

1. Fork or vendor `gitserver-core` and `gitserver-http` into alknet's dependency tree
2. Design the `GitAdapter` trait with `IdentityProvider` auth and `GitAcl` access control
3. Implement Phase 1: HTTP gateway with nested Axum router and `IdentityProvider` auth
4. Implement `RepoStorage` trait with rustfs sync-to-local strategy
5. Design streaming extensions to alknet's call protocol for pack file transfer
6. Evaluate Phase 2: SSH Git adapter using `gitserver-core` directly over SSH channels

---

## References

- [gitserver README](https://github.com/WJQSERVER/gitserver) — project overview, quick start, CLI usage
- [gitserver Architecture docs](docs/en/architecture.md) — crate responsibilities, request flows
- [gitserver Library docs](docs/en/library.md) — embedding, dynamic registration, auth config
- [gitserver API Reference](docs/en/api.md) — REST endpoints, protocol details, error codes
- [alknet Interface Model](../../phase2/interface-model.md) — StreamInterface/MessageInterface design
- [gitoxide](https://github.com/GitoxideLabs/gitoxide) — underlying Git implementation library