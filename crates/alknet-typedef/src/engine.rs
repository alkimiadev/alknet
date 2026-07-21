//! `TypedefEngine` — the compiled form of a schema.
//!
//! Combines the layout engine (both packed and aligned modes) and the
//! jsonschema validator into a single struct. Built once at schema load
//! time via [`TypedefEngine::compile`].

// TODO: implement