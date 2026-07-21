//! Custom keyword validators for all 17 `TypeDef:*` kinds, registered
//! via `jsonschema::options().with_keyword(...)`.
//!
//! Per ADR-098: the `jsonschema` crate handles all structural validation;
//! the custom keywords only validate leaf type constraints.

// TODO: implement