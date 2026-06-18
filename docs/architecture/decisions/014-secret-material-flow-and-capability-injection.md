# ADR-014: Secret Material Flow and Capability Injection

## Status

Accepted

## Context

alknet-vault holds the master seed and can derive keys and encrypt/decrypt
arbitrary data. ADR-008 established that the vault is a **capability source**:
"derived keys and decrypted credentials are injected into operation contexts
at the assembly layer, not passed as vault references to handlers." That
prose was correct but the mechanism was never specified.

The result was a contradiction in the spec documents. ADR-008 said the master
seed never crosses the network, but `operation-registry.md` showed
`vault/derive`, `vault/unlock`, and `vault/decrypt` registered as call protocol
operations — directly on the wire. Those two statements cannot both be true.
The contradiction arose because no injection mechanism existed in the
architecture, so the only way the docs could show a handler obtaining a key was
to expose vault operations over the call protocol.

This is a one-way door. Once secret material crosses the wire as a call
protocol operation, the attack surface is permanent:

- `vault/unlock` accepts a BIP39 mnemonic — the root of trust — over QUIC. A
  compromised peer, a logging accident, a tracing span, and the seed is gone.
- `vault/derive` returns a `DerivedKey`. The type redacts the private key in
  JSON today, but the operation's existence means a serialization change, a
  binary codec addition, or a wrapper change would leak it. The surface is
  the risk, not the current implementation.
- `vault/decrypt` accepts an encrypted blob and returns plaintext. Any
  authorized caller can decrypt any blob they possess.

The broader problem this decision addresses is structural: the industry
default for storing LLM provider keys, API tokens, and other credentials is
plaintext config files and environment variables (e.g., the aisdk Rust port
reads `std::env::var("GOOGLE_API_KEY")` and the example backend calls
`dotenv::dotenv()`). alknet replaces that with a vault. But the vault only
solves the storage problem; the flow problem — how decrypted material reaches
the code that needs it without crossing the network — requires its own
decision.

There is a separate, second axis that the current `OperationContext` conflates
with the secret-flow problem. A handler has two orthogonal credential concerns:

- **Identity (inbound)**: who is calling me? Resolved per-request from
  `AuthContext` (TLS client cert, auth token). Already in `OperationContext`.
- **Capabilities (outbound)**: what secrets can I use for outbound calls? This
  is the missing axis. A handler calling Google's API needs a decrypted Google
  API key. That is not the caller's identity — it is the handler's own outbound
  credential, provisioned by the assembly layer.

Mixing these two into one channel (e.g., stuffing secrets into
`OperationContext.metadata: HashMap<String, Value>`) is a leak risk: metadata
propagates through nested calls via `OperationEnv::invoke()`, so a secret
placed there by one handler would flow to every downstream operation.

## Decision

**1. The vault is assembly-layer only.**

The CLI binary (the `alknet` crate, or an embedded assembly layer) is the sole
component that talks to `VaultServiceHandle` directly. It unlocks the vault at
startup, derives and decrypts what each handler needs, and constructs handlers
with the results. No vault operation (`derive`, `decrypt`, `unlock`, `lock`)
is registered as a call protocol operation. The vault has no ALPN. The master
seed and derived private keys never enter the call protocol.

**2. Capabilities are the injection mechanism.**

A `Capabilities` type carries outbound secret material from the assembly layer
into handlers. Capabilities are distinct from identity (inbound auth) and
distinct from per-request metadata. The concrete shape of the `Capabilities`
type is a two-way door — to be decided during implementation of the
`alknet-call` crate. The one-way constraint is:

- Capabilities hold non-serializable, zeroized secret material. They cannot
  cross the call protocol wire even by accident — they are not
  `serde_json::Value`, they do not implement `Serialize`, and they do not
  appear in `EventEnvelope` payloads.
- Capabilities are injected at handler construction (the common case: a static
  decrypted API key held for the handler's lifetime) or scoped per-request for
  trusted-internal-only flows. They are never populated from call protocol
  inputs.

**3. The call protocol carries no secret material.**

This is a wire-level constraint on the call protocol, not a handler-level
convention. Secret material (private keys, API keys, mnemonics, decrypted
credentials, raw tokens) must not appear in:

- `call.requested` payloads (inputs)
- `call.responded` payloads (outputs)
- `OperationContext.metadata`

The wire format does not enforce this — it carries `serde_json::Value` — so the
constraint is architectural, enforced by the operation registry and by
convention. Operations that need to share public key material (e.g., for
identity verification) use a dedicated operation that returns only the public
component, never the private key.

**4. Adapters take credential sources, not static tokens.**

The `from_openapi` and `from_jsonschema` adapter patterns (defined in Rust in
alknet-call per ADR-013) register HTTP-backed operations. The TypeScript
`@alkdev/operations` `from_openapi` takes `config.auth: { token: "..." }` — a
static string. The Rust adapters take a credential source wired to the assembly
layer (a resolver, a capability handle, or an injected secret), not a literal
token. This is the integration point where the vault feeds credentials into
HTTP-backed operations: the assembly layer decrypts the token at startup and
provides it to the adapter at registration time.

**5. Handlers that need per-request vault access receive a scoped capability.**

The common case (a static decrypted API key) is covered by construction-time
injection. A narrower case — a handler that derives a child key for a specific
operation (e.g., signing for GitHub authentication) — receives a
scoped capability that can only derive at a restricted path set. This is still
not a vault reference: it is a restricted handle that performs a specific
derivation and returns the result to the handler, in-process. The handler
never sees the master seed. Whether this scoped capability is a distinct type
or modeled as a pre-derived key injected at construction is a two-way door
left to the `alknet-call` and `alknet-agent` crate specs.

## Consequences

**Positive:**

- The master seed and derived private keys never cross the network. The attack
  surface for the root of trust is local-only.
- The `OperationContext` gains a clean second axis (capabilities) instead of
  overloading `metadata` for secrets, preventing accidental propagation of
  secret material through nested calls.
- Handlers that need outbound credentials (the agent handler calling an LLM
  provider) receive them directly — no indirection through a `vault/derive`
  call, no latency, no failure mode where the vault must be reachable at call
  time.
- The adapter contract (OQ-15) gains a concrete shape: adapters take a
  credential source from the assembly layer, not a static token. This makes
  the `from_openapi` / `from_jsonschema` / `from_call` patterns safe by
  construction.
- The model is structurally incompatible with the env-var / plaintext-config
  default. There is no `std::env::var("API_KEY")` path — the only way a handler
  gets a credential is through a capability, and the only way a capability is
  populated is through the assembly layer from the vault.

**Negative:**

- The assembly layer (CLI binary) has more construction-time responsibility: it
  must know which handlers need which credentials and wire them. This is
  expected — the CLI assembles everything (ADR-008).
- Adding a new handler that needs a new credential requires updating the
  assembly layer, not just registering an operation. This is a feature, not a
  bug: it forces an explicit decision about what secret material a handler
  needs.
- Remote vault administration (unlock a running node's vault over the network)
  is not supported by this decision. If that capability is needed in the
  future, it would require a separate, heavily restricted mechanism (admin
  scope, mTLS-only, never expose the mnemonic over an unauthenticated channel)
  and its own ADR. This decision does not close that door; it simply does not
  open it.
- The `Capabilities` type shape is not fully specified here. The one-way
  constraint (non-serializable, zeroized, injection-only) is fixed; the
  concrete API is a two-way door for the `alknet-call` spec.

## Assumptions

These are the load-bearing assumptions. If any of them breaks, the decision
should be revisited:

1. **Handlers need credentials at construction time or at call time, not
   dynamically discovered at call time.** If a handler needs to derive a key
   at an unpredictable path determined by call input, the scoped-capability
   model still covers it (the handler holds a scoped vault access), but the
   surface area is larger. The assumption is that this case is rare.
2. **The call protocol's threat model excludes the assembly layer.** The CLI
   binary is trusted to hold the vault handle and inject capabilities. If the
   assembly layer is compromised, all handlers' capabilities are compromised.
   This is the same trust boundary as ADR-008.
3. **No legitimate use case requires returning a private key over the wire.**
   Public key sharing (identity verification, encryption to a recipient) is
   the only cross-node key material flow. If a use case for returning a
   private key emerges (e.g., a key-escrow service), it needs its own ADR and a
   very different threat model.
4. **Adapters are registered at startup, not at call time.** The credential
   source is wired to the adapter when the operation is registered, not when
   the operation is invoked. This is consistent with OQ-04 (static
   registration at startup).

## References

- ADR-003: Crate decomposition (alknet-vault is standalone)
- ADR-005: irpc as call protocol foundation
- ADR-008: Vault integration point (capability source — this ADR specifies the
  mechanism that ADR-008 described in prose)
- ADR-009: One-way door decision framework
- ADR-013: Rust as canonical implementation language
- OQ-15: Call protocol client and adapter contract (this ADR constrains the
  adapter contract: adapters take credential sources, not static tokens)
- OQ-16: Safe vault operations for call protocol exposure (resolved by this
  ADR: none, for now)
- alknet-vault implementation: `crates/alknet-vault/`