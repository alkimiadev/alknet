# OQ-19: Session-Scoped Operation Registries and Agent-Written Operations

- **Origin**: [operation-registry.md](crates/call/operation-registry.md)
- **Status**: resolved
- **Door type**: Two-way (protocol doesn't need changes), one-way (if implementation closes the door)
- **Priority**: medium
- **Resolution**: The call protocol supports session-scoped registries through `OperationEnv` trait layering. No protocol changes needed. The pattern is documented here and in [operation-registry.md](crates/call/operation-registry.md) to prevent an implementation from accidentally closing it.

  The registry model has three tiers:

  | Tier | Scope | Lifetime | Visibility | Who populates it |
  |------|-------|----------|------------|-------------------|
  | Core (global) | All sessions | Process lifetime, static at startup | External + Internal (curated) | Assembly layer at startup |
  | Session | One session | Session lifetime, dynamic | Internal only (never wire-facing) | Agent during session (sandbox) |
  | Promotion | Session → Core | One-time transition | Manual/curated review | Human or architect agent reviews, then redeploys |

  Session-scoped operations are always `Internal` (ADR-015), run under the handler's identity (the agent handler that authorized the sandbox), can only compose operations in the handler's scoped env, and are ephemeral (gone when the session ends). Core operations are curated — reviewed before promotion. The promotion path is the curation checkpoint where autonomous (session-scoped) becomes curated (core). This is not auto-promotion.

  **Implementation guard**: `OperationEnv` must remain a trait, not a concrete type. A session-scoped env wraps the global env (check session registry first, fall through to global). Making `OperationEnv` concrete or hardcoding the global registry into the dispatch path would close this pattern. The static registration constraint (OQ-04) applies to the curated (Layer 0) registry only; session registries are dynamic by nature and are a different registry overlaying the curated one. **Generalized by ADR-024**: connection-scoped remote imports (`from_call`) use the same overlay mechanism as session-scoped ops. Both are per-scope dynamic overlays on the static curated base, composed into the per-call `OperationContext.env` by the `CallAdapter`. `OperationEnv` being a trait object (`Arc<dyn OperationEnv + Send + Sync>`) is what enables both overlay patterns.

  Session-scoped operations run in a locked-down sandbox (no direct net/fs/env access), can only reach operations in the handler's scoped env, and their output should be validated against their declared schema before returning. The promotion path requires review — an agent with a `promote` scope (the architect role) performs the promotion; the writing agent (lower-privileged role) requests it. This is the role-based escalation pattern (ADR-015): privileges escalate through a chain of command, not through direct authority.

  The agent-specific mechanism (quickjs sandbox, session registry lifecycle, promotion workflow) belongs to the agent crate spec. The call protocol's job is to keep the `OperationEnv` trait composable and the visibility/ACL model consistent across tiers.
- **Cross-references**: OQ-04, ADR-014, ADR-015, ADR-016, ADR-024, [operation-registry.md](crates/call/operation-registry.md)
