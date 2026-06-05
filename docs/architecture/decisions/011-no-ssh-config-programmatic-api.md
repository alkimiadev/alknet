# ADR-011: Programmatic-First API, No File-Based Config

## Status
Accepted

## Context
The client and server both need configuration (host addresses, keys, transport options, etc.). There are several approaches:

1. **Read `~/.ssh/config`**: Parse OpenSSH config for default host/key/port. Reduces CLI verbosity for frequent connections.
2. **Custom config file**: Alknet-specific config file (TOML/YAML) with host definitions.
3. **Programmatic API only**: Configuration comes from CLI flags or the library API. No file parsing. `~/.ssh/` path conventions are cross-platform trouble (`~` expansion, Windows paths, etc.).
4. **Hybrid**: `--config` flag pointing to a alknet-specific config file, but no OpenSSH config parsing.

## Decision
Option 3: Programmatic-first API. Configuration is provided via:
- **CLI**: explicit flags (`--server`, `--identity`, `--transport`, etc.)
- **Library API**: `alknet_core::client::ConnectOptions` and `alknet_core::server::ServeOptions` structs, constructable programmatically
- **Environment variables**: for a few convenience defaults (e.g., `ALKNET_SERVER`, `ALKNET_IDENTITY`)

No `~/.ssh/config` parsing, no alknet-specific config files. This approach:
- Avoids cross-platform path issues (`~` expansion, Windows `USERPROFILE`, etc.)
- Makes the library API clean and straightforward for programmatic consumers (NAPI wrapper, pubsub)
- Keeps the CLI simple and explicit — no hidden behavior from config files
- Matches the design principle that the library crate (`alknet-core`) is the primary interface

If users want config-file behavior in the future, it can be added as a separate layer that populates the options structs. But the core doesn't need to know about files.

## Consequences
- **Positive**: Clean library API — `ConnectOptions` and `ServeOptions` are plain Rust structs.
- **Positive**: No cross-platform path issues in the core library.
- **Positive**: Explicit CLI — no hidden settings from a config file the user forgot about.
- **Positive**: NAPI wrapper can construct options programmatically without file I/O.
- **Negative**: Users must type full connection flags each time. Mitigated by shell aliases or environment variables.
- **Negative**: No config file convenience. Users coming from `ssh config` may find this inconvenient.

## References
- [client.md](../client.md)
- [OQ-06](../open-questions.md) — resolved by this ADR