# async-nats Reference Documentation

**Crate**: `async-nats` v0.49.1  
**Source**: https://github.com/nats-io/nats.rs (`async-nats/` directory)  
**License**: Apache-2.0

## Contents

| # | File | Topic |
|---|------|-------|
| 01 | [Overview & Architecture](01-overview-and-architecture.md) | Crate overview, feature flags, source structure, core connection model, dependency graph |
| 02 | [Key Types & Traits](02-key-types-and-traits.md) | `Client`, `Subscriber`, `Message`, `Request`, `ServerInfo`, `ConnectInfo`, `Statistics`, subject/header types, event/state types, error types, trait definitions |
| 03 | [Protocol & Wire Format](03-protocol-and-wire-format.md) | NATS wire protocol (PUB/HPUB/SUB/UNSUB/PING/PONG, MSG/HMSG/INFO/ERR), parser/serializer internals, vectored I/O, WebSocket transport, connection lifecycle, reconnection |
| 04 | [Connection Management](04-connection-management.md) | `ConnectOptions` builder, authentication methods, TLS configuration, reconnection callbacks, event callbacks, `ConnectionHandler` internals, multiplexer, server pool management |
| 05 | [JetStream](05-jetstream.md) | `Context` and `ContextBuilder`, streams, consumers (pull/push/ordered), JetStream messages and acks, publish with ack futures, pagination |
| 06 | [Key-Value Store](06-key-value-store.md) | KV `Store` handle, bucket CRUD, put/get/create/update/delete/purge, watch/history/keys, entry operations, mirrored buckets, KV-to-stream mapping |
| 07 | [Object Store](07-object-store.md) | `ObjectStore` handle, put/get/delete/watch/list/seal, links, `Object` (AsyncRead), chunking, SHA-256 integrity, subject naming |
| 08 | [Service API](08-service-api.md) | `Service` and `ServiceBuilder`, endpoints, groups, verb subscriptions (PING/INFO/STATS), request/respond with stats tracking |
| 09 | [Quick Reference](09-quick-reference.md) | Code examples for all major operations, feature flag reference |

## How This Documentation Was Produced

All information was derived by reading the source code of the `async-nats` crate at version 0.49.1 from the `nats.rs` repository. No external documentation was consulted — this is a ground-up reference based purely on the source.