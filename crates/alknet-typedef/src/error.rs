//! Error types for the typedef engine.
//!
//! Decided in ADR-098: a single `TypedefError` enum covers all error
//! conditions across the engine's three phases (schema parsing, offset
//! computation, read/write) plus validation.

use std::fmt;

/// Errors produced by the typedef engine across all phases.
#[derive(Debug)]
pub enum TypedefError {
    /// Schema parsing errors — invalid JSON, missing required keywords,
    /// unknown `TypeDef:*` kinds, malformed annotations.
    Schema(String),

    /// Offset computation errors — field not found, type not supported
    /// for offset computation, recursive depth exceeded.
    Offset { field_path: String, reason: String },

    /// Read/write errors — buffer too short, invalid UTF-8, value out
    /// of range for the target type.
    Access { field_path: String, reason: String },

    /// Validation errors — delegated to the `jsonschema` crate.
    /// The `'static` lifetime is correct: the validator owns its schema
    /// reference and lives for the lifetime of the `TypedefEngine`.
    Validation(jsonschema::ValidationError<'static>),
}

impl fmt::Display for TypedefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TypedefError::Schema(msg) => write!(f, "schema error: {msg}"),
            TypedefError::Offset { field_path, reason } => {
                write!(f, "offset error at {field_path}: {reason}")
            }
            TypedefError::Access { field_path, reason } => {
                write!(f, "access error at {field_path}: {reason}")
            }
            TypedefError::Validation(inner) => write!(f, "validation error: {inner}"),
        }
    }
}

impl std::error::Error for TypedefError {}
