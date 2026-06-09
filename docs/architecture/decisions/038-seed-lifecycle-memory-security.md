# ADR-038: Seed Lifecycle and Memory Security

## Status

Accepted

## Context

The alknet-secret crate holds the master BIP39 seed phrase in RAM. This seed is
the root of trust for all derived keys (identity, encryption, signing). If the
seed is leaked — through memory dumps, swap files, or core dumps — an attacker
can derive every key in the system.

Security-conscious key management systems typically employ three defenses:

1. **Zeroize**: Overwrite sensitive memory before deallocating. Prevents
   stale-data reads from freed memory.

2. **Memory locking** (`mlock`/`VirtualLock`): Prevent the OS from paging
   sensitive RAM to disk. Prevents swap-file leakage.

3. **Constant-time comparison**: Prevent timing side-channels when comparing
   keys or tokens.

The question is: which of these should alknet-secret adopt in v1, and which
should be deferred?

## Decision

**Phase 3 (v1): Zeroize only. Defer mlock and constant-time comparison to
Phase B.**

- All sensitive types (seed bytes, derived private keys, passphrase strings)
  derive `Zeroize` and implement `Drop` to call `zeroize()` before deallocation.
- The `Lock` operation calls `zeroize()` on the seed and all cached derived
  keys, then drops them.
- `mlock`/`VirtualLock` and constant-time comparison are not included in v1.

### Rationale for deferring mlock

1. **Complexity**: `mlock` requires root/CAP_IPC_LOCK on Linux or
   `SeLockMemory` on Windows. The crate should work in unprivileged contexts
   (development, testing, single-user nodes) without requiring system
   configuration changes.

2. **Performance**: `mlock` locks physical pages, which are typically 4KB.
   Locking many small buffers wastes physical memory. The seed (64 bytes) and
   derived keys (32–64 bytes each) are tiny — the real risk is swap-file
   leakage, which `zeroize` partially mitigates by wiping before free.

3. **Deployment flexibility**: Production head nodes running as root or with
   `CAP_IPC_LOCK` can add `mlock` in Phase B. Development and CLI nodes
   shouldn't need it.

4. **Audit surface**: `mlock` introduces platform-specific code paths (Linux
   vs macOS vs Windows) that should be audited together, not bolted on
   incrementally.

### Rationale for deferring constant-time comparison

The `SecretProtocol` service receives requests over irpc (local mpsc or remote
QUIC). Comparison timing is not observable by callers — they send a message and
wait for a response. The comparison that matters (auth token verification) is
in alknet-core's `IdentityProvider`, not in alknet-secret. Key derivation
results (DerivedKey) are not compared against attacker-controlled input within
this crate.

### Zeroize implementation

```rust
use zeroize::Zeroize;

#[derive(Zeroize)]
#[zeroize(drop)]
struct SeedHolder {
    seed: Vec<u8>,
}

#[derive(Zeroize)]
#[zeroize(drop)]
struct DerivedKeyCache {
    keys: HashMap<String, Vec<u8>>,
}
```

`#[zeroize(drop)]` ensures that `Drop` calls `zeroize()` on all fields,
overwriting memory before deallocation. This is a compile-time guarantee —
forgetting to zeroize a field is a compile error.

### Lock lifecycle

```
Unlock(passphrase)
  → validate mnemonic (if restoring) or generate new
  → derive master key from seed
  → store seed in SeedHolder (Zeroize-protected)
  → cache empty (keys derived on demand)

DeriveEd25519/DeriveEncryptionKey/Encrypt/Decrypt
  → require unlocked state (error if locked)
  → derive key, return result
  → optionally cache derived key

Lock
  → zeroize all cached derived keys
  → zeroize seed
  → drop all sensitive material
  → service returns to locked state
```

## Consequences

- **Positive**: Zeroize is zero-cost at compile time, minimal dependency
  (`zeroize` crate is ~500 lines, no `unsafe` on stable), and provides
  meaningful protection against stale-memory reads.
- **Positive**: Lock effectively purges all sensitive material. After Lock,
  the process memory contains no useful secret data.
- **Positive**: No platform-specific code paths in v1. The crate compiles and
  runs everywhere without privilege requirements.
- **Negative**: Without `mlock`, the OS can page the seed to swap before
  zeroization occurs. This is a window of vulnerability that Phase B closes.
  The risk is acceptable for v1 because swap-file extraction requires root
  access or physical access to the machine — the same threat model as reading
  process memory directly.
- **Negative**: Without constant-time comparison, timing side-channels exist
  in theory. In practice, no comparison in alknet-secret operates on
  attacker-controlled input, so the risk is nil within this crate.
- **Negative**: `zeroize` adds a dependency. The `zeroize` crate is widely
  used in Rust crypto (ring, ed25519-dalek, x25519-dalek) and is a de facto
  standard.

## References

- [secret-service.md](../secret-service.md) — Security model, Lock/Unlock lifecycle
- [ADR-027](027-crate-decomposition.md) — Crate decomposition (alknet-secret is independent)
- [credentials.md](../credentials.md) — SecretStoreCredentialProvider integration
- `zeroize` crate — https://crates.io/crates/zeroize