# OQ-35: ~~API Key Identity vs Peer Identity~~ (Dissolved)

- **Origin**: ADR-030 §"API keys" (the asymmetry between the two auth paths)
- **Status**: **dissolved** (2026-06-27 — the framing was wrong)
- **Door type**: ~~One-way~~
- **Priority**: ~~medium~~
- **Resolution**: **Dissolved.** The original framing ("the fingerprint
  path gets `PeerEntry` id-decoupling, the API-key path doesn't — the
  asymmetry is deliberate") was based on a false distinction between "peer
  bearer" and "auth bearer" tokens. The correct framing is the three
  credential types (Ed25519, X.509, bearer token) and whether the token
  needs a stable logical id across rotation:

  - `PeerEntry` supports multiple credential paths: `fingerprints: Vec<String>`
    (Ed25519 and/or X.509) + `auth_token_hash: Option<String>` (bearer
    token). All resolve to the same `peer_id`.
  - `ApiKeyEntry` is for bearer tokens that ARE the identity (rotation =
    new identity, no stable logical id needed).

  A bearer token that is one credential path among several for a stable
  peer goes in `PeerEntry.auth_token_hash`. A bearer token that IS the
  identity stays in `ApiKeyEntry`. The distinction is whether the token
  needs a stable logical id across rotation, not "peer bearer vs auth
  bearer." See ADR-030 §"Bearer tokens."
- **Cross-references**: ADR-030, [auth.md](crates/core/auth.md),
  [config.md](crates/core/config.md)
