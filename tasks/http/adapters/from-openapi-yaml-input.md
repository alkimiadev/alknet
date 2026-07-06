---
id: http/adapters/from-openapi-yaml-input
name: Add YAML input format to from_openapi (from_yaml + format-detecting from_str, JSON-first per ADR-051)
status: completed
depends_on: []
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Add YAML OpenAPI document support to `from_openapi`. The spec already
stated the contract as "JSON/YAML doc" but the implementation only ever
delivered the JSON half (`OpenAPISpec::from_json`). YAML was always
intended; vast.ai's `openapi.yaml` surfaces the gap. This is gap-filling
against an existing constraint, not a new direction — no architecture
invariants are touched (`OpenAPISpec` stays `serde_json::Value`-based,
forwarding handler unchanged, no-env-vars unchanged, error fidelity
unchanged, `to_openapi` output stays JSON). See ADR-051 for the full
rationale.

### The three new constructors (ADR-051 §1)

`OpenAPISpec` gains two constructors alongside the existing `from_json`:

```rust
impl OpenAPISpec {
    pub fn from_json(doc: &str) -> Result<Self, AdapterError>;   // existing
    pub fn from_yaml(doc: &str) -> Result<Self, AdapterError>;   // new — parses YAML
    pub fn from_str(doc: &str) -> Result<Self, AdapterError>;    // new — detects format
    pub fn from_value(raw: Value) -> Result<Self, AdapterError>; // existing, unchanged
}
```

`from_yaml` parses YAML to `serde_json::Value` via `yaml_serde`, then
feeds the existing `from_value` path — there is one `OpenAPISpec`, not a
JSON and a YAML variant. `from_str` is the convenience for callers that
have a raw doc string of unknown format (e.g., fetched from a URL with no
Content-Type hint).

### JSON-first format detection — a correctness rule, not style (ADR-051 §2)

`from_str`'s detection rule is **JSON-first, YAML-fallback**: attempt
`serde_json::from_str`; if it parses, use the result; if it fails,
attempt YAML. This ordering is a correctness guard, not a style preference.
YAML 1.1 (the version the maintained Rust YAML crates implement) coerces
the bare tokens `yes`/`no`/`on`/`off`/`y`/`n` to booleans. OpenAPI specs
routinely have **string** fields with these values (e.g., a query
parameter named `active` with value `"yes"`, or an enum of
`["on", "off", "auto"]`). If a JSON document is parsed through the YAML
parser, `{"active": "yes"}` silently becomes `{"active": true}` — the
string is lost, the schema is wrong, and the failure is silent (no error,
just a mutated value the forwarding handler then sends as a boolean
where a string was expected). JSON-first avoids this: a JSON parse
succeeds for any valid JSON doc, and JSON's stricter grammar (`"yes"` is
a string, `true` is a boolean — no ambiguity) avoids YAML 1.1 coercion.
Only if JSON parse fails does `from_str` fall back to YAML. A YAML-only
doc (with `openapi: 3.0.0` at the top, no JSON braces) fails JSON parse
immediately and goes to the YAML path. Net result: JSON docs are never
exposed to YAML's type coercion; YAML docs are parsed as YAML.

`from_yaml` (the explicit constructor) does **not** try JSON first — the
caller has declared the format. If the caller is wrong (passes JSON to
`from_yaml`), the YAML parser handles it (JSON is a syntactic subset of
YAML, so it parses) but with the same silent string→boolean coercion as
the `from_str` footgun above. The caller opted in by naming the format;
`from_str` exists for the unsure caller.

### Dependency: yaml_serde (ADR-051 §3)

The original `serde_yaml` crate (dtolnay) is no longer maintained. The
official YAML organization maintains a continuation published as
`yaml_serde` (crate name `yaml_serde`, v0.10), a drop-in fork with full
API compatibility. `alknet-http` uses the direct form
(`yaml_serde = "0.10"`, `use yaml_serde::`).

Add to `crates/alknet-http/Cargo.toml`:

```toml
yaml_serde = "0.10"
```

The dependency is **not feature-gated**. YAML OpenAPI schemas are a
first-class input format (vast.ai publishes one), not an edge case. Gating
it behind a feature would mean a deployment that imports vast.ai must
remember to enable the feature — the kind of friction the no-surprises
default-features model avoids. The dependency is small (pure Rust, no
native code).

### to_openapi output stays JSON (ADR-051 §4 — scope boundary)

This task is consume-side only. `to_openapi` generates the published
gateway doc, served at `GET /openapi.json`, and stays JSON. YAML publish
output is out of scope; it would be a separate ADR if a concrete consumer
requires it. Do not add a `GET /openapi.yaml` endpoint or a YAML serializer
to `to_openapi`.

### What this task does NOT do

- **No forwarding handler changes.** The handler operates on the parsed
  `OpenAPISpec`, which is format-independent. JSON and YAML both produce
  the same `serde_json::Value`-based internal type.
- **No `to_openapi` changes.** Output stays JSON.
- **No `from_mcp`/`to_mcp` changes.** Unrelated adapters.
- **No provenance / visibility / error-fidelity changes.** All
  unchanged — they operate on the parsed spec, not the wire format.
- **No new ADRs or OQs.** ADR-051 is Accepted; this task implements it.

## Acceptance Criteria

- [ ] `yaml_serde = "0.10"` added to `crates/alknet-http/Cargo.toml`
      `[dependencies]` (not feature-gated)
- [ ] `OpenAPISpec::from_yaml(doc: &str) -> Result<Self, AdapterError>`
      — parses YAML to `serde_json::Value` via `yaml_serde::from_str`,
      then feeds the existing `from_value` path
- [ ] `OpenAPISpec::from_str(doc: &str) -> Result<Self, AdapterError>`
      — tries `serde_json::from_str` first; on failure, falls back to
      `from_yaml`. This is the correctness guard.
- [ ] `from_json` and `from_value` unchanged (existing behavior preserved)
- [ ] New constructors re-exported from `adapters/mod.rs` if needed
      (they're methods on the already-re-exported `OpenAPISpec`, so likely
      no change required — verify)
- [ ] YAML parse failure → `AdapterError::SchemaParse` (same variant as
      JSON parse failure; the error message should name YAML)
- [ ] Unit test: `from_yaml` parses a minimal YAML OpenAPI doc → one
      `HandlerRegistration` (mirror of `import_minimal_doc_yields_one_registration`
      with a YAML fixture)
- [ ] Unit test: `from_yaml` parses a YAML doc with `$ref` resolution
      (mirror of `ref_resolution_in_input_schema` with YAML)
- [ ] Unit test: `from_str` on a JSON doc → succeeds via the JSON path
- [ ] Unit test: `from_str` on a YAML doc (no JSON braces) → succeeds via
      the YAML fallback path
- [ ] Unit test: **the correctness guard** — `from_str` on a JSON doc
      with a string field `"yes"` preserves it as a string (not coerced
      to boolean by the YAML path). E.g.,
      `{"active": "yes"}` → `input_schema` has `"active"` as a string,
      not a boolean. This is the test that proves JSON-first works.
- [ ] Unit test: `from_yaml` on a JSON doc with `"yes"` string → the
      string IS coerced to boolean (documents the opt-in behavior — the
      caller explicitly chose YAML, so YAML's rules apply)
- [ ] Unit test: malformed YAML → `AdapterError::SchemaParse`
- [ ] Unit test: `from_str` on malformed-both (not valid JSON, not valid
      YAML) → `AdapterError::SchemaParse`
- [ ] Existing JSON tests still pass (no regression)
- [ ] `cargo test -p alknet-http` succeeds
- [ ] `cargo clippy -p alknet-http --all-targets` succeeds with no warnings
- [ ] `cargo fmt --check -p alknet-http` passes

## References

- docs/architecture/decisions/051-yaml-input-for-from-openapi.md — ADR-051
  (full rationale: API shape, JSON-first correctness rule, yaml_serde
  choice, to_openapi stays JSON)
- docs/architecture/crates/http/http-adapters.md — §"Type definitions"
  (constructor signatures), §Constraints (the two new bullets), §Design
  Decisions table row
- docs/architecture/crates/http/overview.md — dependency list (yaml_serde
  added), Feature Gates section (yaml_serde not feature-gated)
- crates/alknet-http/src/adapters/from_openapi.rs — existing `from_json`
  and `from_value` to mirror; existing tests to mirror with YAML fixtures
- crates/alknet-http/Cargo.toml — add the `yaml_serde` dependency
- https://github.com/yaml/yaml-serde — `yaml_serde` crate (maintained
  official-YAML-org fork of the deprecated `serde_yaml`)

## Notes

> The non-obvious part is the JSON-first detection rule — it's a
> correctness guard against YAML 1.1 boolean coercion, not a style
> preference. The test that matters most is the one proving a JSON doc
> with `"yes"` string fields survives `from_str` unchanged (JSON path
> wins, no YAML coercion). The complementary test documents that
> `from_yaml` (explicit) *does* coerce — the caller opted in. The
> existing `from_json`/`from_value` constructors and the entire
> forwarding-handler / error-fidelity / no-env-vars path are unchanged —
> this task only adds new parse entry points into the same internal type.
> `to_openapi` output stays JSON (the gap is consume-side, not
> publish-side). No feature gate on `yaml_serde` — YAML OpenAPI is
> first-class.