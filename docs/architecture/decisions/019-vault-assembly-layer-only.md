# ADR-019: Vault Assembly-Layer-Only Access

## Status

Accepted

## Context

ADR-008 established that the vault is a **capability source** — the CLI
binary unlocks it at startup, derives and decrypts the credentials each
handler needs, and injects the results into handler capabilities. ADR-014
specified the injection mechanism (`Capabilities` on `OperationContext`) and
locked the constraint that no vault operations are registered in the call
protocol.

These ADRs answer *how the vault integrates with the rest of alknet*. This
ADR answers a narrower question that the vault's own spec needs to be
explicit about: **what is the vault's access model from its own
perspective?**

The vault provides a `VaultServiceHandle` with `unlock`, `lock`,
`derive_ed25519`, `derive_encryption_key`, `derive_ethereum_key`,
`derive_password`, `encrypt`, and `decrypt` methods. Who is allowed to call
these, and through what path?

The candidates:

1. **Handlers call the vault directly** — each handler holds a
   `VaultServiceHandle` and derives keys at call time. This was the
   pre-ADR-008 model and is rejected: it exposes the vault to every handler,
   requires the vault to enforce per-handler path restrictions itself, and
   means the master seed is reachable from every call path.

2. **The call protocol exposes vault operations** — `vault/derive`,
   `vault/decrypt`, `vault/unlock` registered as operations. This was the
   contradiction ADR-014 resolved: the master seed and mnemonics would cross
   the wire.

3. **The assembly layer is the sole caller** — the CLI binary (or an
   embedded assembly layer) holds the `VaultServiceHandle`, calls vault
   methods at startup and (rarely) at call time through scoped capabilities,
   and injects results into handlers. Handlers never hold a vault reference.

## Decision

**The assembly layer is the sole direct caller of the vault.** This
restates ADR-008/ADR-014 from the vault's perspective and makes the access
model explicit in the vault's own spec.

### What the assembly layer does

At startup:

1. Constructs `VaultServiceHandle::new()`
2. Unlocks with a mnemonic (from a secure prompt, a file, or a hardware
   token) and optional passphrase
3. Derives the keys each handler needs (identity, SSH host, TLS identity,
   signing keys)
4. Decrypts the credentials each handler needs (LLM provider API keys,
   OAuth tokens)
5. Constructs handlers with the derived/decrypted material injected into
   their `Capabilities`
6. Registers the handlers in the `OperationRegistry`
7. Starts the endpoint

After startup, the vault is typically not called again. The common case is
construction-time injection — a handler holds a static decrypted API key for
its lifetime.

### What handlers do NOT do

Handlers never:
- Hold a `VaultServiceHandle` reference
- Call `derive_*`, `encrypt`, or `decrypt` directly
- Receive the master seed or mnemonic
- Import `alknet_vault` as a dependency

Handlers receive secret material through `OperationContext.capabilities`
(ADR-014). The `Capabilities` type holds non-serializable, zeroized secret
material that the assembly layer populated at construction time.

### The scoped-capability exception

The narrow exception is a handler that needs a child key at an
unpredictable path determined by call input (e.g., signing for a specific
GitHub repo). This handler receives a **scoped capability** — a restricted
handle that performs a specific derivation at a restricted path set and
returns the result in-process. The handler never sees the master seed and
never holds a full `VaultServiceHandle`.

The scoped capability is still a capability (it lives on
`OperationContext.capabilities`), not a vault reference. Whether it is a
distinct type or a pre-derived key injected at construction is a two-way
door for the alknet-call and alknet-agent crate specs (ADR-014).

### No vault operations on the wire

The vault has no ALPN (ADR-003, ADR-008). No vault operation is registered
in the call protocol's `OperationRegistry` (ADR-014). The master seed,
mnemonics, and derived private keys never appear in `call.requested`
payloads, `call.responded` payloads, or `OperationContext.metadata`
(ADR-014).

If a future use case requires exposing a vault operation over the call
protocol (e.g., a restricted `vault/public-key` operation that returns only
public key material for identity verification), it requires its own ADR
with an explicit threat model justification. This decision does not close
that door; it simply does not open it.

## Consequences

**Positive:**
- The master seed is reachable from exactly one place: the assembly layer.
  The attack surface for the root of trust is a single process boundary, not
  a distributed set of handlers.
- Handlers don't need to enforce path restrictions — they don't have the
  vault. The scoped-capability mechanism enforces restrictions by
  construction.
- The vault's API is consumed by one caller. This simplifies the vault's
  threat model: it doesn't need per-caller authentication, rate limiting, or
  path-based access control. The assembly layer is trusted.
- The vault can be tested in isolation — `VaultServiceHandle::new()` →
  `unlock_new(24)` → `derive_*` is the test pattern, with no networking or
  handler mockery.

**Negative:**
- The assembly layer has more construction-time responsibility: it must
  know which handlers need which credentials and wire them. This is expected
  — the CLI assembles everything (ADR-008).
- Adding a new handler that needs a new credential requires updating the
  assembly layer, not just registering an operation. This is a feature:
  it forces an explicit decision about what secret material a handler needs.
- Remote vault administration (unlock a running node's vault over the
  network) is not supported. If needed in the future, it requires a
  separate, heavily restricted mechanism (admin scope, mTLS-only, never
  expose the mnemonic over an unauthenticated channel) and its own ADR.

## Assumptions

1. **The assembly layer is trusted.** The CLI binary holds the vault handle
   and is the trust boundary. If the assembly layer is compromised, all
   handlers' capabilities are compromised. This is the same trust boundary
   as ADR-008 and ADR-014.

2. **Handlers need credentials at construction time or at call time, not
   dynamically discovered at call time.** If a handler needs to derive a key
   at an unpredictable path determined by call input, the scoped-capability
   model covers it (the handler holds a scoped vault access), but the
   surface area is larger. The assumption is that this case is rare.

3. **No legitimate use case requires returning a private key over the
   wire.** Public key sharing (identity verification, encryption to a
   recipient) is the only cross-node key material flow. If a use case for
   returning a private key emerges (e.g., a key-escrow service), it needs
   its own ADR and a very different threat model.

## References

- ADR-003: Crate decomposition (alknet-vault is standalone)
- ADR-008: Vault integration point (CLI-embedded, capability source)
- ADR-014: Secret material flow and capability injection (the injection
  mechanism this ADR relies on)
- ADR-018: Vault as standalone crate (the independence this ADR preserves)
- [crates/vault/service.md](../crates/vault/service.md)
- [crates/vault/README.md](../crates/vault/README.md)