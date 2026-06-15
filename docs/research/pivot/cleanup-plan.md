# Cleanup Plan: Pre-Pivot Housekeeping

> Status: Draft
> Created: 2026-06-15
> Purpose: Clean the workspace before starting the ALPN-as-service refactor so agents have a clear, bias-free starting point.

## Guiding Principles

1. **Delete noise, preserve signal.** If a document describes concepts that the pivot replaces or defers, it should be archived — not left in place where agents might pattern-match to the old model.
2. **Reference docs are a library, not a plan.** The `references/` directory (iroh, ssh, nats, etc.) is neutral research about external projects. Those stay. But gitserver is MPL — see below.
3. **ADRs that codify discarded abstractions become landmines.** An agent reading ADR-026 ("Transport/interface separation") will try to implement the three-layer model. These need to be marked superseded or archived.
4. **Code that stays should compile cleanly.** We're not rewriting yet — we're making the existing code a clean reference implementation to port from.

---

## Phase 1: Archive Obsolete Architecture Docs

These documents describe concepts that the ALPN pivot **replaces entirely**. They're historical record, not guidance. Move to `docs/_archived/` (preserved in git history, out of sight for agents).

### Architecture docs to archive

| Document | Reason |
|----------|--------|
| `docs/architecture/interface.md` | Defines `StreamInterface` / `MessageInterface` — replaced by `ProtocolHandler` |
| `docs/architecture/services.md` | Defines irpc service layer, `OperationEnv` three dispatch paths — replaced by ALPN dispatch |
| `docs/architecture/call-protocol.md` | Describes `OperationEnv` composition model — will be rewritten for `alknet-call` |
| `docs/architecture/storage.md` | Metagraph/data model — deferred per pivot |
| `docs/architecture/flowgraph.md` | FlowGraph crate — deferred per pivot |
| `docs/architecture/credentials.md` | `CredentialProvider` phases A–D — simplifying per pivot |
| `docs/architecture/napi-and-pubsub.md` | Describes pubsub event target adapter through SSH control channel — old model |
| `docs/architecture/open-questions.md` | Most open questions are about the old model; will regenerate after pivot |

### ADRs to mark superseded

These ADRs codify abstractions that the pivot replaces. Don't delete — mark them with a clear `Superseded` status and a pointer to the pivot doc so agents don't treat them as current guidance.

| ADR | Title | Superseded by |
|-----|-------|---------------|
| 026 | Transport/interface separation (three-layer model) | ALPN dispatch — one trait, one dispatch point |
| 035 | StreamInterface / MessageInterface split | `ProtocolHandler` trait replaces both |
| 033 | OperationEnv as universal composition mechanism | Call protocol is one ALPN; no three dispatch paths |
| 027 | Crate decomposition (core, storage, flowgraph) | New crate decomposition per pivot |
| 028 | Auth as irpc service behind feature flag | Auth is shared in `alknet-core`, not an irpc service |
| 032 | Event boundary discipline (domain, irpc, call) | Simplified — each handler manages its own wire format |
| 018 | Control channel for pubsub over SSH | Pubsub is a handler concern, not a SSH control channel |
| 017 | Stealth mode — protocol multiplexing on port 443 | ALPN negotiation replaces byte-peek protocol detection |

### ADRs that remain valid

These capture decisions that carry forward regardless of the refactor:

| ADR | Title | Why it stays |
|-----|-------|--------------|
| 001 | Pluggable transport via `AsyncRead+AsyncWrite` | Still true — `BiStream` implements these traits |
| 003 | iroh stream via `tokio::io::join` | Still relevant for iroh transport |
| 004 | SSH runs over transport, not alongside | Still true — SSH is one ALPN handler |
| 005 | SOCKS5 as primary client interface | Still true for `alknet-ssh` |
| 006 | No logging of tunnel destinations | Security principle, always valid |
| 008 | ACME/Let's Encrypt certificate provisioning | Still needed for TLS endpoints |
| 012 | Ed25519 keys + OpenSSH cert-authority | Auth model carries forward |
| 013 | Fail2ban-friendly logging | Still relevant |
| 023 | Unified auth with shared key material + token auth | Becomes `IdentityProvider` in core |
| 024 | Bidirectional call protocol (EventEnvelope) | EventEnvelope framing carries into `alknet-call` |
| 025 | Handler/spec separation | Still a good pattern — each ALPN handler has its own spec |
| 029 | Identity as core type in alknet-core | Still true |
| 030 | Static/dynamic config split with ArcSwap | Still true — `ArcSwap<DynamicConfig>` pattern continues |
| 031 | Forwarding policy with rule-based allow/deny | Still relevant for `alknet-ssh` |
| 037 | API keys as DynamicConfig auth | Carries forward |
| 038 | Seed lifecycle and memory security | `alknet-secret` is standalone, unchanged |

### ADRs to archive (deferred/irrelevant)

| ADR | Title | Reason |
|-----|-------|--------|
| 002 | TUN shim as separate process | Already superseded by ADR-014 |
| 007 | NAPI exposes single duplex stream | Needs rewrite — NAPI becomes call protocol client |
| 009 | Default iroh relay with override | Low-level transport detail, not an architecture decision |
| 010 | Transport chaining in CLI | CLI UX detail, revisit when building new CLI |
| 011 | Programmatic-first API, no file-based config | CLI detail, revisit |
| 014 | Defer TUN, recommend SOCKS5 + tun2proxy | Still valid direction but already noted as deferred |
| 015 | napi-rs for FFI bridge | Implementation detail, revisit |
| 016 | NAPI exposes both connect() and serve() | Needs rewrite for new model |
| 019 | Proxy dual semantics | CLI detail, revisit |
| 034 | Head/worker terminology | Terminology, not architecture; revisit |
| 036 | CredentialProvider as core type | Simplifying per pivot |

---

## Phase 2: Clean Up Research Docs

### Keep as-is (neutral external references)

- `docs/research/references/iroh/` — ALPN dispatch, QUIC endpoints, protocol handler pattern
- `docs/research/references/ssh/russh/` — SSH implementation reference for `alknet-ssh`
- `docs/research/references/ssh/russh-sftp/` — SFTP protocol reference for `alknet-sftp`
- `docs/research/references/ssh/sftp-rs/` — SFTP comparison reference
- `docs/research/references/nats.rs/` — Service/pattern reference (not directly used but neutral)
- `docs/research/references/honker/` — May be relevant for messaging/mixnet later
- `docs/research/references/polyglot/` — FFI patterns for NAPI/WASM
- `docs/research/references/openstack-keystone/` — Auth patterns reference
- `docs/research/references/rustfs/` — May be relevant for distributed storage later
- `docs/research/ops/` — fail2ban, certbot — production reference, still useful
- `docs/research/feasibility/` — Historical context, keep

### Handle MPL-licensed reference

**`docs/research/references/gitserver/gitserver-reference.md`** — This references an MPL-2.0 project. The reference doc itself is our analysis, not derived work. However, since the git adapter will use `gix` (Apache-2.0/MIT) directly — not gitserver — keeping this doc around creates risk of an agent copying structure or logic from the MPL codebase.

**Action:** Archive it. Replace with a `gix`-focused reference when we spec the git adapter. The research value is the git protocol knowledge, not the gitserver code structure.

**`docs/research/references/gitlfs/rudolfs-reference.md`** — Unlike gitserver, rudolfs is MIT/Apache-2.0 licensed and is a legitimate candidate for forking. It's only missing a lock implementation and already has S3 storage support. The git ALPN adapter will need custom storage (likely iroh blobs for deduplication), but rudolfs's wire protocol handling and server structure are useful references. Keep this one.

### Archive (biased toward old model)

| Document | Reason |
|----------|--------|
| `docs/research/core.md` | Describes the three-layer model in detail — superseded by pivot |
| `docs/research/services.md` | Describes irpc service layer, OperationEnv — superseded |
| `docs/research/storage.md` | Metagraph — deferred |
| `docs/research/flow.md` | FlowGraph — deferred |
| `docs/research/configuration.md` | Promoted to architecture docs already |
| `docs/research/integration-plan.md` | Phase-based integration plan for old model — superseded |
| `docs/research/phase2/interface-model.md` | StreamInterface/MessageInterface analysis — superseded |
| `docs/research/phase2/credential-provider.md` | CredentialProvider phases — simplifying |
| `docs/research/phase2/definitions.md` | Promoted to architecture already |
| `docs/research/phase2/tls-transport.md` | Describes stealth mode — ALPN replaces this |
| `docs/research/event-sourcing/` | Event sourcing patterns — not currently needed |

### Keep but annotate

| Document | Reason |
|----------|--------|
| `docs/research/pivot/alpn-service-architecture.md` | **The plan.** This is current. |
| `docs/research/pivot/cleanup-plan.md` | **This doc.** Current. |

---

## Phase 3: Clean Up Architecture Docs That Stay

These docs describe concepts that carry forward but need updating to reflect the new model. We'll mark them as `needs-update` and add a note pointing to the pivot. Don't rewrite yet — just annotate so agents know they're partially stale.

| Document | What changes |
|----------|--------------|
| `docs/architecture/overview.md` | Crate structure, three-layer model references |
| `docs/architecture/transport.md` | Still valid but needs ALPN endpoint section |
| `docs/architecture/auth.md` | Auth per-handler table from pivot replaces credential presentation per interface |
| `docs/architecture/client.md` | Client connects to endpoint, opens stream on ALPN — simpler |
| `docs/architecture/server.md` | Server becomes ALPN router — fundamentally different |
| `docs/architecture/identity.md` | Mostly valid, needs `IdentityProvider` simplification |
| `docs/architecture/configuration.md` | ListenerConfig → ALPN advertisement config |
| `docs/architecture/secret-service.md` | Unchanged — `alknet-secret` is standalone |
| `docs/architecture/definitions.md` | Terminology needs update (interface, listener, handler → ALPN, handler) |
| `docs/architecture/tun-shim.md` | Already deprecated — archive |

---

## Phase 4: Clean Up Code

Not a rewrite — just remove dead weight so agents don't pattern-match to it.

### Delete from `alknet-core`

These modules/files implement concepts that the pivot replaces entirely. They'll be re-implemented in new crates:

| What | Lines | Reason |
|------|-------|--------|
| `src/interface/mod.rs` | 140 | `StreamInterface` / `MessageInterface` — replaced by `ProtocolHandler` |
| `src/interface/pairs.rs` | 122 | Transport/interface validation — no longer needed |
| `src/interface/config.rs` | 270 | `ListenerConfig` variants — replaced by ALPN advertisement |
| `src/interface/session.rs` | 62 | `InterfaceSession` / `InterfaceEvent` — old model |
| `src/interface/http.rs` | 66 | Old HTTP interface — becomes `alknet-http` handler |
| `src/interface/dns.rs` | 47 | Old DNS interface — becomes `alknet-dns` handler |
| `src/interface/raw_framing.rs` | 399 | Stealth mode byte-peek — replaced by ALPN negotiation |
| `src/server/stealth.rs` | 316 | Stealth mode — replaced by ALPN negotiation |
| `src/server/control_channel.rs` | 196 | SSH control channel for pubsub — old model |

**Keep as-is (port later):**

| What | Lines | Destination |
|------|-------|-------------|
| `src/interface/ssh.rs` | 982 | → `alknet-ssh` (largest single extraction) |
| `src/server/handler.rs` | 974 | → `alknet-ssh` (SSH server handler) |
| `src/server/channel_proxy.rs` | 555 | → `alknet-ssh` (port forwarding proxy) |
| `src/server/serve.rs` | 1526 | → rewrite as ALPN router (keep for reference, rewrite later) |
| `src/call/*` | ~1200 | → `alknet-call` (relatively clean extraction) |
| `src/auth/*` | ~1450 | → `alknet-core` (shared auth/identity) |
| `src/config/*` | ~950 | → `alknet-core` (static/dynamic config) |
| `src/transport/*` | ~1500 | → `alknet-core` (endpoint acceptors) |
| `src/client/*` | ~1900 | → `alknet-ssh` (client session, SOCKS5, forwarding) |
| `src/socks5/*` | ~800 | → `alknet-ssh` (SOCKS5 server) |
| `src/credentials/*` | ~250 | → simplify into `alknet-core` auth |
| `src/http/*` | ~340 | → `alknet-http` |
| `src/error.rs` | ~240 | → `alknet-core` |
| `src/testutil.rs` | ~140 | → `alknet-core` test utilities |

### Delete entire crate

| Crate | Reason |
|-------|--------|
| (none yet — `alknet-storage` and `alknet-flowgraph` don't exist as crates) |

The current workspace only has `alknet-core`, `alknet-secret`, `alknet-napi`, and `alknet` (CLI). No storage or flowgraph crates exist to delete.

---

## Phase 5: Create Pivot Spec Stub

After cleanup, create placeholder spec files for the new model. These are empty shells — just enough structure to guide the first implementation tasks:

```
docs/research/pivot/
├── alpn-service-architecture.md   (exists — the pivot proposal)
├── cleanup-plan.md                (this file)
├── specs/
│   ├── 01-alknet-core-v2.md       (ProtocolHandler trait, ALPN router, BiStream, AuthContext)
│   ├── 02-alknet-ssh.md           (SSH handler extraction)
│   ├── 03-alknet-call.md          (Call protocol extraction)
│   ├── 04-alknet-git.md           (Git adapter on gix)
│   ├── 05-alknet-sftp.md          (SFTP adapter on russh-sftp)
│   ├── 06-alknet-http.md          (HTTP adapter on axum)
│   ├── 07-alknet-dns.md           (DNS adapter on hickory)
│   ├── 08-alknet-msg.md           (Messaging adapter — E2E + mixnet)
│   ├── 09-alknet-napi.md          (NAPI rewrite as call protocol client)
│   └── 10-cli-binary.md           (CLI registers handlers)
```

These get filled in as we spec each component. They don't need content yet — just the file creation so the directory structure exists and the naming is established.

---

## Execution Order

1. **Create `docs/_archived/` directory** and move files there (preserves git history)
2. **Mark superseded ADRs** with `Superseded` status and pivot reference
3. **Move obsolete research docs** to `docs/_archived/research/`
4. **Annotate stale-but-keeping architecture docs** with `status: needs-update` header
5. **Delete replaced code modules** from `alknet-core` (interface layer, stealth, control channel)
6. **Create pivot spec stub directory** with empty spec files
7. **Fix compilation** — removing modules will break imports. Fix them minimally (comment out, stub, or remove call sites) so the project compiles. This is temporary scaffolding, not the refactor.

After this cleanup, the repo should:
- Compile (possibly with reduced functionality)
- Have no references to `StreamInterface`, `MessageInterface`, `ListenerConfig`, or stealth mode in active docs
- Have clear spec stubs for the new model
- Have all obsolete material in `docs/_archived/` where it won't bias agents