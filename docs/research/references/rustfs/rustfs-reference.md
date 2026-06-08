# RustFS Reference Document

> Status: Research Complete
> Last updated: 2026-06-08
> Source: /workspace/rustfs/ (cloned repository, v1.0.0-beta.7)
> Context: alknet internal service integration research

---

## 1. Architecture Overview

### What is RustFS?

RustFS is a high-performance, distributed, S3-compatible object storage system written in Rust. It is an Apache 2.0-licensed alternative to MinIO that combines S3 API compatibility with OpenStack Swift/Keystone support, designed for data lake, AI, and big data workloads.

**Key characteristics:**
- Language: Rust (edition 2024, MSRV 1.95.0)
- License: Apache 2.0 (no AGPL restrictions)
- Workspace: 57 crates in a flat `crates/` layout
- Main binary: `rustfs/` (75K lines); core engine: `crates/ecstore/` (87K lines)
- Version: 1.0.0-beta.7

### Ports and Endpoints

| Port | Purpose |
|------|---------|
| 9000 | S3 API (primary data path) + Admin API (`/minio/` prefix) |
| 9001 | Web Console UI |

### Request Flow

```
HTTP request
  → server (TLS, auth, routing, compression)
    → app/object_usecase (validation, policy, lifecycle)
      → storage/ecfs (erasure coding, encryption, checksums)
        → ecstore (disk pool selection, data distribution)
          → rio (reader pipeline: encrypt → compress → hash → write)
            → io-core (zero-copy I/O, buffer pool, direct I/O)
              → local disk / remote disk via RPC
```

### Key Crate Map (Security & Auth Focus)

| Crate | Lines | Purpose |
|-------|-------|---------|
| `credentials` | 713 | Credential types (access key / secret key), global credentials |
| `signer` | 1.4K | AWS Signature V4 request signing |
| `iam` | 9.0K | Identity and Access Management (users, groups, policies, OIDC) |
| `policy` | 8.8K | S3 bucket/IAM policy engine |
| `keystone` | 1.9K | OpenStack Keystone auth integration |
| `appauth` | 143 | Application-level auth tokens |
| `crypto` | 1.6K | Encryption primitives |
| `kms` | 8.1K | Key management service integration |
| `protocols` | 18K | FTP/FTPS, WebDAV, Swift API support |
| `s3-ops` | — | S3 operation definitions and mapping |
| `s3-types` | — | S3 event type definitions |

### Startup Sequence (Auth-Relevant Steps)

1. Environment variable compatibility (`MINIO_*` → `RUSTFS_*`)
2. Tokio runtime construction
3. CLI argument parsing
4. Config parsing, credentials/endpoints initialization
5. HTTP server start (S3 API + optional console)
6. ECStore initialization
7. **Steps 13: Bucket metadata, IAM, Keystone, OIDC** initialization
8. FullReady → serving requests

---

## 2. S3 API Compatibility

### Supported S3 Operations

RustFS implements a substantial subset of the S3 API via the `s3s` crate (a fork/custom build at `https://github.com/rustfs/s3s`). Based on the feature status table and crate structure:

| Category | Status | Details |
|----------|--------|---------|
| Core Object Ops (GET/PUT/DELETE/HEAD) | ✅ Available | Primary data path |
| Multipart Upload | ✅ Available | Upload, download, multipart |
| Versioning | ✅ Available | Object versioning |
| Bucket Operations | ✅ Available | Create, list, delete, metadata |
| Logging | ✅ Available | Access logging |
| Event Notifications | ✅ Available | Webhook, Kafka, AMQP, MQTT, NATS targets |
| Bitrot Protection | ✅ Available | Checskums at storage layer |
| Single Node Mode | ✅ Available | Single-node deployment |
| Bucket Replication | ✅ Available | Cross-region replication |
| KMS | 🚧 Under Testing | Key management service |
| Lifecycle Management | 🚧 Under Testing | Object lifecycle rules |
| Distributed Mode | 🚧 Under Testing | Multi-node erasure coding |
| Admin API | ✅ Available | `/minio/` prefix, 30+ handler modules |
| Console | ✅ Available | Web UI on port 9001 |
| S3 Select | ✅ Available | `s3select-api` + `s3select-query` crates |
| WebDAV | ✅ Available | `protocols` crate, `dav-server` |
| FTP/FTPS | ✅ Available | `libunftp`, `suppaftp` |
| SFTP | — | `russh` + `russh-sftp` crate deps |

### Authentication Methods

RustFS supports multiple authentication methods (derived from `auth.rs`):

| Auth Type | Constant | Detection |
|-----------|----------|-----------|
| AWS Signature V4 (header) | `Signed` | `Authorization: AWS4-HMAC-SHA256 ...` |
| AWS Signature V4 (query) | `Presigned` | `X-Amz-Credential` in query |
| AWS Signature V2 (header) | `SignedV2` | `Authorization: AWS ...` |
| AWS Signature V2 (query) | `PresignedV2` | `AWSAccessKeyId` in query |
| Streaming V4 | `StreamingSigned` | `x-amz-content-sha256: STREAMING-AWS4-HMAC-SHA256-PAYLOAD` |
| Streaming V4 (trailer) | `StreamingSignedTrailer` | `STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER` |
| Unsigned payload (trailer) | `StreamingUnsignedTrailer` | `STREAMING-UNSIGNED-PAYLOAD-TRAILER` |
| POST policy | `PostPolicy` | `multipart/form-data` content type |
| Bearer JWT | `JWT` | `Authorization: Bearer ...` |
| STS | `STS` | `Action` header presence |
| Anonymous | `Anonymous` | No `Authorization` header |
| Keystone token | — | `X-Auth-Token` header (via middleware) |

### S3 Request Signing

The `rustfs-signer` crate implements AWS Signature V4. The general flow:

1. Client computes a canonical request (method + path + query + headers + payload hash)
2. Client creates a string to sign (algorithm + timestamp + credential scope + canonical request hash)
3. Client computes HMAC-SHA256 signature using the secret key
4. Client sends the `Authorization` header with the signature

---

## 3. OpenStack Swift and Keystone Integration

### Swift API

RustFS provides an **OpenStack Swift-compatible API** as an opt-in feature (behind the `swift` cargo feature flag). This is implemented in `crates/protocols/src/swift/`.

**Swift API endpoint pattern:** `/v1/AUTH_{project_id}/...`

**Supported Swift operations:**
- Container CRUD (create, list, delete, metadata)
- Object CRUD with streaming downloads
- Keystone token authentication
- Multi-tenant isolation with SHA256-based bucket prefixing
- Server-side object copy (COPY method)
- HTTP Range requests (206/416 responses)
- Custom metadata (X-Object-Meta-*, X-Container-Meta-*)

**Not yet implemented:** Account-level ops, large object support (>5GB), object versioning, container ACLs/CORS, TempURL, XML/plain-text response formats.

**Tenant isolation:** Swift containers are mapped to S3 buckets with a secure hash prefix:
```
Swift: /v1/AUTH_abc123/mycontainer
  → S3 Bucket: {sha256(abc123)[0:16]}-mycontainer
```

### Keystone Authentication — Complete Flow

This is the most auth-relevant subsystem for alknet integration.

#### Configuration (Environment Variables)

| Variable | Description | Default |
|----------|-------------|---------|
| `RUSTFS_KEYSTONE_ENABLE` | Enable Keystone auth | `false` |
| `RUSTFS_KEYSTONE_AUTH_URL` | Keystone endpoint URL | (required) |
| `RUSTFS_KEYSTONE_VERSION` | API version (`v3` or `v2.0`) | `v3` |
| `RUSTFS_KEYSTONE_ADMIN_USER` | Admin username | (optional) |
| `RUSTFS_KEYSTONE_ADMIN_PASSWORD` | Admin password | (optional) |
| `RUSTFS_KEYSTONE_ADMIN_PROJECT` | Admin project/tenant | (optional) |
| `RUSTFS_KEYSTONE_ADMIN_DOMAIN` | Admin domain | `Default` |
| `RUSTFS_KEYSTONE_VERIFY_SSL` | Verify TLS certificates | `true` |
| `RUSTFS_KEYSTONE_ENABLE_CACHE` | Enable token caching | `true` |
| `RUSTFS_KEYSTONE_CACHE_SIZE` | Token cache capacity | `10000` |
| `RUSTFS_KEYSTONE_CACHE_TTL` | Token cache TTL (seconds) | `300` |
| `RUSTFS_KEYSTONE_TENANT_PREFIX` | Enable tenant project prefixing | `true` |
| `RUSTFS_KEYSTONE_IMPLICIT_TENANTS` | Auto-create tenants | `true` |
| `RUSTFS_KEYSTONE_TIMEOUT` | Request timeout (seconds) | `30` |

#### Architecture: Component Stack

```
KeystoneClient (HTTP calls to Keystone v3 API)
    ↓
KeystoneAuthProvider (Authentication + Caching via moka::future::Cache)
    ↓
KeystoneAuthMiddleware (Tower layer, intercepts HTTP requests)
    ↓ (task-local: KEYSTONE_CREDENTIALS)
IAMAuth → check_key_valid (Authorization)
    ↓
RustFS Credentials (access_key starts with "keystone:")
```

#### Authentication Flow

**Request with `X-Auth-Token` header:**

1. **Middleware intercepts:** `KeystoneAuthMiddleware` extracts `X-Auth-Token` header
2. **Cache check:** Token cache hit → return cached credentials (~1-2ms)
3. **Token validation:** Cache miss → `KeystoneClient.validate_token()` → `GET /v3/auth/tokens` with `X-Auth-Token` and `X-Subject-Token` headers
4. **Token parsing:** Parse `KeystoneToken` (user_id, username, project_id, project_name, domain, roles, expires_at)
5. **Credential mapping:** Convert to `Credentials` struct:
   - `access_key`: `keystone:<user_id>` (special prefix identifies Keystone users)
   - `secret_key`: `""` (empty — bypasses AWS SigV4 verification)
   - `session_token`: the Keystone token string
   - `parent_user`: Keystone username
   - `groups`: roles list
   - `claims`: JSON map with `keystone_user_id`, `keystone_project_id`, `keystone_roles`, `auth_source: "keystone"`
6. **Task-local storage:** Store credentials in `KEYSTONE_CREDENTIALS` task-local (async-scoped to request)
7. **Auth bypass:** IAMAuth detects `keystone:` prefix → returns empty secret key, bypassing SigV4
8. **Authorization:** `check_key_valid()` retrieves credentials from task-local storage
9. **Role check:** `admin` or `reseller_admin` roles → `is_owner=true`; other roles → `is_owner=false`

**Request without `X-Auth-Token`:**
1. Middleware passes through unchanged
2. Standard AWS SigV4 authentication proceeds
3. IAM validation as normal

**Invalid token:**
1. Middleware returns `401 Unauthorized` immediately with XML error body
2. **No fallback** to standard S3 auth

#### EC2 Credentials

RustFS also supports Keystone EC2 credentials for S3 API compatibility:

- `POST /v3/ec2tokens` with `{access, signature, data}` validates EC2-style credentials
- `GET /v3/users/{user_id}/credentials/OS-EC2` lists EC2 credentials for a user
- Access key format: `user_id:project_id` or `user_id`

#### Role Mapping (Keystone → RustFS)

| Keystone Role | RustFS Policy | Permissions |
|---------------|---------------|-------------|
| `admin` | AdminPolicy | Full access (`s3:*`) |
| `Admin` | AdminPolicy | Full access |
| `Member` | ReadWritePolicy | Read/write |
| `_member_` | ReadOnlyPolicy | Read-only |
| `ResellerAdmin` | AdminPolicy | Full access |
| `SwiftOperator` | ReadWritePolicy | Read/write |
| `objectstore:admin` | AdminPolicy | Full access |
| `objectstore:creator` | ReadWritePolicy | Read/write |

Custom role mappings can be added programmatically via `KeystoneIdentityMapper::add_role_mapping()`.

#### Multi-Tenancy

When `RUSTFS_KEYSTONE_TENANT_PREFIX=true`:
- Bucket creation: `mybucket` → stored as `project_id:mybucket`
- Bucket listing: filtered by project_id
- Access control: users can only access their project's buckets

---

## 4. Authentication Model — Complete Reference

### Credentials Struct

The core `Credentials` struct (in `rustfs-credentials`):

```rust
pub struct Credentials {
    pub access_key: String,      // S3 access key (or "keystone:<user_id>")
    pub secret_key: String,      // S3 secret key (empty for Keystone)
    pub session_token: String,   // STS session token / Keystone token
    pub expiration: Option<OffsetDateTime>,  // Token expiration
    pub status: String,          // "active" or "off"
    pub parent_user: String,     // Parent user for STS/service accounts
    pub groups: Option<Vec<String>>,  // Group membership
    pub claims: Option<HashMap<String, Value>>,  // JWT/Keystone claims
    pub name: Option<String>,    // Human-readable name
    pub description: Option<String>,
}
```

Key methods:
- `is_expired()` — checks if the credential's expiration has passed
- `is_temp()` — true if `session_token` is non-empty and not expired
- `is_service_account()` — true if claims contain `sa-policy` key and `parent_user` is non-empty
- `is_valid()` — access_key >= 3 chars, secret_key >= 8 chars, not expired, status != "off"
- Default credentials: `rustfsadmin` / `rustfsadmin` (env vars: `RUSTFS_ACCESS_KEY` / `RUSTFS_SECRET_KEY`)

### IAM System

The IAM system (`rustfs-iam`) manages:

- **Users and groups** with RBAC
- **Service accounts** and API key authentication
- **Policy engine** with fine-grained S3-style permissions
- **LDAP/Active Directory** integration
- **Session management** and token validation
- **OIDC integration** (full OpenID Connect with PKCE)

The IAM system is initialized as a singleton (`IAM_SYS`) backed by an `ObjectStore` (persisted in the S3 storage itself). Lookups go through `IamSys::check_key(access_key)` which loads from cache or disk.

### OIDC Support

RustFS has comprehensive OIDC support (`rustfs-iam` → `oidc.rs`):

**Configuration (environment variables):**
- `RUSTFS_IDENTITY_OPENID_ENABLE=on`
- `RUSTFS_IDENTITY_OPENID_CONFIG_URL` — OIDC discovery URL
- `RUSTFS_IDENTITY_OPENID_CLIENT_ID` — OAuth2 client ID
- `RUSTFS_IDENTITY_OPENID_CLIENT_SECRET` — OAuth2 client secret
- `RUSTFS_IDENTITY_OPENID_SCOPES` — comma-separated scopes (default: `openid,profile,email`)
- `RUSTFS_IDENTITY_OPENID_GROUPS_CLAIM` — claim for group membership
- `RUSTFS_IDENTITY_OPENID_ROLES_CLAIM` — claim for role mapping (Microsoft Entra ID app roles)
- `RUSTFS_IDENTITY_OPENID_CLAIM_NAME` — primary claim for policy mapping
- `RUSTFS_IDENTITY_OPENID_CLAIM_PREFIX` — prefix for claim-to-policy mapping
- `RUSTFS_IDENTITY_OPENID_REDIRECT_URI` — callback URL
- `RUSTFS_IDENTITY_OPENID_REDIRECT_URI_DYNAMIC` — allow dynamic redirect URIs

**Features:**
- Authorization Code flow with PKCE
- OIDC discovery and JWKS auto-refresh
- Multiple OIDC providers (suffixed env vars like `_PRIMARY`, `_SECONDARY`)
- ID token verification (signature, issuer, audience, expiry)
- `AssumeRoleWithWebIdentity` flow (JWT directly, no browser)
- Roles and groups claim mapping to RustFS IAM policies
- Provider-specific configuration (Microsoft Entra ID roles claim support)

**OIDC Claims → RustFS Policy Mapping:**
```json
{
  "Version": "2012-10-17",
  "Statement": [{
    "Effect": "Allow",
    "Action": ["admin:*"],
    "Resource": ["arn:aws:s3:::*"],
    "Condition": {
      "ForAnyValue:StringEquals": {
        "jwt:roles": ["RustFS.ConsoleAdmin"]
      }
    }
  }]
}
```

### RPC Authentication

RustFS uses a derived RPC secret for inter-node communication:
- Environment variable: `RUSTFS_RPC_SECRET` (explicit) or derived from `access_key + secret_key` via HMAC-SHA256
- Uses a `0xFFFFFFFFFFFFFFFF` mask for the signing context
- Base64url-encoded (no padding) output

---

## 5. Docker Deployment

### Simple Deployment

```yaml
# docker-compose-simple.yml
services:
  rustfs:
    image: rustfs/rustfs:latest
    ports:
      - "9000:9000"  # S3 API
      - "9001:9001"  # Console
    environment:
      - RUSTFS_VOLUMES=/data/rustfs{0...3}
      - RUSTFS_ADDRESS=0.0.0.0:9000
      - RUSTFS_CONSOLE_ADDRESS=0.0.0.0:9001
      - RUSTFS_ACCESS_KEY=rustfsadmin
      - RUSTFS_SECRET_KEY=rustfsadmin
      - RUSTFS_OBS_LOGGER_LEVEL=info
    volumes:
      - rustfs_data_0:/data/rustfs0
      - rustfs_data_1:/data/rustfs1
      - rustfs_data_2:/data/rustfs2
      - rustfs_data_3:/data/rustfs3
```

### Full Deployment (with Observability)

```yaml
# docker-compose.yml (with --profile observability)
services:
  rustfs:
    # ... same as above, plus:
    - RUSTFS_OBS_ENDPOINT=http://otel-collector:4318
  otel-collector:  # OpenTelemetry collector
  tempo:           # Distributed tracing
  jaeger:          # Jaeger UI
  prometheus:      # Metrics
  loki:            # Logs
  grafana:         # Dashboards
  nginx:           # Reverse proxy (optional, --profile proxy)
```

### Dockerfile

- Base: Alpine 3.23.4
- Runs as non-root user `rustfs` (UID/GID 10001:10001)
- Single binary: `/usr/bin/rustfs`
- Entrypoint: `/entrypoint.sh` (processes volumes, log dirs, default credential warnings)
- Health check: HTTP/HTTPS `/health` on port 9000, `/rustfs/console/health` on 9001
- Supports TLS via `RUSTFS_TLS_PATH=/opt/tls` with `rustfs_cert.pem` + `rustfs_key.pem` + optional `ca.crt`

### Keystone-Enabled Deployment

```bash
docker run -d \
  -p 9000:9000 -p 9001:9001 \
  -e RUSTFS_ACCESS_KEY=admin \
  -e RUSTFS_SECRET_KEY=adminsecret \
  -e RUSTFS_KEYSTONE_ENABLE=true \
  -e RUSTFS_KEYSTONE_AUTH_URL=http://keystone:5000 \
  -e RUSTFS_KEYSTONE_VERSION=v3 \
  -e RUSTFS_KEYSTONE_ADMIN_USER=admin \
  -e RUSTFS_KEYSTONE_ADMIN_PASSWORD=secret \
  -e RUSTFS_KEYSTONE_ADMIN_PROJECT=admin \
  -e RUSTFS_KEYSTONE_ADMIN_DOMAIN=Default \
  -v /data:/data \
  rustfs/rustfs:latest
```

### Webhook Notification

```bash
docker run -d --name rustfs -p 9000:9000 \
  -e RUSTFS_NOTIFY_ENABLE=true \
  -e RUSTFS_NOTIFY_WEBHOOK_ENABLE_PRIMARY=on \
  -e RUSTFS_NOTIFY_WEBHOOK_ENDPOINT_PRIMARY=http://host:3020/webhook \
  -e RUSTFS_NOTIFY_WEBHOOK_QUEUE_DIR_PRIMARY=/tmp/rustfs-events \
  rustfs/rustfs:latest
```

---

## 6. SDK/Client Libraries — Rust S3 Clients

### aws-sdk-s3 (Official AWS SDK for Rust)

RustFS itself uses `aws-sdk-s3` (v1.135.0) as a dependency — this is the most mature Rust S3 client:

```toml
aws-sdk-s3 = { version = "1.135.0", default-features = false, features = ["sigv4a", "default-https-client", "rt-tokio"] }
aws-config = { version = "1.8.18" }
aws-credential-types = { version = "1.2.14" }
```

**Pros:** Full S3 API coverage, SigV4/SigV4a signing, async, production-tested
**Cons:** Heavy dependency (pulls in significant AWS SDK surface area), AWS-centric abstractions

### s3s (RustFS's own S3 framework)

RustFS uses a custom `s3s` crate (`https://github.com/rustfs/s3s`, with `minio` feature):
```toml
s3s = { git = "https://github.com/rustfs/s3s", rev = "507e1312b211c3ddc214b03875d6fabd15d22ed5", features = ["minio"] }
```

This provides S3 request/response types, routing, and the `S3Auth` trait used by RustFS's `IAMAuth`.

### rust-s3 ( Community)

Not used by RustFS, but worth noting as an alternative:
- Crate: `rust-s3` / `s3`
- Simpler API than aws-sdk-s3
- Supports MinIO-compatible endpoints
- Less complete S3 operation coverage

### Recommendation for alknet

For alknet's S3 adapter:
- **Internal use**: aws-sdk-s3, configured with custom endpoint pointing to rustfs
- **Request signing**: If building a lightweight adapter, extract just the signing logic from `rustfs-signer` or use `aws-smithy-runtime` directly
- **The CredentialSet::S3AccessKey variant** (from alknet's credential-provider.md) maps directly to RustFS's `access_key + secret_key` pair; no additional transformation needed

---

## 7. Relevance to Alknet

### 7.1 RustFS as an Internal Object Store Behind Alknet's HTTP Interface

**Architecture:**
```
Client (any S3 SDK)
  → Alknet HTTP adapter (port 443/80 with HTTPS termination)
    → RustFS (port 9000, Docker network, not exposed externally)
      → Disk storage (/data volumes)
```

**Deployment pattern:** RustFS runs as a Docker container on the same Docker network as alknet, listening only on the internal network. Alknet's HTTP interface reverse-proxies S3 API calls to rustfs.

**Reverse proxy considerations:**
- Alknet would forward `Host`, `Authorization`, `X-Auth-Token`, `X-Amz-*` headers unchanged
- RustFS needs the real client IP for S3 policy `SourceIp` conditions; alknet should set `X-Forwarded-For` and configure `RUSTFS_TRUSTED_PROXIES` or use rustfs's `trusted-proxies` crate
- Health check: Alknet proxies `/health` → rustfs:9000
- RustFS supports `X-Forwarded-Proto` for TLS offloading via its `trusted-proxies` crate

**Why behind alknet rather than standalone:**
1. Unified TLS termination at alknet
2. alknet can inject auth headers (e.g., OIDC tokens) before forwarding
3. alknet can enforce rate limiting and access control
4. Network isolation — rustfs only accessible via alknet

**Webhook integration:** RustFS can POST events to alknet via its notification system:
```bash
RUSTFS_NOTIFY_WEBHOOK_ENDPOINT_PRIMARY=http://alknet:3020/webhook
```

### 7.2 Mapping S3 Auth to Alknet's CredentialProvider/CredentialSet

The alknet `CredentialSet` enum directly models the S3 auth pattern:

| RustFS Auth Method | Alknet CredentialSet Variant | Mapping |
|---|---|---|
| Access key + secret key (SigV4) | `S3AccessKey { access_key, secret_key, session_token }` | Direct 1:1 mapping; access_key and secret_key are the S3 credential pair |
| Keystone X-Auth-Token | `OidcToken { access_token, ... }` | Keystone token → OIDC access_token; expires_at maps to Keystone token expiration |
| STS AssumeRole session | `S3AccessKey { ..., session_token: Some(...) }` | STS temporary credentials with session token |
| OIDC (browser flow) | `OidcToken { access_token, refresh_token, expires_at }` | Direct mapping |
| Admin default credentials | `S3AccessKey { access_key: "rustfsadmin", secret_key: "rustfsadmin" }` | Service-level credential |

**S3 Request Signing (Phase C in credential-provider.md):**
The `S3AccessKey` variant contains the raw credential data. The signing computation itself is separate — it's a utility function `s3_sign(credential: &S3AccessKey, request: &HttpRequest) -> SignedRequest` that should live in a shared `alknet-s3` utility crate, not in `CredentialSet`. This matches OpenQ-04 in the credential-provider doc.

**For alknet's `S3CredentialManager`:**
```rust
impl CredentialManager for S3CredentialManager {
    fn refresh(&self, current: &CredentialSet) -> Option<CredentialSet> {
        // If we have an STS session token, check expiration
        // and re-AssumeRole if needed
    }
    
    fn is_expired(&self, current: &CredentialSet) -> bool {
        match current {
            CredentialSet::S3AccessKey { session_token: Some(t), .. } 
                if !t.is_empty() => check_sts_expiration(t),
            CredentialSet::OidcToken { expires_at: Some(ts), .. } 
                => *ts < now(),
            _ => false, // Static keys don't expire
        }
    }
    
    fn provision(&self, identity: &Identity) -> Option<CredentialSet> {
        // Create a rustfs IAM access key for this alknet identity
        // via the rustfs admin API
    }
}
```

### 7.3 Alknet as an OIDC Provider for RustFS (Phase D)

This is the most strategically important integration point. RustFS already has complete OIDC support — it just needs an OIDC provider to trust.

**How it would work:**

1. **alknet exposes OIDC endpoints** (via call protocol HTTP adapter or a dedicated `/oidc/` path):
   - `GET /.well-known/openid-configuration` — discovery document
   - `GET /oidc/authorize` — authorization endpoint
   - `POST /oidc/token` — token exchange
   - `GET /oidc/userinfo` — user info
   - `GET /oidc/jwks` — JSON Web Key Set
   - `GET /oidc/logout` — RP-initiated logout

2. **alknet's Identity maps to OIDC claims:**
   - `sub` → `Identity.id` (SSH fingerprint or account UUID)
   - `email` → from account metadata (if available)
   - `username` → display name or `Identity.id`
   - `groups` → `Identity.scopes` (e.g., `["s3:admin", "s3:readwrite"]`)
   - `roles` → derived from scopes (e.g., `scope "s3:admin"` → role `"admin"`)

3. **RustFS configuration** (pointing at alknet):
   ```bash
   RUSTFS_IDENTITY_OPENID_ENABLE=on
   RUSTFS_IDENTITY_OPENID_CONFIG_URL=https://alknet:443/.well-known/openid-configuration
   RUSTFS_IDENTITY_OPENID_CLIENT_ID=alknet-rustfs-client
   RUSTFS_IDENTITY_OPENID_CLIENT_SECRET=<auto-generated>
   RUSTFS_IDENTITY_OPENID_SCOPES=openid,profile,email,groups
   RUSTFS_IDENTITY_OPENID_GROUPS_CLAIM=groups
   RUSTFS_IDENTITY_OPENID_ROLES_CLAIM=roles
   ```

4. **Authentication flow:**
   - User connects to alknet (via SSH/WebTransport/HTTP)
   - alknet resolves identity → `Identity { id, scopes, resources }`
   - User requests access to rustfs console
   - Browser redirects to alknet's OIDC authorize endpoint
   - alknet issues authorization code → token exchange → ID token
   - RustFS verifies the ID token using alknet's JWKS endpoint
   - RustFS maps `groups` and `roles` claims to IAM policies

5. **For `AssumeRoleWithWebIdentity` (programmatic access):**
   - alknet issues a JWT directly to the client
   - Client presents JWT to RustFS via `Action=AssumeRoleWithWebIdentity`
   - RustFS calls `OidcSys::verify_web_identity_token()` which:
     - Decodes JWT payload to get `iss` claim
     - Finds matching OIDC provider (alknet)
     - Verifies signature, issuer, audience, expiry
     - Extracts claims → maps to RustFS policies

**This eliminates stored credentials entirely** — alknet identities authenticate directly to rustfs via OIDC, no `S3AccessKey` needed.

### 7.4 Alknet RustFS Adapter Architecture

An alknet HTTP/HTTPS adapter for the S3 API would look like:

```
alknet HTTP adapter
  ├── Route: /s3/* → reverse proxy to rustfs:9000
  │   ├── Preserve all S3 headers (Authorization, X-Amz-*, X-Auth-Token, Content-*)
  │   ├── Set X-Forwarded-For, X-Forwarded-Proto
  │   ├── Optionally inject X-Auth-Token from alknet Identity
  │   └── Response streaming (for large object downloads)
  ├── Route: /s3/health → rustfs:9000/health (health check)
  └── Route: /s3/admin/* → rustfs:9000/minio/* (admin API)
```

**Key considerations:**
- S3 requests can be very large (multipart uploads, 5TB+ objects). The adapter must support streaming both request and response bodies without buffering.
- `X-Forwarded-For` must be set so rustfs can evaluate `SourceIp` condition keys in bucket policies.
- RustFS already handles `X-Forwarded-Proto` for HTTPS offloading via its `trusted-proxies` crate.
- For OIDC integration, the adapter doesn't need to modify auth headers — rustfs handles OIDC token validation itself when pointed at alknet's OIDC endpoint.

**Alknet's `OpenAPIServiceRegistry` integration:**

Since rustfs exposes an S3 API, alknet could auto-register S3 operations via an OpenAPI spec or hardcoded operation specs:

```rust
// In alknet's service registry:
let s3_ops = FromOpenAPI(s3_openapi_spec, config);
// Where config.auth = CredentialSet::S3AccessKey { access_key, secret_key, session_token: None }
// Or: config.auth = CredentialSet::OidcToken { access_token, refresh_token, expires_at }
```

---

## 8. Key RustFS Source Files for Reference

| File | Purpose |
|------|---------|
| `crates/credentials/src/credentials.rs` | `Credentials` struct, global credentials, key generation |
| `crates/credentials/src/constants.rs` | Default access/secret keys, IAM policy constants |
| `crates/signer/` | AWS Signature V4 implementation |
| `crates/keystone/src/config.rs` | Keystone configuration from env vars |
| `crates/keystone/src/client.rs` | Keystone v3 API client (token validation, EC2 creds, admin auth) |
| `crates/keystone/src/auth.rs` | `KeystoneAuthProvider` (token → `Credentials` mapping) |
| `crates/keystone/src/middleware.rs` | Tower middleware extracting `X-Auth-Token`, task-local storage |
| `crates/keystone/src/identity.rs` | `KeystoneIdentityMapper` (role → policy, tenant prefix) |
| `crates/iam/src/oidc.rs` | Complete OIDC system (discovery, PKCE, token exchange, JWT verification) |
| `crates/iam/src/sys.rs` | `IamSys` (IAM singleton, user/key management) |
| `crates/policy/` | S3 bucket/IAM policy evaluation engine |
| `rustfs/src/auth.rs` | `IAMAuth`, `check_key_valid`, auth type detection, condition values |
| `rustfs/src/server/` | HTTP server, TLS, routing, middleware stack |
| `crates/protocols/src/swift/` | OpenStack Swift API implementation |
| `Dockerfile` / `docker-compose-simple.yml` | Deployment configuration |

---

## 9. Configuration Quick Reference

### RustFS Docker Environment Variables (Auth-Relevant)

| Variable | Description | Default |
|----------|-------------|---------|
| `RUSTFS_ACCESS_KEY` | Root access key | `rustfsadmin` |
| `RUSTFS_SECRET_KEY` | Root secret key | `rustfsadmin` |
| `RUSTFS_ADDRESS` | S3 API listen address | `0.0.0.0:9000` |
| `RUSTFS_CONSOLE_ADDRESS` | Console listen address | `0.0.0.0:9001` |
| `RUSTFS_CONSOLE_ENABLE` | Enable web console | `true` |
| `RUSTFS_TLS_PATH` | TLS certificate directory | (none, HTTP) |
| `RUSTFS_KEYSTONE_ENABLE` | Enable Keystone auth | `false` |
| `RUSTFS_KEYSTONE_AUTH_URL` | Keystone v3 endpoint | (required if enabled) |
| `RUSTFS_KEYSTONE_VERSION` | Keystone API version | `v3` |
| `RUSTFS_KEYSTONE_ADMIN_USER` | Keystone admin user | (optional) |
| `RUSTFS_KEYSTONE_ADMIN_PASSWORD` | Keystone admin password | (optional) |
| `RUSTFS_KEYSTONE_ADMIN_PROJECT` | Keystone admin project | (optional) |
| `RUSTFS_KEYSTONE_ADMIN_DOMAIN` | Keystone admin domain | `Default` |
| `RUSTFS_KEYSTONE_VERIFY_SSL` | Verify Keystone TLS | `true` |
| `RUSTFS_KEYSTONE_CACHE_SIZE` | Token cache size | `10000` |
| `RUSTFS_KEYSTONE_CACHE_TTL` | Token cache TTL (sec) | `300` |
| `RUSTFS_KEYSTONE_TENANT_PREFIX` | Enable tenant prefixing | `true` |
| `RUSTFS_IDENTITY_OPENID_ENABLE` | Enable OIDC | `off` |
| `RUSTFS_IDENTITY_OPENID_CONFIG_URL` | OIDC discovery URL | (required) |
| `RUSTFS_IDENTITY_OPENID_CLIENT_ID` | OIDC client ID | (required) |
| `RUSTFS_IDENTITY_OPENID_CLIENT_SECRET` | OIDC client secret | (optional) |
| `RUSTFS_IDENTITY_OPENID_SCOPES` | OIDC scopes | `openid,profile,email` |
| `RUSTFS_IDENTITY_OPENID_GROUPS_CLAIM` | Groups claim name | `groups` |
| `RUSTFS_IDENTITY_OPENID_ROLES_CLAIM` | Roles claim name | (empty, opt-in) |
| `RUSTFS_RPC_SECRET` | Inter-node RPC auth secret | (derived from keys) |
| `RUSTFS_NOTIFY_WEBHOOK_ENABLE_PRIMARY` | Enable webhook notifications | `off` |
| `RUSTFS_NOTIFY_WEBHOOK_ENDPOINT_PRIMARY` | Webhook URL | (required) |

---

## 10. Summary of Integration Paths

### Phase A (Immediate): Static S3 Credentials

- Deploy rustfs as a Docker service next to alknet
- Configure `RUSTFS_ACCESS_KEY` and `RUSTFS_SECRET_KEY`
- alknet stores these as `CredentialSet::S3AccessKey`
- alknet's HTTP adapter reverse-proxies S3 calls to rustfs
- Use `aws-sdk-s3` or `rust-s3` as the client library

**Effort:** Low. No auth changes in either system.

### Phase B: OIDC via External Provider

- Configure rustfs `RUSTFS_IDENTITY_OPENID_*` to point at an external OIDC provider (e.g., Keycloak, Authentik, Microsoft Entra ID)
- alknet can still manage its own auth independently
- Both systems trust the same OIDC provider

**Effort:** Low. Configuration-only change in rustfs.

### Phase C: Managed Credentials

- alknet provisions rustfs access keys via admin API (`/minio/` endpoints)
- `S3CredentialManager` handles session token rotation
- Identity-bound credentials: alknet creates per-user access keys in rustfs IAM

**Effort:** Medium. Requires admin API client, credential lifecycle management.

### Phase D: Alknet as OIDC Provider (Target State)

- alknet exposes OIDC endpoints (`.well-known/openid-configuration`, `/oidc/authorize`, `/oidc/token`, `/oidc/jwks`)
- rustfs trusts alknet as its OIDC provider
- `Identity.scopes` maps to rustfs IAM policies (e.g., `s3:admin` → admin policy)
- No stored S3 credentials — users authenticate directly via alknet identity
- `AssumeRoleWithWebIdentity` for programmatic access

**Effort:** High. Requires building OIDC authorization server in alknet. This is the most elegant but most complex path.

---

## References

- [RustFS GitHub](https://github.com/rustfs/rustfs) — v1.0.0-beta.7
- [RustFS Documentation](https://docs.rustfs.com)
- [RustFS Keystone README](file:///workspace/rustfs/crates/keystone/README.md) — comprehensive Keystone integration docs
- [RustFS OIDC implementation](file:///workspace/rustfs/crates/iam/src/oidc.rs) — full OIDC client with PKCE, discovery, JWKS refresh
- [RustFS auth.rs](file:///workspace/rustfs/rustfs/src/auth.rs) — IAMAuth, check_key_valid, auth type detection
- [alknet credential-provider.md](file:///workspace/@alkdev/alknet/docs/research/phase2/credential-provider.md) — alknet's outbound auth design
- [alknet identity.md](file:///workspace/@alkdev/alknet/docs/architecture/identity.md) — alknet's inbound auth design