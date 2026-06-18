# ADR-013: Rust as Canonical Implementation Language

## Status

Accepted

## Context

alknet's core crates (alknet-core, alknet-call, alknet-vault) and all handler crates are implemented in Rust. A previous TypeScript implementation (`@alkdev/operations`, `@alkdev/pubsub`) informed the design of the call protocol — its operation registry, EventEnvelope framing, adapter patterns (from_openapi, from_mcp, from_call), and bidirectional composition.

The question is: what is the relationship between the TypeScript implementation and the Rust implementation? Is TypeScript a parallel implementation that must be maintained in lockstep, or is Rust the canonical implementation with TypeScript serving a specific role?

Five factors make Rust the canonical choice:

1. **Memory safety eliminates an entire vulnerability class.** Rust's ownership model prevents buffer overflows, use-after-free, and other memory corruption bugs that are endemic in C/C++ and impossible to audit away in JavaScript runtimes.

2. **LLM code generation quality is comparable across Rust and TypeScript.** Agents "grok" both languages roughly equally, so there is no productivity argument for TypeScript.

3. **NPM supply chain attacks are growing rapidly.** The JavaScript ecosystem's dependency density makes supply chain attacks a persistent and increasing risk. NPM is dropping features like post-install scripts in response. This trend makes JavaScript an unreliable foundation for security-critical infrastructure.

4. **Rust is significantly faster.** For networking, encryption, and protocol handling, the performance difference is material — not marginal.

5. **The only legitimate JavaScript use case is the browser.** WASM/WebTransport clients need a JavaScript SDK, and the existing `@alkdev/operations` TypeScript code can be adapted for browser use cases where users want to expose operations to web applications. This is a consumer SDK, not a parallel implementation.

## Decision

**Rust is the canonical implementation language.** All alknet crates are implemented in Rust. The TypeScript `@alkdev/operations` and `@alkdev/pubsub` libraries are reference implementations that informed the design; they are not maintained as parallel implementations.

The relationship between the TypeScript and Rust implementations:

| Aspect | Rust (canonical) | TypeScript (reference/browser) |
|--------|-----------------|-------------------------------|
| OperationSpec, OperationRegistry | alknet-call owns canonical types | `@alkdev/operations` projects canonical types into TS |
| Wire protocol (EventEnvelope) | alknet-call owns canonical framing | `@alkdev/pubsub` implements the same wire format for browser |
| Adapter patterns (from_*, to_*) | alknet-call defines adapter traits and Rust implementations | Browser-adapted implementations where needed |
| Call protocol client | alknet-call (QUIC) | alknet-napi (QUIC via NAPI) or browser SDK (WebTransport) |
| LLM provider integration | alknet-agent (forked aisdk, simplified) | Not applicable |
| Provider key management | alknet-vault via assembly-layer capabilities (no env vars) | Not applicable |

**The adapter contract (from_openapi, from_mcp, from_call, to_openapi, to_mcp) lives in Rust.** These patterns convert external specifications or protocols into `OperationSpec + Handler` pairs that register in the local `OperationRegistry`. The TypeScript implementations serve as reference for browser adaptations, not as the source of truth.

**alknet-napi is a thin projection layer.** It exposes the Rust call protocol client to Node.js via NAPI. It does not contain business logic or adapter implementations. TypeScript consumers who want to use alknet from Node.js use alknet-napi to access the Rust implementation.

**The browser SDK is a future adaptation.** When WASM/WebTransport support is needed, the existing TypeScript code can be adapted to run in browsers, speaking the same EventEnvelope wire format over WebTransport streams. This preserves the WASM door (ADR-009) without requiring Rust-to-WASM compilation of the full stack.

## Consequences

**Positive:**
- Single implementation to maintain, test, and secure
- Memory safety eliminates a whole class of vulnerabilities
- Provider key management through alknet-vault (call protocol) instead of env vars
- No NPM dependency chain for security-critical infrastructure
- The existing TypeScript code informs the Rust design — its patterns are preserved, not its implementation
- Browser clients get a thin, adapted SDK rather than the full operations library

**Negative:**
- Browser support requires a separate JavaScript SDK (adapted from existing TS code) rather than a shared implementation
- Contributors who only know JavaScript cannot contribute to core alknet crates
- The `@alkdev/operations` TypeScript library may drift from the canonical Rust types if not kept in sync during the transition period

**Risks mitigated:**
- WASM door preserved: The `@alkdev/operations` TypeScript code can be adapted for browser use without recompiling Rust to WASM. The wire format is JSON, which any runtime can produce and consume.
- NAPI consumers: alknet-napi provides the call protocol client to Node.js without reimplementing in JavaScript.

## References

- ADR-003: Crate decomposition
- ADR-005: irpc as call protocol foundation
- ADR-009: One-way door decision framework (WASM door)
- Reference TypeScript implementation: `/workspace/@alkdev/operations`
- Reference TypeScript pubsub: `/workspace/@alkdev/pubsub`
- aisdk (Rust port to be forked): `/workspace/aisdk`