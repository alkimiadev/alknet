//! Error types for the typedef engine.
//!
//! Decided in ADR-098: a single `TypedefError` enum covers all error
//! conditions across the engine's three phases (schema parsing, offset
//! computation, read/write) plus validation.

// TODO: implement