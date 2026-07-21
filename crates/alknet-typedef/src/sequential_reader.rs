//! Packed sequential `SequentialReader` — Mode 1 read-side (ADR-096).
//!
//! Walks a buffer field-by-field according to the schema, reading length
//! prefixes to determine variable-length data positions. Used at read
//! time when the consumer is parsing an incoming frame.

// TODO: implement