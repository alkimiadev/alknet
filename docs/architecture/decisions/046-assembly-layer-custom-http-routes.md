# ADR-046: Assembly-Layer Custom HTTP Routes on HttpAdapter

## Status

Proposed

## Context

The `HttpAdapter` (`crates/http/http-server.md`) is constructed by the
assembly layer with an `Arc<dyn IdentityProvider>`, an
`Arc<OperationRegistry>`, and a `DecoyConfig`. The axum `Router` it
builds has a fixed surface:

- The `to_openapi` gateway endpoints (`/search`, `/schema`, `/call`,
  `/batch`, `/subscribe` — ADR-042) — the sole invoke path over HTTP
  (ADR-047 removed the direct-call `POST /{service}/{op}` surface that
  ADR-036 originally defined).
- `/healthz`, `/openapi.json`, the MCP route (feature-gated), and the
  decoy fallback for unknown paths.

There is no documented extension point for a downstream deployment to
add its own HTTP routes to this router. A deployment that wants to
expose a custom HTTP endpoint — one that is *not* a gateway endpoint —
has no specified way to do so. The architecture currently ties the HTTP
surface to the simplified contract with no escape hatch.

### The concrete use case

A hub deployment (e.g., `api.alk.dev`) wants to expose the standard
alknet contract (direct-call + gateway) **and** an OpenAI-compatible
proxy at `/v1/chat/completions`. The OAI proxy is a custom HTTP route:
it receives an OpenAI-shaped request, dispatches into the
`OperationRegistry` (likely to a `from_openapi`-imported `openai/chat`
operation or a custom agent operation), and returns an OpenAI-shaped
response. It is not an alknet operation — it is a deployment-specific
HTTP endpoint that uses the registry as a backend.

This pattern is not exotic. It is the standard "wrap an external API
shape around our operations" pattern: a deployment adds a
compatibility shim (OAI-compatible, Anthropic-compatible, a legacy API
shape) as a custom route, backed by call-protocol operations. The
alternative — forcing every custom endpoint to be a call-protocol
operation whose input/output match the external API's shape — is
brittle (the OAI streaming response shape is not a clean call-protocol
output) and unnecessary (the deployment owns the HTTP shape; the
registry owns the operation shape).

The runner pattern that motivates this (remote GPU instance downloads
a binary, connects back to the hub via `from_call`, registers its ops;
opencode connects to the hub as a standard OAI provider) is already
supported by the existing architecture (`from_call`, `PeerRef` routing,
`from_openapi` to wrap the OAI API). The only missing piece is the
custom HTTP route on the hub.

### Why this needs an ADR

The extension mechanism — how the assembly layer injects custom routes
— is a published API surface of `HttpAdapter`. Once downstream
deployments build against it (passing their custom routers at
construction), changing the mechanism is a one-way door (every consumer
construction site breaks). It needs an ADR before implementation so the
contract is deliberate, not accidental.

The specific routes a deployment adds are a two-way door (add/remove
freely, no protocol contract). The *mechanism* (the constructor
parameter and its semantics) is the one-way door this ADR commits.

## Decision

### 1. HttpAdapter accepts additional axum routes from the assembly layer

The `HttpAdapter` constructor gains a parameter for deployment-specific
routes. The assembly layer builds an `axum::Router` with its custom
routes and passes it in; `HttpAdapter` composes it with the default
surface. A deployment that passes no custom routes gets exactly the
documented default behavior — the extension point is additive, not
mandatory.

```rust
pub struct HttpAdapter {
    identity_provider: Arc<dyn IdentityProvider>,
    registry: Arc<OperationRegistry>,
    decoy: DecoyConfig,
    /// Deployment-specific routes added by the assembly layer. None =
    /// the default surface only. See ADR-046.
    extra_routes: Option<Router>,
}
```

The exact composition mechanism (merge vs nest vs builder, whether
custom routes get a prefix) is a two-way-door implementation detail;
the one-way constraint is that the assembly layer can inject routes
and they coexist with the default surface. axum's `Router::merge` /
`Router::nest` are the natural primitives.

### 2. Custom routes are raw HTTP, not call-protocol operations

A custom route is a raw axum handler — it receives an HTTP request and
returns an HTTP response. It is not registered in the
`OperationRegistry`, not discoverable via `/search`, not described in
the `to_openapi` gateway doc. The deployment owns its shape entirely.

A custom route *may* dispatch into the `OperationRegistry` (via
`OperationRegistry::invoke()`, same as the gateway's `/call` endpoint
does) if
it wants to back the HTTP endpoint with a call-protocol operation. The
OAI-compatible proxy does this: the `/v1/chat/completions` handler
parses the OAI request, invokes the `openai/chat` (or `agent/chat`)
operation, and reformats the response as an OAI response. But this is
the custom route's choice — it could equally be a pure HTTP handler
that never touches the registry (a webhook receiver, a static asset
server, a legacy API shim with its own backend).

### 3. The default surface's reserved paths take precedence on collision

The default-surface paths are reserved: `/search`, `/schema`, `/call`,
`/batch`, `/subscribe`, `/healthz`, `/openapi.json`, and the MCP route.
(ADR-047 removed the direct-call `/{service}/{op}` surface, so it is no
longer a reserved path; a deployment that builds a per-operation
projection as a custom route is the one case where `/{service}/{op}`
patterns appear, and those custom routes are subject to the same
collision rule.) If a custom route collides with a reserved path, the
default surface wins — the custom route is silently shadowed (or the
construction panics/warns; the specific collision-handling is a
two-way-door implementation detail). A deployment that wants
`/v1/chat/completions` namespaces it away from the reserved set, which
is natural (`/v1/...` doesn't collide).

### 4. Custom routes carry the same auth middleware by default; per-route opt-out is the deployment's choice

Custom routes run under the same Bearer-auth resolution as the default
surface (the `Authorization: Bearer` → `resolve_from_token` path). A
deployment that wants a custom route to be unauthenticated (a public
webhook receiver, a health endpoint with a different shape than
`/healthz`) applies axum middleware to opt that route out of auth —
the deployment owns its custom routes' middleware stack. The
`HttpAdapter` provides the identity provider and the default auth
middleware; the custom `Router` the assembly layer passes in can
layer its own middleware on top. This is standard axum composition; no
alknet-specific mechanism.

### 5. Custom routes are not part of the published `to_openapi` doc

The `to_openapi` gateway doc (ADR-042, ADR-045) describes the 5
gateway endpoints — the default contract. Custom routes are
deployment-specific and not described by `to_openapi`. A deployment
that wants its custom routes documented for external consumers
generates its own OpenAPI doc for them (a separate projection, not
`to_openapi`). The default `info.version` semver (ADR-045) tracks the
gateway contract, not custom routes — custom routes have no
versioning contract with alknet; the deployment versions them however
it wants.

### 6. This does not change the default surface

A deployment that constructs `HttpAdapter` with no extra routes gets
exactly the behavior documented in `http-server.md` — gateway,
`/healthz`, `/openapi.json`, MCP (feature-gated), decoy. The
extension point is purely additive. The default surface remains the
published contract (ADR-042, ADR-045; ADR-036's routing decision is
superseded by ADR-047); custom routes are a
deployment-specific addition on top, not a modification of it.

## Consequences

**Positive:**
- Deployments can wrap external API shapes (OAI-compatible,
  Anthropic-compatible, legacy) around call-protocol operations without
  forcing the external shape into the operation's input/output schema.
  The "compatibility shim" pattern is first-class.
- The runner pattern (remote worker → `from_call` → hub → custom OAI
  route → opencode) works end-to-end with no architectural gap. The hub
  is a standard alknet node *plus* a deployment-specific HTTP surface.
- The extension point is standard axum composition — no alknet-specific
  routing abstraction for deployers to learn. A developer who knows
  axum can add routes.
- The default surface is unchanged for deployments that don't need
  custom routes. No complexity tax for the common case.

**Negative:**
- The HTTP surface is no longer fully described by the alknet specs
  alone — a deployment's custom routes are outside the architecture
  docs. This is inherent to the extension point (the deployment owns
  them); the specs describe the *default* surface and the *mechanism*,
  not every possible custom route.
- A custom route that dispatches into the registry bypasses the
  gateway's `AccessControl`-filtered `/search` discovery — the custom
  route is responsible for its own authorization story. The default
  Bearer-auth middleware covers the common case, but a custom route
  that wants per-operation ACL checks must call
  `OperationRegistry::invoke()` with a proper `OperationContext`
  (caller identity from the resolved bearer token), not a bypass. The
  `invoke()` path enforces `AccessControl` regardless of the entry
  point (direct-call, gateway, or custom route), so this is not an
  ACL bypass — but the custom route author must construct the context
  correctly.
- Two deployments with custom routes have different HTTP surfaces —
  there is no single "what does an alknet HTTP endpoint look like"
  answer anymore. The default surface is the contract; custom routes
  are deployment-specific variance. This is honest (deployments
  *do* vary) but means the architecture docs describe the default, not
  the union.

## Assumptions

1. **The assembly layer is the composition point.** Custom routes are
   added at `HttpAdapter` construction, not registered dynamically at
   runtime. This matches the static-registration constraint (OQ-04 /
   ADR-010) for the `HandlerRegistry`; the `HttpAdapter`'s router is
   likewise immutable after construction. Dynamic route addition would
   require `ArcSwap<Router>` and is not part of this ADR.

2. **Custom routes are a deployment concern, not an alknet-crate
   concern.** `alknet-http` provides the extension point (accepts the
   extra `Router`); it does not provide custom route implementations.
   The OAI-compatible proxy, the legacy API shim, the webhook receiver
   are all written by the deployment (or a downstream crate like
   `alknet-agent` that builds on `alknet-http`), not by `alknet-http`
   itself.

3. **The default surface is the published contract; custom routes are
   not.** ADR-036 (direct-call), ADR-042 (gateway), ADR-045 (versioning)
   govern the default surface. Custom routes have no alknet-governed
   compatibility contract — the deployment owns their stability. This
   keeps the published-contract surface small and stable while allowing
   arbitrary deployment-specific extension.

4. **axum's composition primitives are sufficient.** `Router::merge`,
   `Router::nest`, and axum middleware cover the extension patterns
   needed (custom routes, per-route auth opt-out, prefix namespacing).
   No alknet-specific routing abstraction is required. If a future
   need exceeds axum's composition (e.g., route-level dynamic dispatch),
   that would be a separate ADR.

## References

- [ADR-010](010-alpn-router-and-endpoint.md) — static registration at
  startup (the `HttpAdapter` router is immutable after construction,
  same constraint)
- [ADR-042](042-openapi-gateway-pattern.md) — the gateway endpoints
  (the default surface custom routes coexist with; reserved paths)
- [ADR-045](045-to-openapi-gateway-spec-versioning.md) — the published
  doc versions the gateway contract, not custom routes
- [ADR-047](047-remove-direct-call-http-surface.md) — the direct-call
  surface is removed; the gateway is the sole invoke path (a
  deployment that wants the former per-operation HTTP surface builds it
  as a custom route projection; this ADR §4 is the mechanism)
- `crates/http/http-server.md` — the `HttpAdapter` spec that gains the
  `extra_routes` constructor parameter