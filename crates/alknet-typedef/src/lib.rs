//! alknet-typedef: The binary struct engine.
//!
//! Takes a JSON Schema with `TypeDef:*` custom keywords and produces
//! an offset map, read/write functions, and validation — all driven
//! by the schema. The schema is the format definition; the engine is
//! generic.
//!
//! ## Architecture
//!
//! - **Schema layer** ([`schema`]): TypeDef kind detection, annotation
//!   parsing, `$ref` normalization, endianness.
//! - **Layout engine** ([`offset_map`], [`layout_builder`],
//!   [`sequential_reader`]): Two layout modes — aligned static for
//!   mmap-friendly formats, packed sequential for protocol wire formats.
//! - **Data access** ([`data_access`]): Typed read/write at computed
//!   offsets, zero-copy for fixed-size types.
//! - **TUnion dispatch** ([`tunion`]): Byte-offset and field-name
//!   discriminator dispatch.
//! - **Validation** ([`validation`]): Custom keyword validators for all
//!   17 `TypeDef:*` kinds, delegated to the `jsonschema` crate.
//! - **Engine** ([`engine`]): `TypedefEngine` — the compiled form of a
//!   schema, combining layout and validation.

pub mod data_access;
pub mod engine;
pub mod error;
pub mod layout_builder;
pub mod offset_map;
pub mod schema;
pub mod sequential_reader;
pub mod tunion;
pub mod validation;

pub use error::TypedefError;
pub use schema::{
    parse_align, parse_discriminator, parse_encoding, parse_endian, parse_max_length,
    normalize_refs, DiscriminatorKind, Endian, VariableEncoding,
};
pub use validation::build_validator;
pub use offset_map::{ByteRange, OffsetMap};
pub use layout_builder::{FieldPosition, LayoutBuilder, PackedLayout};
pub use sequential_reader::{FieldValue, SequentialReader};
pub use tunion::UnionDispatch;
pub use engine::{LayoutMode, TypedefEngine};