---
id: client/port-forwarding
name: Implement port forwarding — local (-L) and remote (-R) forwards
status: completed
depends_on:
  - auth/client-auth-handler
  - transport/trait-and-types
  - auth/error-types
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement SSH port forwarding per client.md:

**Local port forwards (`-L local_addr:local_port:remote_host:remote_port`)**:
1. Bind `TcpListener` on `local_addr:local_port`
2. For each accepted connection, open `channel_open_direct_tcpip(remote_host, remote_port, ...)`
3. Proxy bytes bidirectionally via `copy_bidirectional`

**Remote port forwards (`-R remote_addr:remote_port:local_host:local_port`)**:
1. Send `tcpip_forward(remote_addr, remote_port)` to request the server listen on a port
2. When the handler receives `server_channel_open_forwarded_tcpip`, connect to `local_host:local_port`
3. Proxy bytes bidirectionally

Both types are specified as repeatable `--forward` / `--remote-forward` CLI options.

## Acceptance Criteria

- [ ] `crates/alknet-core/src/client/forward.rs` exports `PortForwardSpec`, `LocalForwarder`, `RemoteForwarder`
- [ ] `PortForwardSpec` parses `-L` / `-R` spec strings: `local_addr:local_port:remote_host:remote_port`
- [ ] `LocalForwarder` binds TcpListener, accepts connections, opens SSH direct-tcpip channel for each, proxies bidirectionally
- [ ] `RemoteForwarder` sends `tcpip_forward` request, handles `forwarded-tcpip` channel opens, connects to local target, proxies bidirectionally
- [ ] Both forwarders handle their accept loops concurrently with the SOCKS5 server
- [ ] Connection errors close the individual channel without affecting other forwards or the SSH session
- [ ] Port forward listeners are re-registered after SSH reconnection (depends on channel-manager)
- [ ] Unit tests: spec parsing, local forward proxy, remote forward request handling

## References

- docs/architecture/client.md — Port Forwarding section
- docs/architecture/decisions/005-socks5-before-tun.md — port forwarding as optional complement to SOCKS5

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion