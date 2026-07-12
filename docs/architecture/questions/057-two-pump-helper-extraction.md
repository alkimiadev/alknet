# OQ-57: Two-Pump Helper Extraction to alknet-core

- **Origin**: `docs/research/alknet-channels/poc-summary.md` §Issues
  Surfaced #7; `docs/architecture/decisions/078-two-pump-shutdown-on-completion.md`
- **Status**: deferred(scope)
- **Door type**: two-way (additive — a helper function does not change any
  API surface; handlers that inline the pattern continue to work)
- **Priority**: low
- **Blocked on**: a second two-pump handler existing, so the shape
  convergence is observable. The tunnel handler is the first two-pump
  consumer; the SSH `direct-tcpip` channel will be the second. Extracting
  the helper from one consumer (the tunnel) would bake in a shape that the
  second consumer (SSH) might not fit — the `fn pump_bidi<R, W>(recv: R,
  send: W, ...) -> impl Future` signature is a cross-crate API surface if
  it lives in `alknet-core`. The trigger is: two real two-pump handlers
  exist and their inline implementations have converged on the same shape.
- **Resolution**: Not yet decidable. The shutdown-on-completion *contract*
  is decided (ADR-078) — a two-pump handler MUST shut down the opposite
  sink when one pump completes, or it deadlocks. The *helper* extraction is
  an implementation convenience: ~10 lines of inline code per handler vs. a
  shared function in `alknet-core`. The contract is pinned; only the
  extraction is deferred. The helper is extracted when two real consumers
  exist and their shapes converge, so the extraction is grounded in two
  implementations rather than guessed from one.
- **What does NOT block on this**: the two-pump pattern is documented
  (ADR-078) and the tunnel handler implements it inline. The SSH crate's
  `direct-tcpip` handler will implement it inline too. Both work without a
  shared helper. The friction is copy-paste with documentation (~10 lines),
  not a missing capability.
- **Cross-references**: ADR-078 (the shutdown-on-completion contract),
  ADR-074 (the `accept_bi` that yields the stream pair the pumps operate
  on), `docs/research/alknet-channels/poc-summary.md` §Issues Surfaced #7