# ADR-051: YAML Input Format for from_openapi

## Status

Accepted

## Context

`from_openapi` imports external HTTP APIs as call-protocol operations by
parsing an OpenAPI document. The `http-adapters.md` spec already states the
one-way constraint as "`from_openapi` accepts a standard OpenAPI 3.x
JSON/YAML doc" — YAML was always part of the intended input contract. The
implementation, however, only ever delivered the JSON half:
`OpenAPISpec::from_json(&str)`. This was fine for providers that publish JSON
OpenAPI schemas (e.g., runpod's `openapi.json`) but blocks providers that
publish YAML schemas (e.g., vast.ai's `openapi.yaml`). A coming consumer crate
needs to import vast.ai's operations, which surfaces the gap.

This is gap-filling against an existing constraint, not a new architectural
direction. None of the architecture invariants are touched: `OpenAPISpec`
stays `serde_json::Value`-based, the forwarding handler is unchanged, the
no-env-vars credential injection is unchanged, error fidelity is unchanged,
`to_openapi` output stays JSON. The change is a new parse path into the
same internal type.

Two decisions need recording: the parse strategy (JSON-first, not
parse-everything-as-YAML) and the dependency choice (the maintained
`yaml_serde` fork, not the deprecated `serde_yaml`).

## Decision

### 1. `from_openapi` accepts YAML via a `from_yaml` constructor and a format-detecting `from_str`

`OpenAPISpec` gains two constructors alongside the existing `from_json`:

```rust
impl OpenAPISpec {
    pub fn from_json(doc: &str) -> Result<Self, AdapterError>;   // existing
    pub fn from_yaml(doc: &str) -> Result<Self, AdapterError>;   // new — parses YAML
    pub fn from_str(doc: &str) -> Result<Self, AdapterError>;    // new — detects format
    pub fn from_value(raw: Value) -> Result<Self, AdapterError>; // existing, unchanged
}
```

`from_str` is the convenience for callers that have a raw doc string of
unknown format (e.g., fetched from a URL with no Content-Type hint). The
detection rule is **JSON-first, YAML-fallback** (see §2 for why the order
matters): attempt `serde_json::from_str`; if it parses, use the result; if
it fails, attempt YAML. `from_json` and `from_yaml` remain for callers that
know the format and want a precise error on mismatch.

This is an additive API surface change (two-way door — constructors can be
renamed/added; nothing downstream breaks). The constructors produce the same
`OpenAPISpec`; the rest of the adapter is format-agnostic.

### 2. Format detection is JSON-first, YAML-fallback — a defensive default, not a style preference

> **Amendment (2026-07-06):** The original §2 cited YAML 1.1
> boolean-coercion (`yes`/`no`/`on`/`off` → booleans) as a *present
> hazard* with the maintained Rust YAML crates, framing JSON-first as a
> correctness guard against silent string→boolean mutation. A probe
> during implementation verified this is factually wrong for the chosen
> dependency: `yaml_serde` 0.10.x (and the deprecated `serde_yaml` 0.9)
> implement the **YAML 1.2 core schema**, where only `true`/`false` (and
> case variants) are booleans — the bare tokens `yes`/`no`/`on`/`off`/
> `y`/`n` are plain strings. The coercion hazard the original rationale
> cited does not exist with this dependency version. The JSON-first rule
> is **retained** (Accepted ADR) — the rationale is reframed below as a
> defensive default, not a guard against a present hazard. The decision
> did not change; the rationale did.

JSON's grammar is a strict subset of YAML (under YAML 1.2) and never
exhibits any YAML-specific type interpretation. Running a JSON document
through a YAML parser is *currently* safe with `yaml_serde` 0.10.x — a
JSON doc like `{"active": "yes"}` parses through the YAML path with
`"yes"` intact as a string (YAML 1.2 core schema, verified by the
`from_yaml_preserves_bare_yes_as_string_yaml_1_2_behavior` test).
JSON-first detection is therefore not guarding against a present hazard
with this dependency; it is a **defensive default that locks in the
contract against a future YAML-parser swap**. If `yaml_serde` is ever
swapped for a YAML 1.1 crate (where `yes`/`no`/`on`/`off` coerce to
booleans), or if a future `yaml_serde` version tightens its core schema
in a way that introduces type interpretation JSON doesn't have, the
JSON-first rule ensures JSON input cannot be silently mutated by the
YAML path. The contract is durable; the dependency is a two-way door
(§3).

The rule is cheap: `from_str` tries `serde_json::from_str` first (strict
grammar, no YAML-specific interpretation), and only on JSON parse failure
falls back to the YAML parser. A YAML-only document (with `openapi: 3.0.0`
at the top, no JSON braces) fails JSON parse immediately and goes to the
YAML path. The cost is one wasted parse attempt for YAML docs, paid once
at adapter-import time (not per forwarded call — see Consequences).

`from_yaml` (the explicit constructor) does not try JSON first — the caller
has declared the format. This is correct: a caller that explicitly says
"this is YAML" wants the YAML parse, including whatever type
interpretation the YAML parser applies. If the caller is wrong (passes
JSON to `from_yaml`), the YAML parser handles it — JSON is a syntactic
subset of YAML, so it parses, with whatever interpretation the YAML
parser's schema applies (currently none for `yes`/`no` under YAML 1.2; a
future YAML 1.1 swap would coerce). The caller opted in by naming the
format; `from_str` exists for the unsure caller.

### 3. The YAML dependency is `yaml_serde` (the official YAML org fork of `serde_yaml`), not the deprecated `serde_yaml`

The original `serde_yaml` crate (dtolnay) is no longer maintained. The
official [YAML organization](https://github.com/yaml) maintains a
continuation published as `yaml_serde` (crate name `yaml_serde`, v0.10), a
drop-in fork with full API compatibility. The migration path is either
`serde_yaml = { package = "yaml_serde", version = "0.10" }` (keeps
`use serde_yaml::` imports) or direct `yaml_serde = "0.10"` with updated
imports. `alknet-http` uses the direct form (`yaml_serde = "0.10"`,
`use yaml_serde::`).

The dependency is a two-way door: `yaml_serde` can be swapped for another
maintained YAML-serde fork (or a future replacement) by changing the
Cargo line and the imports. The one-way constraint is that `alknet-http`
owns its YAML parse and produces `serde_json::Value` (the shared internal
type) — which dependency does the parse is an implementation detail.
`yaml_serde` is chosen because it is the maintained continuation under
the official YAML umbrella, not because its API is irreversibly
load-bearing.

The dependency is **not feature-gated**. YAML OpenAPI schemas are a
first-class input format (vast.ai publishes one), not an edge case. Gating
it behind a feature would mean a deployment that imports vast.ai must
remember to enable the feature — the kind of friction the no-surprises
default-features model avoids. The dependency is small (a pure-Rust YAML
parser, no native code), consistent with the existing default-features
philosophy of the crate.

### 4. Scope boundary: `to_openapi` output is not affected

`to_openapi` generates the published gateway doc, served at `GET
/openapi.json`. It stays JSON. This ADR fills a gap on the *consume* side
(importing external YAML schemas); the *publish* side serves our own
gateway contract and JSON is the standard exchange format for OpenAPI
tooling (code generators, validators, `fetch`-based clients all consume
JSON). A `GET /openapi.yaml` additive output is not part of this decision:
it is a separate scope (publish-side format, not consume-side), would be a
separate ADR if a concrete consumer requires YAML output, and is
additive (a new endpoint, no breaking change to the JSON path). The
`OpenAPISpec` type is shared, but the output serialization is JSON-only.

## Consequences

**Positive:**
- `from_openapi` consumes both JSON and YAML OpenAPI schemas — the
  intended contract (spec line: "JSON/YAML doc") is finally delivered. vast.ai
  and any other YAML-publishing provider can be imported.
- Format detection (`from_str`) makes fetch-and-import ergonomic: a caller
  that fetched a schema from a URL with no reliable Content-Type doesn't
  have to sniff the format itself.
- JSON-first detection is a defensive default that locks in the
  contract against a future YAML-parser swap. With `yaml_serde` 0.10.x
  (YAML 1.2 core schema) the coercion hazard the original rationale
  cited is not present; JSON-first nonetheless ensures JSON input is
  never exposed to YAML-specific type interpretation, present or
  future. The rule is cheap (one wasted parse for YAML docs, paid once
  at import time) and the contract is durable.
- The maintained `yaml_serde` fork keeps the dependency off the archived
  `serde_yaml`; the swap is documented so a future maintainer doesn't
  re-derive why the crate name doesn't match the obvious name.

**Negative:**
- A new pure-Rust dependency (`yaml_serde`) in `alknet-http`. Small, but
  non-zero. The trade is first-class YAML support without a feature gate —
  accepted because YAML OpenAPI is a real input format, not an edge case.
- `from_str`'s JSON-first detection does one wasted parse attempt for YAML
  docs (the JSON parse fails, then the YAML parse runs). The cost is
  trivial — `from_openapi` runs once at adapter-import time (not per
  forwarded call), so the double-parse happens once per imported service,
  not per request. Callers that know the format use `from_json`/`from_yaml`
  directly and pay no double-parse. The defensive benefit (JSON input never
  reaches the YAML parser, immune to any YAML-specific interpretation
  present or future) is worth the one-time cost.

## Assumptions

1. **The `OpenAPISpec` internal type stays `serde_json::Value`-based.** YAML
   parses to `serde_json::Value` via `yaml_serde`, then feeds the existing
   `from_value` path. No second internal representation. If a future
   switch to `openapiv3::OpenApi` happens (the two-way-door the spec already
   notes), both JSON and YAML constructors adapt in lockstep — the
   constructor is the adapter between wire format and internal type.

2. **`yaml_serde` 0.10.x implements the YAML 1.2 core schema.** Verified
   by a probe during implementation: bare `yes`/`no`/`on`/`off`/`y`/`n`
   are plain strings, not booleans (codified by the
   `from_yaml_preserves_bare_yes_as_string_yaml_1_2_behavior` test). The
   original §2 rationale cited YAML 1.1 coercion as a present hazard; it
   is not, with this dependency version. JSON-first detection is retained
   as a defensive default (§2 as amended): a future swap to a YAML 1.1
   crate, or a future `yaml_serde` schema tightening, cannot silently
   regress JSON input because JSON never reaches the YAML path under
   `from_str`. If the dependency swaps to a YAML 1.1 crate, the defensive
   default becomes a load-bearing correctness guard — the rule is the
   same either way, which is why it is stated as a contract rather than
   as a workaround for a specific crate version.

## References

- [ADR-017](017-call-protocol-client-and-adapter-contract.md) —
  `from_openapi` is an `OperationAdapter`; published `to_*` specs are
  compatibility contracts (the publish side stays JSON)
- [ADR-023](023-operation-error-schemas.md) — error fidelity is unaffected
  (error schemas come from the parsed `OpenAPISpec`, format-independent)
- [ADR-039](039-http-server-and-client-host-colocated.md) — `alknet-http`
  owns both HTTP directions and their dependencies
- [http-adapters.md](../crates/http/http-adapters.md) — the spec that
  states the "JSON/YAML doc" constraint, the `OpenAPISpec` type, and the
  Constraints/Design Decisions entries this ADR backs (see the
  "Input formats" doc-comment, the Constraints §"`from_openapi` accepts
  JSON and YAML", and the Design Decisions table row)
- `yaml_serde` crate (https://github.com/yaml/yaml-serde) — the maintained
  official-YAML-org fork of the deprecated `serde_yaml`