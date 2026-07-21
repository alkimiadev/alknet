//! Packed sequential `LayoutBuilder` — Mode 1 write-side (ADR-096).
//!
//! Fields are packed with no alignment padding. Variable-length fields
//! shift all subsequent fields. The consumer provides actual data sizes
//! for variable-length fields; the builder computes byte positions for
//! each field. Used at write time when the consumer knows the data sizes
//! upfront.

// TODO: implement