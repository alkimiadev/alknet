# OQ-60: Where Does Transport Construction Live?

- **Origin**: `docs/architecture/decisions/083-endpoint-as-accept-loop-runner.md`
  (the endpoint refactor commits to the boundary — construction is not
  in the endpoint — but not the location of `build_iroh_endpoint`).
- **Status**: open
- **Door type**: one-way (where `build_iroh_endpoint` lives determines
  who depends on `iroh` for transport construction; moving it later
  churns the dep graph and every binary's assembly code)
- **Priority**: high (the hub is the first multi-transport consumer;
  its assembly code sets the pattern)
- **Blocked on**: nothing structural. The three options are clear; the
  decision is a trade-off, not a missing capability.
- **Scope**: This OQ covers **`build_iroh_endpoint`** — the function
  that reads `StaticConfig` and builds an `iroh::Endpoint`.
  `build_quinn_server_config_from_rustls` is **decided** (it moves to
  `alknet-tls` as `TlsServerConfig::for_quinn()` per ADR-082 — it's a
  thin wrapper over a `rustls::ServerConfig`, which `alknet-tls` owns).
  Only `build_iroh_endpoint` is genuinely undecided: it reads
  `StaticConfig` (not a rustls config), builds an `iroh::Endpoint` from
  an `Ed25519SecretKey` + relay URL, and doesn't fit the `alknet-tls`
  cert-provider boundary.
- **Resolution**: Not yet decided. The options:

  **Option A: In the assembly layer (the binary).** Each binary reads
  `StaticConfig` and hand-assembles quinn/iroh/TCP+TLS. Pro: maximal
  flexibility, core stays lean. Con: every binary duplicates the
  "build an iroh endpoint from an `Ed25519SecretKey` + relay URL"
  boilerplate; a 10-step procedure is copy-pasted per binary.

  **Option B: In `alknet-tls` as convenience helpers.** `alknet-tls`
  gains a `build_iroh_endpoint` helper. Pro: one place. Con: `alknet-tls`
  then depends on `iroh`, bloats a crate whose stated job is "TLS setup,
  not transport endpoint construction" (ADR-082 scopes `alknet-tls` to
  cert config and its accessors — `build_iroh_endpoint` doesn't touch
  certs, it touches iroh's relay/secret-key APIs).

  **Option C: A new crate or module — `alknet-transport` / an
  `alknet-core::transport` module — that owns transport construction.**
  Pro: clean separation (TLS = certs, transport = endpoints, endpoint =
  dispatch). Con: another layer in the dep graph.

  The ADR-082 boundary ("`alknet-tls` is the cert provider, not the
  transport constructor") is a strong argument against Option B. Option C
  is the cleanest separation but adds a layer. Option A is simplest but
  risks per-binary duplication that drifts.
- **Cross-references**: ADR-083 (endpoint refactor), ADR-082
  (`alknet-tls` — the cert provider boundary; `for_quinn()` is in scope,
  `build_iroh_endpoint` is not), ADR-010 (original endpoint design,
  where construction was welded to the endpoint)