# OQ-68: Channels Add/Strip API Shape (Built-In vs. Utility)

- **Origin**: `docs/research/stream-unification/findings.md` §"The
  add/strip utility"; `docs/architecture/decisions/093-channels-pure-channel-multiplexing.md`
  §"What this ADR does NOT decide"; `docs/architecture/crates/channels/channels-wire.md`
  §"The add/strip composition"
- **Status**: open
- **Door type**: two-way (the *contract* — channels strips its 8-byte
  header on read, handler parses its own framing from the payload — is
  decided in ADR-093; the *function surface* — whether add/strip is
  built into the read/write path or exposed as a standalone utility —
  is reversible without breaking the wire format or the handler contract)
- **Priority**: low
- **Impacts**: None — the add/strip *contract* is decided (ADR-093);
  this OQ is about the API shape, not the contract. The channels crate
  can ship with either shape and switch later without a wire-format
  change.
- **Investigation target**: work through 2+ example handler
  compositions (TTY inside channels, tunnel inside channels, a
  recursive `alknet/channels`-inside-`alknet/channels` composition) to
  see where the add/strip naturally lives. If the header add/strip is
  built into the channels read/write path, the handler never sees the
  `channel_id` — the `BiStream` the handler receives is the payload
  bytes. If the add/strip is a standalone utility, the handler (or a
  test helper, or the hub relay's `channel_id` rewrite) can call it
  explicitly. The question is which shape is cleaner for the common
  case (handler inside channels) without foreclosing the
  less-common cases (recursive composition, the hub relay, test
  helpers).
- **Resolution**: Not yet decided. The two options:

  **Option A — Built into read/write (the default).** The channels
  layer's `accept_bi` returns a `BiStream` whose bytes are the payload
  (the channels header is stripped internally). The handler's
  `AsyncWrite` on the `BiStream` re-adds the 8-byte header internally
  (the handler writes payload bytes; the channels layer frames them).
  The handler never sees the `channel_id`; the add/strip is invisible.
  This is the cleanest shape for the common case (a handler inside
  channels). The utility (`add_channel_id` / `strip_channel_id`) is
  still available internally (the channels layer calls it), and may
  be exposed publicly for the less-common cases (recursive composition,
  the hub relay, test helpers) — but the handler boundary doesn't
  require it.

  **Option B — Standalone utility (the explicit alternative).** The
  channels layer exposes `add_channel_id(channel_id, payload_bytes)
  -> chunk` and `strip_channel_id(chunk) -> (channel_id,
  payload_bytes)` as the primary API. The handler (or a wrapper, or a
  test helper) calls them explicitly. This is the shape the
  stream-unification research proposed. It's more explicit (the
  handler sees the `channel_id`, can log it, can route on it) but
  pushes the add/strip to the handler boundary, not the channels
  read/write path. The handler's `BiStream` is the raw chunk bytes
  (header + payload), not the payload alone.

  The trade-off: Option A is cleaner for the common case (the handler
  doesn't care about `channel_id`; the channels layer owns it
  entirely) but may require an escape hatch for the less-common cases
  (the hub relay needs to rewrite `channel_id`; recursive composition
  needs to re-add a header; test helpers may want to construct chunks
  directly). Option B is more uniform (the same add/strip pair at every
  level, including the handler boundary) but pushes work to the
  handler that the channels layer could own. The investigation target
  (2+ example compositions) is what surfaces which shape is cleaner in
  practice.

  This question is decision-ready when the channels crate's
  implementation begins. Until then, the *contract* (channels strips,
  handler parses payload) is decided (ADR-093); the *function surface*
  is not.

- **Cross-references**: ADR-093 (the umbrella decision that decides the
  add/strip *contract* and leaves the *function surface* to this OQ);
  `docs/research/stream-unification/findings.md` §"The add/strip
  utility" (the research that proposed the utility);
  `docs/architecture/crates/channels/channels-wire.md` §"The add/strip
  composition" (the spec that records the contract and points to this
  OQ)