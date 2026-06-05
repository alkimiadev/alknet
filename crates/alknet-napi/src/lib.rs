//! # alknet-napi
//!
//! Node.js native addon for [Alknet](https://git.alk.dev/alkdev/alknet) via napi-rs.
//! Exposes `connect()` and `serve()` functions for programmatic SSH tunnel creation.
//!
//! > **Alpha software.** The NAPI interface may change between versions.
//!
//! # Quick example (Node.js)
//!
//! ```js
//! const { connect, serve } = require('alknet-napi');
//!
//! // Client: open a duplex SSH stream
//! const stream = await connect({
//!   server: "example.com:22",
//!   transport: "tcp",
//!   identity: "/path/to/key",
//! });
//! await stream.write(Buffer.from("hello"));
//! const data = await stream.read(1024);
//! await stream.close();
//! ```

#[allow(unused_imports)]
#[macro_use]
extern crate napi_derive;

mod connect;
mod serve;
