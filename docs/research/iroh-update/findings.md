# iroh / irpc Version Update — Findings & Migration Plan

**Status:** Findings complete. Migration plan drafted; not yet applied. Two cheap wins identified (dead `irpc` dep, stale-pin hypothesis ruled out), one real migration traced end-to-end against iroh 1.0.2.
**Date:** 2026-07-08
**Scope:** Resolves the version-gap open question deferred by `alknet-blobs-external-store-probe.md` §"Versioning reality" and §"What this closes and what remains". Verifies latest stable versions on crates.io (not training-data defaults), determines whether alknet-core's `iroh 0.35` dep is load-bearing or a stale pin, and traces the exact `Preset` trait surface in iroh 1.0.x so the migration is concrete rather than a TODO.

---

## TL;DR

1. **Latest stable versions verified against the crates.io API** (2026-07-08): `iroh 1.0.2`, `iroh-blobs 0.103.0`, `irpc 0.17.0`, `irpc-derive 0.17.0`, `bao-tree 0.16.0`. iroh-blobs has **not** moved past 0.103 — no pre-releases, no newer stable. The probe's target version is the current latest; the 4-line/20-line fork premise still holds.

2. **`iroh 0.35` in alknet-core is load-bearing, not a stale pin.** The "pure Cargo.toml change" hypothesis is wrong. `crates/alknet-core/src/endpoint.rs` uses ~15 distinct iroh APIs under `#[cfg(feature = "iroh")]`. Three of them broke between 0.35 and 1.0 and require code edits. The bump is small (~3 edits in `endpoint.rs`) but real.

3. **`irpc 0.16` in the workspace is a dead dep — the actual cheap win.** The probe doc claimed `alknet-call` "already uses `irpc` for structured RPC." That is false: zero `.rs` files in the workspace reference `irpc`. The dep is declared in `Cargo.toml` and never imported. Dropping it (or bumping to 0.17) dissolves the `irpc 0.16 vs 0.17` half of the version gap with zero code impact.

4. **The iroh 1.0.x migration target is stable.** All three flagged APIs are byte-identical between iroh 1.0.0 and 1.0.2 (the actual latest). Pinning `iroh = "1.0"` resolves to 1.0.2 and the migration edits are stable against it. No "do it again when they update" risk on the 1.0.x line.

5. **The `Preset` trace is complete.** alknet-core should use `iroh::endpoint::presets::Minimal` — it sets only the now-mandatory `crypto_provider` and leaves relay mode, address lookup, ALPNs, and secret key for alknet-core to configure as it already does. This matches the pattern iroh-blobs 0.103's own tests use (`tests.rs:506,525`).

---

## 1. Latest stable versions (crates.io API, 2026-07-08)

Queried `https://crates.io/api/v1/crates/<name>` for each crate. Versions below are `max_stable_version` (== `newest_version` for all of these — no pre-releases in flight).

| crate | latest stable | updated | alknet-core pin | iroh-blobs 0.103 requires | gap |
|---|---|---|---|---|---|
| `iroh` | **1.0.2** | 2026-07-06 | `0.35` (optional) | `^1.0.0` (runtime) | 2 majors |
| `iroh-base` | **1.0.2** | 2026-07-06 | (transitive 0.35) | `^1.0.0` (runtime) | follows iroh |
| `iroh-relay` | **1.0.2** | 2026-07-06 | (transitive 0.35) | — | follows iroh |
| `iroh-blobs` | **0.103.0** | 2026-06-15 | — | — | target version, no newer |
| `irpc` | **0.17.0** | 2026-06-15 | `0.16` (workspace, **unused**) | `^0.17.0` (runtime) | 1 minor — dead dep |
| `irpc-derive` | **0.17.0** | 2026-06-15 | `0.16` (workspace, **unused**) | — | same as irpc |
| `bao-tree` | **0.16.0** | 2025-11-04 | (not declared) | `^0.16` (runtime) | alknet-blobs will add |

### iroh-blobs has not moved past 0.103

Recent version history (crates.io `/versions` endpoint):

```
0.103.0  2026-06-15
0.102.0  2026-05-27
0.101.0  2026-05-08
0.100.0  2026-04-20
0.99.0   2026-03-17
0.98.0   2026-01-28
```

`max_stable == newest == 0.103.0`. No yanked releases, no pre-releases. The probe's `Command`/`Store`/`from_sender`/`Scope` surface findings (all verified against `iroh-blobs-0.103.0/src/api.rs:33,299,303` and `api/proto.rs:693`) are current. The fork premise holds.

### iroh-blobs 0.103's iroh/irpc deps are runtime, not dev

The crates.io API's dependency `kind` field was misleading (reported everything as "build"). The authoritative source is the raw `Cargo.toml` in the cargo registry cache:

```
[dependencies.iroh]
version = "1.0.0"
default-features = false

[dependencies.iroh-base]
version = "1.0.0"

[dependencies.irpc]
version = "0.17.0"
features = ["spans", "stream", "derive", "varint-util"]
default-features = false
```

All three are in `[dependencies]`, not `[dev-dependencies]`. So any crate depending on `iroh-blobs 0.103` pulls `iroh 1.0` and `irpc 0.17` transitively. The version gap is real and load-bearing for alknet-blobs.

---

## 2. `iroh 0.35` is load-bearing (not a stale pin)

The other session's hypothesis: "if nothing in alknet-core's current code exercises iroh 0.35-specific APIs, bumping to 1.0 may be a pure Cargo.toml change." Verified by grep — this is wrong.

`crates/alknet-core/src/endpoint.rs` uses these iroh APIs under `#[cfg(feature = "iroh")]`:

- `iroh::Endpoint::builder()` — `endpoint.rs:681`
- `iroh::SecretKey::from_bytes` / `SecretKey::generate` — `endpoint.rs:684,688`
- `iroh::RelayMap::from` / `RelayMode::Custom` / `RelayMode::Disabled` — `endpoint.rs:694-697`
- `iroh::Endpoint::accept()`, `Endpoint::close()` — `endpoint.rs:403,228`
- `iroh::endpoint::{Connection, SendStream, RecvStream, ConnectionError, VarInt, ApplicationClose}` — `endpoint.rs:441`; `types.rs:232,240,366,473,504,824-856`
- `iroh::RelayUrl` — `config.rs:28`

Three of these broke between 0.35 and 1.0 (see §4). The pin is load-bearing.

---

## 3. `irpc 0.16` is a dead dep (the actual cheap win)

The probe doc (`alknet-blobs-external-store-probe.md:163`) claims:

> **`alknet-call`** — already uses `irpc` (workspace dep `irpc = "0.16"`) for structured RPC.

**This is false.** Grepped every `.rs` file under `crates/alknet-call/src/` and `crates/alknet-call/tests/` for `irpc` — **zero matches**. The only `irpc` reference in alknet-call is `Cargo.toml:18` (`irpc = { workspace = true }`). The actual wire protocol in `src/protocol/wire.rs` is hand-rolled length-prefixed JSON (`EventEnvelope`/`ResponseEnvelope`/`FrameFramedReader`/`FrameFramedWriter`) — no `irpc` types, no `#[rpc_requests]` macro, no `irpc::Client`.

The workspace `irpc = "0.16"` / `irpc-derive = "0.16"` deps (`Cargo.toml:19-20`) are **declared but unused by any crate in the workspace**.

### Why this matters for the version gap

The only thing pinning `irpc 0.16` in the workspace is a dead dep. Two options, both free:

1. **Drop the dead dep entirely** (cleanest): remove `irpc`/`irpc-derive` from workspace `Cargo.toml:19-20` and from `alknet-call/Cargo.toml:18`. Re-add when `alknet-blobs` actually needs `irpc 0.17` (it will, as a real dep).
2. **Bump to 0.17 now**: change workspace `Cargo.toml` to `irpc = "0.17"` / `irpc-derive = "0.17"`. Zero code impact since nothing uses it. Gets the version in place for `alknet-blobs`.

Either way, the `irpc 0.16 vs 0.17` half of the gap dissolves with no migration work. This is the "stale pin" cheap win — it just wasn't where the probe said it was (the probe pointed at `iroh 0.35`; the stale pin was `irpc 0.16`).

---

## 4. The iroh 0.35 → 1.0.2 migration (3 edits)

Diffed the actual pub surfaces of `iroh-0.35.0` vs `iroh-1.0.0` (cached in cargo registry) and `iroh-1.0.2` (fetched from static.crates.io). Three APIs broke; everything else alknet-core uses is stable.

### 4.1 Breaking changes (require code edits)

| location | 0.35 | 1.0.2 | fix |
|---|---|---|---|
| `endpoint.rs:681` | `iroh::Endpoint::builder()` (no args) | `iroh::Endpoint::builder(preset: impl Preset)` — `endpoint.rs:950` | pass `presets::Minimal` (see §5) |
| `endpoint.rs:688` | `iroh::SecretKey::generate(&mut csprng)` — `iroh-base-0.35.0/src/key.rs:273` takes `R: CryptoRngCore` | `iroh::SecretKey::generate()` — `iroh-base-1.0.2/src/key.rs:318` takes no args (uses internal rng) | drop the `&mut csprng` arg + the `let mut csprng = ...` line |
| `endpoint.rs:684` | `iroh::SecretKey::from_bytes(&[u8;32]) -> Result<Self, SignatureError>` — `iroh-base-0.35.0/src/key.rs:109` | `iroh::SecretKey::from_bytes(&[u8;32]) -> Result<Self, KeyParsingError>` — `iroh-base-1.0.2/src/key.rs:122` (error type renamed) | add `?` + map `KeyParsingError` to `EndpointError::TlsConfig` |

All three confirmed **byte-identical between iroh 1.0.0 and iroh 1.0.2** — the migration target is stable. Pinning `iroh = "1.0"` resolves to 1.0.2 and these edits hold.

### 4.2 Stable APIs (no edits needed)

Confirmed by diffing `iroh-0.35.0` vs `iroh-1.0.2` cached/fetched sources:

- `iroh::RelayMap::from<RelayUrl>` — stable (`iroh-relay-1.0.0/src/relay_map.rs:181`)
- `iroh::RelayMode::{Custom, Disabled}` — stable (`iroh-1.0.2/src/endpoint.rs:1922`). Note: 1.0 adds `RelayMode::Default` and `RelayMode::Staging` variants, but alknet-core only uses `Custom`/`Disabled`, which are unchanged.
- `iroh::Endpoint::accept()` — stable (`iroh-1.0.2/src/endpoint.rs:1162`)
- `iroh::Endpoint::close()` — stable (`iroh-1.0.2/src/endpoint.rs:1703`)
- `iroh::endpoint::{Connection, SendStream, RecvStream, ConnectionError, VarInt, ApplicationClose}` — stable (module path unchanged)
- `Builder::secret_key`, `Builder::alpns`, `Builder::relay_mode`, `Builder::bind` — all stable (signatures unchanged in 1.0.2)

### 4.3 The 1.0 Builder also changed structurally (context, not a direct edit)

The 0.35 `Builder::default()` set `relay_mode: default_relay_mode()` (n0 prod relays) and had no `crypto_provider` field. The 1.0 `Builder::empty()` starts with `RelayMode::Disabled` and **requires** `crypto_provider` to be set before `bind()` (returns `BindError::InvalidCryptoProvider` otherwise — `endpoint.rs:230`).

This is why the `Preset` arg to `builder()` exists: it's the mechanism for setting the mandatory `crypto_provider`. alknet-core's existing explicit `.relay_mode(...)` call (line 695/697) overrides whatever the preset sets, so the preset only needs to supply the crypto provider. That's exactly what `presets::Minimal` does (see §5).

---

## 5. The `Preset` trait trace (iroh 1.0.x)

**Location:** `iroh-1.0.2/src/endpoint/presets.rs` (re-exported as `iroh::endpoint::presets`)

### The trait

```rust
// presets.rs:21
pub trait Preset {
    fn apply(self, builder: Builder) -> Builder;
}
```

Single-method. `Endpoint::builder(preset)` calls `preset.apply(Builder::empty())`.

### The four built-in presets

| preset | cfg gate | what `apply` sets | use case |
|---|---|---|---|
| `presets::Empty` | always | nothing — returns builder as-is | full manual control; `bind()` fails unless you set `crypto_provider` yourself |
| `presets::Minimal` | `with_crypto_provider` | **only `crypto_provider`** (ring if `tls-ring`, aws-lc-rs if `tls-aws-lc-rs`; ring wins if both) | you want control over relay/discovery but not crypto boilerplate |
| `presets::N0` | `with_crypto_provider` | `Minimal` + n0 DNS address lookup (pkarr publisher + DNS resolver) + `RelayMode::Default` (n0 prod relays) | the "0.35 `builder()` default" equivalent — production with n0 infra |
| `presets::N0DisableRelay` | `with_crypto_provider` | `N0` but overrides relay to `RelayMode::Disabled` | n0 DNS discovery, no relay |

The cfg gate `with_crypto_provider` is set by iroh's `build.rs:8` to `any(feature = "tls-ring", feature = "tls-aws-lc-rs")`. iroh's **default features include `tls-ring`**, so `Minimal`/`N0`/`N0DisableRelay` are available unless you disable default features.

### Why `presets::Minimal` is the right choice for alknet-core

alknet-core's `build_iroh_endpoint` (`endpoint.rs:676-704`) does three things after constructing the builder:
1. `builder.secret_key(...)` — sets the iroh identity (line 685/688)
2. `builder.alpns(alpns.to_vec())` — sets ALPNs (line 691)
3. `builder.relay_mode(...)` — **explicitly** sets either `RelayMode::Custom(relay_map)` or `RelayMode::Disabled` based on config (lines 693-698)

`presets::N0` would be wrong because:
- It sets `RelayMode::Default` (n0 prod relays), but alknet-core **immediately overrides** `relay_mode`. Wasteful, and worse:
- It adds n0 DNS address-lookup services (`PkarrPublisher::n0_dns()` + `DnsAddressLookup::n0_dns()` at `presets.rs:121,133`). This would publish alknet node identities to `iroh.link` DNS — not desired for a self-hosted alknet deployment that configures its own relay.
- alknet-core's 0.35 `builder()` default did **not** set address-lookup (0.35's `Builder::default()` has no `address_lookup` field — that concept didn't exist).

`presets::Minimal` is the exact match: it sets **only** the `crypto_provider` (mandatory since 1.0), and leaves relay mode, address lookup, ALPNs, and secret key for alknet-core to set as it already does.

### Why `presets::Empty` is wrong

`Empty` sets nothing, so `bind()` returns `BindError::InvalidCryptoProvider` (`endpoint.rs:230`) unless alknet-core adds an explicit `.crypto_provider(...)` call. `Minimal` is strictly better: it handles the crypto provider selection via iroh's feature flags, which is the intended mechanism.

### Consistency with iroh-blobs 0.103

iroh-blobs 0.103's own tests use the same pattern (`src/tests.rs:506,525`):

```rust
let ep = Endpoint::builder(presets::Minimal)
    .relay_mode(RelayMode::Default)
    .address_lookup(sp.clone())
    .bind()
    .await?;
```

`Minimal` + explicit relay mode + explicit address lookup. alknet-core should follow the same shape: `Minimal` + explicit relay mode (no address lookup, since alknet doesn't use iroh's DNS discovery). No preset conflict, no duplicate crypto provider when alknet-blobs (which depends on both) is added later.

---

## 6. Cargo.toml changes

### alknet-core (`crates/alknet-core/Cargo.toml:21`)

```toml
# from:
iroh = { version = "0.35", optional = true }
# to:
iroh = { version = "1.0", optional = true, default-features = false, features = ["tls-aws-lc-rs"] }
```

**Why `default-features = false`:** iroh's defaults include `tls-ring`. alknet-core's quinn path already uses `rustls::crypto::aws_lc_rs::default_provider()` (`endpoint.rs:551,631`). To keep iroh on the same crypto provider (avoid pulling in both ring and aws-lc-rs), disable defaults and enable only `tls-aws-lc-rs`. This makes `presets::Minimal` select aws-lc-rs (the `#[cfg(all(feature = "tls-aws-lc-rs", not(feature = "tls-ring")))]` branch at `presets.rs:71`), matching the quinn path's existing provider choice.

**What `default-features = false` drops:** `tls-ring`, `metrics`, `fast-apple-datapath`, `portmapper`. None are used by alknet-core today. If metrics or portmapper are wanted later, add them explicitly: `features = ["tls-aws-lc-rs", "metrics"]`. The load-bearing choice is `tls-aws-lc-rs` + not-`tls-ring` so `Minimal` picks aws-lc-rs.

### Workspace (`Cargo.toml:19-20`) — the dead dep

```toml
# from:
irpc = "0.16"
irpc-derive = "0.16"
# to (option A — cleanest, recommended):
# (delete both lines; re-add as 0.17 when alknet-blobs needs them)
#
# or (option B — bump now, zero code impact):
irpc = "0.17"
irpc-derive = "0.17"
```

### alknet-call (`crates/alknet-call/Cargo.toml:18`) — the dead dep consumer

```toml
# from:
irpc = { workspace = true }
# to (option A):
# (delete the line — nothing in src/ imports irpc)
#
# or (option B — leave it, it resolves to the bumped workspace 0.17):
irpc = { workspace = true }
```

Option A (drop) is recommended since the dep is genuinely unused; re-adding when needed is trivial. Option B keeps the declaration in case someone planned to wire irpc into alknet-call soon, but the current code doesn't use it.

---

## 7. The migration diff for `build_iroh_endpoint`

`crates/alknet-core/src/endpoint.rs:676-704`:

```rust
 async fn build_iroh_endpoint(
     static_config: &StaticConfig,
     alpns: &[Vec<u8>],
 ) -> Result<iroh::Endpoint, EndpointError> {
-    let mut builder = iroh::Endpoint::builder();
+    let mut builder = iroh::Endpoint::builder(iroh::endpoint::presets::Minimal);

     if let Some(TlsIdentity::RawKey(secret_key)) = static_config.tls_identity.as_ref() {
-        let iroh_key = iroh::SecretKey::from_bytes(&secret_key.as_bytes());
+        let iroh_key = iroh::SecretKey::from_bytes(&secret_key.as_bytes())
+            .map_err(|e| EndpointError::TlsConfig(io::Error::other(e)))?;
         builder = builder.secret_key(iroh_key);
     } else {
-        let mut csprng = rand::rngs::OsRng;
-        builder = builder.secret_key(iroh::SecretKey::generate(&mut csprng));
+        builder = builder.secret_key(iroh::SecretKey::generate());
     }

     builder = builder.alpns(alpns.to_vec());

     if let Some(relay_url) = static_config.iroh_relay.as_ref() {
         let relay_map = iroh::RelayMap::from(relay_url.clone());
         builder = builder.relay_mode(iroh::RelayMode::Custom(relay_map));
     } else {
         builder = builder.relay_mode(iroh::RelayMode::Disabled);
     }

     builder
         .bind()
         .await
         .map_err(|e| EndpointError::BindFailed(io::Error::other(e)))
 }
```

**Three edits:**
1. `builder()` → `builder(presets::Minimal)` — supplies the now-mandatory `crypto_provider` (aws-lc-rs via the `tls-aws-lc-rs` feature).
2. `from_bytes(...)` now returns `Result<Self, KeyParsingError>` — add `?` + map to `EndpointError::TlsConfig`. The `TlsConfig` variant already exists (quinn path uses it at `endpoint.rs:554`); no new enum variant needed.
3. `generate(&mut csprng)` → `generate()` — the rng arg is gone; `OsRng` is used internally. Delete the `let mut csprng = rand::rngs::OsRng;` line.

### `EndpointError` note

`from_bytes`'s new `KeyParsingError` needs a conversion. The cleanest path is `.map_err(|e| EndpointError::TlsConfig(io::Error::other(e)))?` (reusing the existing `TlsConfig` variant), which is what the diff above does. If a dedicated variant is preferred (`InvalidKey(KeyParsingError)`), that's a 1-line enum change + `From` impl — but the reuse path is simpler and matches how the quinn path handles crypto errors.

---

## 8. Survey: what else may need updating

This section catalogs other places in the workspace that touch iroh/irpc and may need attention as part of the bump. Each item is a "check this" — not all will require edits, but all should be verified when the migration branch is opened.

### 8.1 `crates/alknet-core/src/types.rs` — iroh stream/connection types

Uses `iroh::endpoint::{SendStream, RecvStream, Connection, ConnectionError, VarInt, ApplicationClose}` (lines 232, 240, 366, 473, 504, 824-856). All confirmed stable in 1.0.2 (same module paths, same enum variants). **Likely no edits**, but the `ConnectionError` match arms at `types.rs:824-856` should be re-checked against 1.0.2's `ConnectionError` enum in case any variants were added (new variants don't break exhaustive matches if the match is on `&_` or has a catch-all, but if it's exhaustive without `_`, the compiler will flag it).

### 8.2 `crates/alknet-core/src/fingerprint.rs` — ed25519 fingerprint parity test

The test `fingerprint_from_ed25519_spki_matches_iroh_format` (line 220) constructs an `iroh_fp = format!("ed25519:{}", hex::encode(raw_key))` and compares it against the quinn SPKI path. It doesn't import iroh types — it just asserts string-format parity. **No edits expected.**

### 8.3 `crates/alknet-core/src/config.rs:28` — `iroh::RelayUrl`

`pub iroh_relay: Option<iroh::RelayUrl>`. `RelayUrl` is stable across 0.35 → 1.0.2 (`iroh-base-1.0.2/src/relay_url.rs:21`, same `pub struct RelayUrl(Arc<Url>)` shape). **No edits expected.**

### 8.4 `Cargo.lock` — will regenerate on `cargo update -p iroh`

After bumping `Cargo.toml`, `cargo update -p iroh` (and `-p iroh-base`, `-p iroh-relay`) will pull 1.0.2 and its transitive deps. The lockfile currently pins `iroh 0.35.0`, `irpc 0.16.0`, `irpc-derive 0.16.0` (verified at `Cargo.lock:1998-1999, 2202-2203, 2224-2225`). Expect a sizable lockfile diff (iroh's dep tree changed substantially between 0.35 and 1.0 — `magicsock` → `socket`, `disco` removed, `iroh-dns` added, etc.).

### 8.5 `rand` dependency — may become unused

The 0.35 path uses `rand::rngs::OsRng` at `endpoint.rs:687`. After the migration (`generate()` takes no rng), this import may become dead. Check whether `rand` is used elsewhere in alknet-core before removing it from `Cargo.toml`. (`ed25519-dalek` at `Cargo.toml:39` uses `rand_core` features, but that's a separate dep.)

### 8.6 `crates/alknet-call/` — the dead `irpc` dep

Covered in §3 and §6. No source changes; just `Cargo.toml` cleanup.

### 8.7 Tests under `#[cfg(feature = "iroh")]` in `endpoint.rs`

`endpoint.rs:1107-1150` (`endpoint_constructs_with_iroh_raw_key_identity`, `iroh_endpoint_runs_accept_loop_and_shutdown`) and `endpoint.rs:1326-1364` (`has_iroh_identity_*`) construct test configs with `iroh_relay: None` and exercise `build_iroh_endpoint`. These will hit the same 3 breaking APIs and need the same edits applied (they call into `build_iroh_endpoint`, so if that function is fixed, the tests should pass — but verify the test helpers don't construct `iroh::SecretKey` or `iroh::Endpoint::builder()` directly).

### 8.8 Future: `alknet-blobs` crate (not yet created)

When `alknet-blobs` is added as a workspace member, it will depend on `iroh-blobs = "0.103"`, which pulls `iroh 1.0` and `irpc 0.17` transitively. At that point the workspace must have a single consistent iroh version. The migration in this doc (bumping alknet-core to `iroh 1.0`) is the prerequisite for that — without it, `alknet-blobs` and `alknet-core` would depend on different iroh majors and Cargo would fail to resolve (or pick one, breaking the other).

The `~20-line fork` of iroh-blobs (widening `Store::from_sender`/`ref_from_sender` + `ApiClient` + `Scope` field, per `alknet-blobs-external-store-probe.md` §"The four items that must be made pub") targets the 0.103 line. Since iroh-blobs hasn't moved past 0.103 (§1), the fork surface is stable. When iroh-blobs does eventually bump, the fork's ~20-line diff is the only thing that needs re-validation — the `Command` protocol shape is the thing to watch, and the probe confirmed it's macro-generated from a `pub enum Request`, so a breaking change there would be visible in iroh-blobs' changelog.

---

## 9. Recommended execution order

1. **Drop the dead `irpc` dep** (§3, §6) — zero code impact, dissolves the `irpc` half of the gap. Do this first as a standalone commit so the workspace is clean before the iroh migration touches anything.
2. **Bump `alknet-core` iroh to 1.0** (§4, §6) — `Cargo.toml` change + 3 edits in `endpoint.rs`. Run `cargo update -p iroh -p iroh-base -p iroh-relay`.
3. **Verify the build** with `--features iroh` and `--all-features`. Check the `ConnectionError` match arms in `types.rs` (§8.1) for exhaustiveness.
4. **Run the iroh-feature tests** (`endpoint.rs:1107-1150, 1326-1364`) to confirm the accept loop and identity paths still work.
5. **Survey + clean up** `rand` usage (§8.5) and any newly-dead imports.
6. **(Later, separate effort)** Add `alknet-blobs` as a workspace member with the `~20-line` iroh-blobs fork. This is unblocked once step 2 lands, since the workspace is then on a single consistent iroh 1.0.

---

## 10. What this closes and what remains

**Closes:**
- The version-gap open question deferred by `alknet-blobs-external-store-probe.md` §"Versioning reality" and §"What this closes and what remains". The gap is real (iroh 0.35 → 1.0.2, irpc 0.16 → 0.17), the irpc half is a free fix (dead dep), and the iroh half is a traced 3-edit migration with a stable target.
- The probe's false claim that alknet-call uses `irpc` (§3). Corrected: alknet-call declares but never imports `irpc`.
- The "is iroh 0.35 a stale pin" question (§2). Answered: no, it's load-bearing; 3 APIs broke.
- The `Preset` selection question (§5). Answered: `presets::Minimal`, with `tls-aws-lc-rs` feature to match the quinn path's existing crypto provider.

**Does not close (deferred to the migration branch):**
- Actually applying the diff and confirming the build compiles (§7, §8). The edits are traced but not yet made.
- Verifying the `ConnectionError` match exhaustiveness in `types.rs` (§8.1) — needs a compile check.
- Whether `rand` can be dropped from alknet-core's deps after the migration (§8.5).
- The `~20-line` iroh-blobs fork itself (that's the `alknet-blobs` crate effort, separate from this update).

---

## References

- Probe doc: `docs/research/alknet-filesystem/alknet-blobs-external-store-probe.md` — the version-gap open question this doc resolves is at §"Versioning reality" (line 170) and §"What this closes and what remains" (line 206).
- Storage strategy: `docs/research/alknet-storage-strategy/findings.md` — cross-references iroh-blobs as the content-addressed blob layer.
- alknet-core iroh usage: `crates/alknet-core/src/endpoint.rs:676-704` (`build_iroh_endpoint`), `types.rs:231-504` (stream/connection types), `config.rs:28` (`iroh_relay`), `fingerprint.rs:220-228` (parity test).
- alknet-call dead dep: `crates/alknet-call/Cargo.toml:18` (`irpc = { workspace = true }`), `crates/alknet-call/src/protocol/wire.rs` (hand-rolled JSON framing, no irpc).
- Workspace deps: `Cargo.toml:19-20` (`irpc`/`irpc-derive` at 0.16).
- Versions verified via crates.io API: `https://crates.io/api/v1/crates/<name>` (queried 2026-07-08).
- iroh 1.0.2 source: fetched from `https://static.crates.io/crates/iroh/iroh-1.0.2.crate` (not in cargo cache). `Preset` trait at `src/endpoint/presets.rs:21`; `Endpoint::builder(preset)` at `src/endpoint.rs:950`; `Builder::bind()` crypto check at `src/endpoint.rs:225-230`.
- iroh 1.0.0 source: cached at `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/iroh-1.0.0/`. Used for 0.35 vs 1.0.0 pub-surface diff.
- iroh-blobs 0.103.0 source: cached at `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/iroh-blobs-0.103.0/`. `from_sender`/`ref_from_sender`/`ApiClient`/`Scope` visibility confirmed at `src/api.rs:33,299,303` and `src/api/proto.rs:693`. Test preset usage at `src/tests.rs:506,525`.
- iroh-base versions: cached 0.35.0 and 1.0.0; fetched 1.0.2. `SecretKey::generate` signature change at `src/key.rs:273` (0.35) vs `:318` (1.0.2); `from_bytes` error type at `:109` (0.35, `SignatureError`) vs `:122` (1.0.2, `KeyParsingError`).