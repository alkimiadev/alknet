# ADR-010: Transport Chaining in CLI

## Status
Accepted

## Context
Transport chaining allows combining iroh with an upstream proxy, e.g.:

```bash
alknet connect --transport iroh --proxy socks5://127.0.0.1:1080
```

This routes iroh's outbound TCP connections through a SOCKS5 proxy, which could itself be another alknet instance. This is important for:
- Nested tunnel topologies
- Environments where iroh needs to go through an existing proxy
- Composing transports in flexible ways

iroh's `Endpoint::builder` supports proxy configuration natively. The implementation is straightforward — pass the proxy URL to iroh's builder.

## Decision
Support `--transport iroh --proxy socks5://...` natively in the CLI. This works because iroh's endpoint builder accepts a proxy configuration, so the implementation is minimal: parse the proxy URL and pass it to the endpoint builder.

For other transport combinations (TCP+TLS is already implicit — TLS wraps TCP), the `--proxy` flag applies to outbound connections from the SSH client or iroh endpoint.

## Consequences
- **Positive**: Flexible transport composition without requiring separate manual configuration.
- **Positive**: Matches user expectation from the overview doc's transport chaining example.
- **Positive**: Implementation is minimal — iroh already supports proxy config.
- **Negative**: Slightly more CLI surface area (`--proxy` interaction with `--transport`).

## References
- [transport.md](../transport.md)
- [OQ-05](../open-questions.md) — resolved by this ADR