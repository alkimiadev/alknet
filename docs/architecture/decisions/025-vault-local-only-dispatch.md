# ADR-025: Vault Local-Only Dispatch

## Status

Accepted

## Context

alknet-vault uses irpc for its internal dispatch. The `VaultProtocol` enum is
annotated with `#[rpc_requests(message = VaultMessage, no_spans)]`, which
generates a `Service` trait impl (for in-process mpsc dispatch) and a
`RemoteService` trait impl (for remote QUIC dispatch). The vault's
`VaultServiceActor` processes `VaultMessage` variants from an mpsc channel.
This was adopted from irpc's actor pattern (ADR-005).

### What irpc gives the vault

Separating irpc into its constituent parts and asking which the vault
actually needs:

| irpc component | What it does | Does the vault need it? |
|---|---|---|
| `#[rpc_requests]` macro | Generates message enum, `Channels` impls, `From` conversions | Marginally — it's convenient boilerplate, but the vault's protocol is 8 variants |
| `Service` trait | Local in-process dispatch via mpsc + oneshot | No — `VaultServiceHandle` direct calls are already preferred (service.md: "For local in-process use, prefer `VaultServiceHandle` directly — no channel, no serialization") |
| `RemoteService` trait | Remote dispatch via QUIC + postcard | No — this is the footgun |
| `Client<S>` | Wraps either local mpsc or remote QUIC | No — the assembly layer uses the handle directly |
| `IrohProtocol` handler | Forwards all messages without auth | No — this is the default-insecure handler |
| postcard serialization | Binary serialization for remote dispatch | No — not needed without remote dispatch |
| `DerivedKey` dual serialization | JSON redacts, postcard preserves | Only needed *because* remote dispatch exists |

The vault uses irpc for the actor pattern (in-process mpsc dispatch), but
the actor pattern is the *secondary* dispatch path. The primary path — direct
method calls on `VaultServiceHandle` — doesn't use irpc at all. And the thing
that makes irpc attractive for the actor pattern (the macro-generated
boilerplate) is a convenience, not a structural need. The vault's protocol
is small enough that the boilerplate is manageable by hand, or simply
unnecessary when the actor is removed.

### The security problem: default-insecure

The core problem is not that remote vault access is *possible* in principle
— it's that irpc makes it possible *by default*, with the unsafe path being
the easy path.

The `#[rpc_requests]` macro generates `RemoteService` unless you pass
`no_rpc`. The `IrohProtocol` handler forwards all message types without auth
checks. The docs frame "register an ALPN" as a server-setup change
(OQ-21: "Enabling remote access is a server-setup change"). The result is
an architecture where:

1. The vault is remote-capable by construction (the footgun is loaded).
2. Enabling remote access is easy — one line: `Router::builder(endpoint)
   .accept(b"alknet/vault", protocol).spawn()`.
3. The default handler has no auth (the safety is off).
4. Making it safe requires an auth-wrapping handler *outside the vault
   crate* (the safety is a separate part you have to remember to install).

This is the **default-insecure anti-pattern**. Security should be opt-in, not
opt-out. The vault should be local-only by default, and remote access should
require *adding* something, not *removing* a default.

### The use cases don't justify the default

**Single node, local vault (the designed path):** The CLI binary unlocks the
vault at startup, derives/decrypts credentials, injects them into handler
capabilities. The vault is accessed only at the assembly layer (ADR-019). No
network. This is the path every deployment starts with, and it needs only
direct in-process method calls on `VaultServiceHandle`. irpc adds nothing.

**Many nodes encrypt/decrypt the same data:** The most likely network-vault
use case, but a stretch. The better pattern is per-node vaults: the head
encrypts credentials *for* the worker using the worker's public key or a
shared derivation path the worker can derive locally. The worker decrypts
locally. This is end-to-end encryption between nodes, not a centralized
decryption oracle. It matches ADR-008's "capability source" model —
credentials are injected at the assembly layer, not fetched over the network
at call time.

**Machine node → workers (OQ-21's use case):** A long-lived machine node
holds the mnemonic and exposes a restricted vault API to ephemeral workers.
This is the use case the vault docs actually spec. But `from_call`'s trust
model already flags the risk: "a compromised remote node can do anything its
operations are declared to do" (operation-registry.md). If the machine node
is compromised, every worker that calls it is compromised. That's inherent
to remote vault access and not a reason to forbid it, but it *is* a reason
to make the exposure a deliberate, hard-to-accidentally-enable act — not the
default state of the crate.

None of these use cases justify making the vault remote-capable *by
construction*. The first needs no remote. The second has a better pattern
(per-node vaults). The third is real but should be an explicit addition, not
a default that's already loaded.

### The actor path is dead code

service.md says "For local in-process use, prefer `VaultServiceHandle`
directly — no channel, no serialization." The actor exists *for* irpc, and
the direct path is preferred. So the vault has two dispatch paths, and the
one irpc provides (actor) is the secondary one. The primary path (direct
method calls) doesn't use irpc at all. The actor is dead code for the
designed use case — it exists only to make irpc's `Service` trait work,
which exists only to make `RemoteService` work, which is the footgun.

## Decision

### 1. alknet-vault drops irpc entirely

The vault's dispatch is direct method calls on `VaultServiceHandle`. No
`VaultProtocol` enum, no `VaultMessage`, no `VaultServiceActor`, no mpsc
channel, no `Service` trait, no `RemoteService` trait, no `Client<S>`, no
`IrohProtocol` handler, no postcard serialization.

The vault's public API is `VaultServiceHandle` (and the types it returns:
`DerivedKey`, `KeyType`, `EncryptedData`, `EncryptionKey`). That's it. An
implementer reading the vault crate sees one way to use it, not two ways
with a note saying "prefer the first."

### 2. The vault is local-only by construction

The vault crate has no remote dispatch capability. There is no
`RemoteService` trait, no remote handler, no wire format for vault messages.
Enabling remote vault access is not a flag flip or a server-setup change —
it requires *building a separate crate* that depends on both alknet-core
(for auth) and alknet-vault (for the handle) and adds the remote transport
+ auth-wrapping handler. That is a visible architectural act that shows up
in code review, not a runtime config flip on a macro that was already
generating the remote code.

This inverts the security default: local-only is the only mode. Remote
access requires adding something, not removing a default.

### 3. `DerivedKey` serialization simplifies

Without the postcard/remote-dispatch path, `DerivedKey`'s custom
`Serialize` always redacts the private key (for logging safety) — there is
no "postcard preserves bytes" path. The custom `Deserialize` rejects
`private_key == "[REDACTED]"` with an error rather than producing a
corrupted key (this resolves review #002 finding W8).

The redaction is purely for defense-in-depth against logging accidents.
The architectural control — `DerivedKey` never appears in call protocol
payloads (ADR-014) — is unchanged and remains the primary control. The
serialization redaction is the safety net, not the primary mechanism.

`VaultServiceError` no longer needs `Serialize`/`Deserialize` (which it had
for irpc dispatch). It can be a plain `thiserror::Error` enum. If a future
remote-vault crate needs to serialize errors across the wire, *that crate*
defines the wire representation.

### 4. If remote vault access is ever needed, it's a separate crate

The vault-server-crate question (review #002 C7) is decided: *if* remote
vault access is ever needed, it is a separate crate that depends on both
alknet-core (for `IdentityProvider`, scopes, auth-wrapping) and
alknet-vault (for `VaultServiceHandle`). The vault crate itself remains
local-only. This is a decision not to create the crate now, and not to
preclude it. It is the path of least commitment, and it matches ADR-018's
standalone-vault principle.

The remote vault crate would need its own ADR (matching ADR-019's language:
"requires its own ADR") defining the threat model, the access policy, the
auth-wrapping handler, and the operation filtering (Unlock/Lock local-only).

### 5. The vault's dependency footprint shrinks

The vault drops: `irpc`, `irpc-derive`, `postcard` (for remote), `noq`
(via irpc), `iroh` (via irpc-iroh). It retains: `bip39`, `ed25519-bip32`,
`aes-gcm`, `sha2`, `hmac`, `secp256k1` (feature-gated), `tokio` (for
`RwLock` sync primitives, not for channels), `serde` (for `DerivedKey`
redaction and `EncryptedData` wire format), `zeroize`, `thiserror`, `base64`,
`rand`.

ADR-018's "zero alknet crate dependencies" becomes "zero alknet crate
dependencies and zero RPC framework dependencies." This is the cleanest
version of ADR-018's intent.

## Consequences

**Positive:**

- The security default is inverted. Local-only is the only mode. Remote
  access requires building a separate crate — a visible, deliberate act.
  This matches the principle that security should be opt-in, not opt-out.
- The vault's API is honest. `VaultServiceHandle` is the API. No secondary
  dispatch path that exists for a feature (remote) that isn't enabled. An
  implementer sees one way to use the vault, not two with a note saying
  "prefer the first."
- Dead code is removed. The actor path, which service.md says is secondary
  to direct calls, is gone. The `VaultProtocol` enum, `VaultMessage`,
  `VaultServiceActor`, and the mpsc dispatch loop are gone. The vault is a
  pure library with a thread-safe handle.
- `DerivedKey` serialization simplifies. The dual serialization (JSON
  redacts, postcard preserves) is replaced by always-redact-on-serialize,
  reject-on-deserialize. No "postcard preserves bytes" path to test or
  document. This resolves review #002 W8 (silent corruption on
  JSON-deserialized `DerivedKey`) — the custom `Deserialize` rejects
  redacted payloads with an error.
- The dependency footprint shrinks. No irpc, no postcard-for-remote, no
  noq, no iroh via irpc. The vault is truly standalone (ADR-018's intent,
  strengthened). Supply-chain surface is reduced.
- The vault's concurrency model is honest. `VaultServiceHandle` is
  `Arc<RwLock<...>>` — the RwLock provides concurrent reads (derive) and
  exclusive writes (unlock/lock). The actor's sequential processing was
  actually *worse* for throughput than the RwLock. Removing the actor
  makes the concurrency model visible and correct.
- `derive_password` and `site_password_path` are removed from the vault's
  API and path model. The password-manager pattern (deterministic per-site
  passwords from HD derivation) is not relevant to an RPC system's vault —
  handlers call APIs (using API keys, OAuth tokens, mTLS), not websites
  with passwords. The vault is for cryptographic key derivation and
  credential encryption. This resolves review #002 C9 (site_password_path
  hash mapping underspecified) by removing the feature rather than
  specifying the non-standard string→u32 mapping and Ed25519-as-password-
  entropy construction. If deterministic password generation is ever needed
  (browser-automation edge case), it can be re-added or implemented as a
  separate concern — the cost is near-zero, and removing it now eliminates
  permanent API surface that was inherited from a prior project's
  password-manager pattern.

**Negative:**

- The vault's `VaultProtocol` enum and `VaultServiceActor` are removed.
  This is a breaking change to the vault crate's public API (`VaultProtocol`,
  `VaultMessage`, `VaultServiceActor`, `Client<VaultProtocol>` are removed
  from the public exports). Since no implementation consumer exists outside
  the vault crate itself (ADR-019: the assembly layer uses
  `VaultServiceHandle` directly), this is a spec edit, not a migration.
- If a future use case needs the actor pattern (e.g., for a remote-vault
  crate that wants in-process mpsc dispatch before forwarding over the
  wire), it must be re-added in *that crate*, not in the vault. This is
  additive — the vault's direct-handle API is unchanged.
- The `DerivedKey` postcard round-trip tests in `protocol.rs` are removed.
  The JSON-redaction tests remain. If a future remote-vault crate needs
  postcard serialization, it defines and tests its own serialization path
  for the types it sends over the wire.
- `VaultServiceError` loses `Serialize`/`Deserialize`. Any code that
  serialized vault errors (only the irpc dispatch path, which is removed)
  must adapt. The assembly layer converts vault errors to alknet-core
  errors at the boundary (ADR-018), and that conversion is string-based
  already.

**On review #002 findings resolved by this ADR:**

- **C7 (OQ-21 remote vault)**: resolved. OQ-21 moves from "deferred" to
  "resolved: remote vault access is not a feature of the vault crate; if
  needed, a separate vault-server crate wraps the vault and adds remote
  transport + auth, requiring its own ADR." The vault-server-crate question
  is decided: not created now, not precluded. The crate-decomposition
  one-way door (ADR-003 territory) is decided by *not* creating the crate
  now.
- **W8 (`DerivedKey` JSON deserialization silently corrupts)**: resolved.
  Without the postcard path, the custom `Deserialize` rejects
  `private_key == "[REDACTED]"` with an error. There is no
  "postcard preserves bytes" path to complicate the serialization story.
  The redaction is purely for logging safety; deserialization of a redacted
  payload is always an error.
- **C8 (operation access policy table incomplete)**: dissolved. Without
  `VaultProtocol`'s remote capability, there is no operation access policy
  table to complete — all operations are local-only by default. The table
  in protocol.md goes away. If a future vault-server crate exposes some
  operations remotely, *that crate* defines the access policy in its own
  ADR.
- **C9 (site_password_path hash mapping underspecified)**: resolved. The
  `derive_password` / `derive_password_string` / `site_password_path`
  methods are removed from the vault's API. The password-manager pattern
  is not relevant to an RPC system's vault. No hash mapping to specify,
  no Ed25519-as-password-entropy question to answer.

## Assumptions

1. **The vault's designed use case is local-only.** ADR-019 says the
   assembly layer is the sole direct caller. ADR-008 says the vault is a
   capability source accessed at assembly time. ADR-014 says handlers
   receive credentials through `OperationContext.capabilities`, not by
   calling vault operations. The vault was always designed to be local —
   irpc's remote capability was an accident of adoption, not a designed
   feature.

2. **Per-node vaults are the right pattern for multi-node deployments.**
   Each node has its own vault and mnemonic. Credentials are encrypted *for*
   the receiving node's public key, not decrypted centrally. This is
   end-to-end encryption, not a centralized decryption oracle. If this
   assumption is wrong (a use case truly requires centralized vault
   access), a remote-vault crate is the answer — not making the vault
   remote-capable by default.

3. **The actor pattern's sequential processing is not needed.**
   `VaultServiceHandle`'s `Arc<RwLock<...>>` provides concurrent reads
   (derive operations) and exclusive writes (unlock/lock). The actor's
   sequential processing was a constraint, not a feature — it serialized
   all operations including independent reads. The RwLock is the better
   concurrency model for this workload.

4. **The vault's protocol is small enough that macro-generated boilerplate
   is not a maintenance burden.** With 8 operations, the
   `VaultServiceHandle` method signatures *are* the protocol. There is no
   need for a separate protocol enum when the handle's methods are the
   API. If the vault grew to dozens of operations (unlikely given its
   scope), a protocol enum could be re-introduced — but it would be a
   local enum, not an irpc-generated one.

5. **`DerivedKey` never needs to cross a wire format that preserves
   private key bytes.** The architectural control (ADR-014:
   `DerivedKey` never appears in call protocol payloads) means
   `DerivedKey` is always used in-process. The redacting `Serialize` impl
   is for logging safety (defense-in-depth), not for wire transport. If a
   future remote-vault crate needs to send `DerivedKey` over the wire, it
   defines its own serialization for that context — the vault's
   `DerivedKey` stays redact-always.

## References

- ADR-005: irpc as call protocol foundation (this ADR amends the vault
  reference in ADR-005's Decision and Consequences; irpc remains the
  foundation for alknet-*call*, just not for alknet-*vault*)
- ADR-008: Vault integration point (the vault is a capability source
  accessed at assembly time — this ADR makes that the *only* mode)
- ADR-014: Secret material flow and capability injection (`DerivedKey`
  never appears in call protocol payloads — the redacting `Serialize`
  is defense-in-depth for logging, not for wire transport)
- ADR-018: Vault as standalone crate (this ADR strengthens the
  standalone principle: zero alknet crate dependencies *and* zero RPC
  framework dependencies)
- ADR-019: Vault assembly-layer-only access (this ADR makes the vault
  local-only, not just assembly-layer-only-for-direct-calls)
- OQ-21: Remote vault administration (resolved by this ADR — not a vault
  crate feature; if needed, a separate crate with its own ADR)
- docs/reviews/002-pre-implementation-architecture-sanity-check.md
  (findings C7, C8, W8 — resolved or dissolved by this ADR)
- irpc design patterns: `docs/research/references/iroh/irpc/09-design-patterns-and-examples.md`
  (Pattern 3: `no_rpc` flag — this ADR goes further by dropping irpc
  entirely, since the actor pattern is also unnecessary)