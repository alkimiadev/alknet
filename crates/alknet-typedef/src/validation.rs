//! Custom keyword validators for all 17 `TypeDef:*` kinds, registered
//! via `jsonschema::options().with_keyword(...)`.
//!
//! Per ADR-098: the `jsonschema` crate handles all structural validation;
//! the custom keywords only validate leaf type constraints. Each validator
//! is a small (~10 line) struct implementing [`jsonschema::Keyword`].
//!
//! The factory closures reject schemas where the keyword is not set to
//! `true` (returning [`jsonschema::ValidationError::schema`]). A few
//! factories read parent context (e.g. `maxLength`) to pass into the
//! validator struct.

use crate::error::TypedefError;
use jsonschema::{Keyword, ValidationError};
use serde_json::{Map, Value};

/// Build a jsonschema validator with all 17 `TypeDef:*` custom keywords
/// registered.
///
/// The returned validator can validate JSON representations of data
/// against the schema's type constraints. Structural validation
/// (`properties`, `required`, `items`, `enum`, ...) is handled by
/// jsonschema's built-in keywords; the custom keywords only check leaf
/// type constraints (range, finiteness, RFC 3339 shape, ...).
///
/// # Errors
///
/// Returns [`TypedefError::Schema`] if the schema is malformed or the
/// underlying jsonschema validator cannot be built.
pub fn build_validator(schema: &Value) -> Result<jsonschema::Validator, TypedefError> {
    jsonschema::options()
        .with_keyword("TypeDef:Float32", float32_factory)
        .with_keyword("TypeDef:Float64", float64_factory)
        .with_keyword("TypeDef:Int8", int8_factory)
        .with_keyword("TypeDef:Int16", int16_factory)
        .with_keyword("TypeDef:Int32", int32_factory)
        .with_keyword("TypeDef:Uint8", uint8_factory)
        .with_keyword("TypeDef:Uint16", uint16_factory)
        .with_keyword("TypeDef:Uint32", uint32_factory)
        .with_keyword("TypeDef:Boolean", boolean_factory)
        .with_keyword("TypeDef:String", string_factory)
        .with_keyword("TypeDef:Bytes", bytes_factory)
        .with_keyword("TypeDef:Enum", enum_factory)
        .with_keyword("TypeDef:Struct", struct_factory)
        .with_keyword("TypeDef:Union", union_factory)
        .with_keyword("TypeDef:Array", array_factory)
        .with_keyword("TypeDef:Record", record_factory)
        .with_keyword("TypeDef:Timestamp", timestamp_factory)
        .build(schema)
        .map_err(|e| TypedefError::Schema(format!("validator build failed: {e}")))
}

// ---------------------------------------------------------------------------
// Numeric validators
// ---------------------------------------------------------------------------

struct Float32Validator;
impl Keyword for Float32Validator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        match instance.as_f64() {
            Some(f) if f.is_finite() => Ok(()),
            _ => Err(ValidationError::custom("expected a finite f32-compatible number")),
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.as_f64().is_some_and(|f| f.is_finite())
    }
}

struct Float64Validator;
impl Keyword for Float64Validator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        match instance.as_f64() {
            Some(f) if f.is_finite() => Ok(()),
            _ => Err(ValidationError::custom("expected a finite f64 number")),
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.as_f64().is_some_and(|f| f.is_finite())
    }
}

struct Int8Validator;
impl Keyword for Int8Validator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        match instance.as_i64() {
            Some(n) if (-128..=127).contains(&n) => Ok(()),
            _ => Err(ValidationError::custom("expected an integer in range [-128, 127]")),
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.as_i64().is_some_and(|n| (-128..=127).contains(&n))
    }
}

struct Int16Validator;
impl Keyword for Int16Validator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        match instance.as_i64() {
            Some(n) if (-32_768..=32_767).contains(&n) => Ok(()),
            _ => Err(ValidationError::custom("expected an integer in range [-32768, 32767]")),
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance
            .as_i64()
            .is_some_and(|n| (-32_768..=32_767).contains(&n))
    }
}

struct Int32Validator;
impl Keyword for Int32Validator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        match instance.as_i64() {
            Some(n) if (-2_147_483_648..=2_147_483_647).contains(&n) => Ok(()),
            _ => Err(ValidationError::custom(
                "expected an integer in range [-2147483648, 2147483647]",
            )),
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance
            .as_i64()
            .is_some_and(|n| (-2_147_483_648..=2_147_483_647).contains(&n))
    }
}

struct Uint8Validator;
impl Keyword for Uint8Validator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        match instance.as_u64() {
            Some(n) if n <= 255 => Ok(()),
            _ => Err(ValidationError::custom("expected an unsigned integer in range [0, 255]")),
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.as_u64().is_some_and(|n| n <= 255)
    }
}

struct Uint16Validator;
impl Keyword for Uint16Validator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        match instance.as_u64() {
            Some(n) if n <= 65_535 => Ok(()),
            _ => Err(ValidationError::custom("expected an unsigned integer in range [0, 65535]")),
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.as_u64().is_some_and(|n| n <= 65_535)
    }
}

struct Uint32Validator;
impl Keyword for Uint32Validator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        match instance.as_u64() {
            Some(n) if n <= 4_294_967_295 => Ok(()),
            _ => Err(ValidationError::custom(
                "expected an unsigned integer in range [0, 4294967295]",
            )),
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.as_u64().is_some_and(|n| n <= 4_294_967_295)
    }
}

// ---------------------------------------------------------------------------
// String and binary validators
// ---------------------------------------------------------------------------

struct StringValidator {
    max_length: Option<usize>,
}
impl Keyword for StringValidator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        match instance.as_str() {
            Some(s) => {
                if let Some(max) = self.max_length {
                    if s.len() > max {
                        return Err(ValidationError::custom(format!(
                            "string byte length {} exceeds maxLength {max}",
                            s.len()
                        )));
                    }
                }
                Ok(())
            }
            None => Err(ValidationError::custom("expected a string")),
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.as_str().is_some_and(|s| {
            self.max_length.is_none_or(|max| s.len() <= max)
        })
    }
}

struct BytesValidator {
    max_length: Option<usize>,
}
impl Keyword for BytesValidator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        match instance.as_str() {
            Some(s) => {
                if let Some(max) = self.max_length {
                    if s.len() > max {
                        return Err(ValidationError::custom(format!(
                            "bytes length {} exceeds maxLength {max}",
                            s.len()
                        )));
                    }
                }
                Ok(())
            }
            None => Err(ValidationError::custom("expected a string for bytes")),
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.as_str().is_some_and(|s| {
            self.max_length.is_none_or(|max| s.len() <= max)
        })
    }
}

/// `TypeDef:Enum` is a layout marker — the built-in `enum` keyword
/// handles value-membership validation. The custom keyword exists solely
/// for the layout engine to recognize the type as a fixed-size u32 index.
struct EnumValidator;
impl Keyword for EnumValidator {
    fn validate<'i>(&self, _instance: &'i Value) -> Result<(), ValidationError<'i>> {
        Ok(())
    }
    fn is_valid(&self, _instance: &Value) -> bool {
        true
    }
}

struct TimestampValidator;
impl Keyword for TimestampValidator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        match instance.as_str() {
            Some(s) if is_rfc3339_timestamp(s) => Ok(()),
            _ => Err(ValidationError::custom("expected an RFC 3339 timestamp string")),
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.as_str().is_some_and(is_rfc3339_timestamp)
    }
}

/// Simple RFC 3339 / ISO 8601 datetime check: `YYYY-MM-DDTHH:MM:SS`
/// optionally followed by `Z` or a timezone offset.
fn is_rfc3339_timestamp(s: &str) -> bool {
    let parts: Vec<&str> = s.splitn(2, 'T').collect();
    if parts.len() != 2 {
        return false;
    }
    let date_parts: Vec<&str> = parts[0].split('-').collect();
    if date_parts.len() != 3 {
        return false;
    }
    let time_part = parts[1];
    let time_clean = if let Some(pos) = time_part.find(['Z', '+']) {
        &time_part[..pos]
    } else if let Some(pos) = time_part.rfind('-') {
        if pos >= 8 {
            &time_part[..pos]
        } else {
            time_part
        }
    } else {
        time_part
    };
    let time_parts: Vec<&str> = time_clean.split(':').collect();
    if time_parts.len() < 2 || time_parts.len() > 3 {
        return false;
    }
    date_parts[0]
        .parse::<u16>()
        .is_ok_and(|y| y > 0)
        && date_parts[1].parse::<u8>().is_ok_and(|m| (1..=12).contains(&m))
        && date_parts[2].parse::<u8>().is_ok_and(|d| (1..=31).contains(&d))
        && time_parts[0].parse::<u8>().is_ok_and(|h| h <= 23)
        && time_parts[1].parse::<u8>().is_ok_and(|m| m <= 59)
}

// ---------------------------------------------------------------------------
// Composite validators (structural checks delegated to jsonschema)
// ---------------------------------------------------------------------------

struct StructValidator;
impl Keyword for StructValidator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        if instance.is_object() {
            Ok(())
        } else {
            Err(ValidationError::custom("expected an object"))
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.is_object()
    }
}

/// `TypeDef:Union` discriminator and variant conformance are validated
/// by jsonschema's built-in keywords (`properties`, `oneOf`, etc.). The
/// custom keyword only asserts the instance is an object.
struct UnionValidator;
impl Keyword for UnionValidator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        if instance.is_object() {
            Ok(())
        } else {
            Err(ValidationError::custom("expected an object for union"))
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.is_object()
    }
}

struct ArrayValidator;
impl Keyword for ArrayValidator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        if instance.is_array() {
            Ok(())
        } else {
            Err(ValidationError::custom("expected an array"))
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.is_array()
    }
}

/// `TypeDef:Record` value-type conformance is validated by jsonschema's
/// built-in `additionalProperties` keyword. The custom keyword only
/// asserts the instance is an object.
struct RecordValidator;
impl Keyword for RecordValidator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        if instance.is_object() {
            Ok(())
        } else {
            Err(ValidationError::custom("expected an object for record"))
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.is_object()
    }
}

struct BooleanValidator;
impl Keyword for BooleanValidator {
    fn validate<'i>(&self, instance: &'i Value) -> Result<(), ValidationError<'i>> {
        if instance.is_boolean() {
            Ok(())
        } else {
            Err(ValidationError::custom("expected a boolean"))
        }
    }
    fn is_valid(&self, instance: &Value) -> bool {
        instance.is_boolean()
    }
}

// ---------------------------------------------------------------------------
// Factory closures
// ---------------------------------------------------------------------------

fn float32_factory<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() == Some(true) {
        Ok(Box::new(Float32Validator))
    } else {
        Err(ValidationError::schema("TypeDef:Float32 must be set to true"))
    }
}

fn float64_factory<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() == Some(true) {
        Ok(Box::new(Float64Validator))
    } else {
        Err(ValidationError::schema("TypeDef:Float64 must be set to true"))
    }
}

fn int8_factory<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() == Some(true) {
        Ok(Box::new(Int8Validator))
    } else {
        Err(ValidationError::schema("TypeDef:Int8 must be set to true"))
    }
}

fn int16_factory<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() == Some(true) {
        Ok(Box::new(Int16Validator))
    } else {
        Err(ValidationError::schema("TypeDef:Int16 must be set to true"))
    }
}

fn int32_factory<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() == Some(true) {
        Ok(Box::new(Int32Validator))
    } else {
        Err(ValidationError::schema("TypeDef:Int32 must be set to true"))
    }
}

fn uint8_factory<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() == Some(true) {
        Ok(Box::new(Uint8Validator))
    } else {
        Err(ValidationError::schema("TypeDef:Uint8 must be set to true"))
    }
}

fn uint16_factory<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() == Some(true) {
        Ok(Box::new(Uint16Validator))
    } else {
        Err(ValidationError::schema("TypeDef:Uint16 must be set to true"))
    }
}

fn uint32_factory<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() == Some(true) {
        Ok(Box::new(Uint32Validator))
    } else {
        Err(ValidationError::schema("TypeDef:Uint32 must be set to true"))
    }
}

fn boolean_factory<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() == Some(true) {
        Ok(Box::new(BooleanValidator))
    } else {
        Err(ValidationError::schema("TypeDef:Boolean must be set to true"))
    }
}

fn string_factory<'a>(
    parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() != Some(true) {
        return Err(ValidationError::schema("TypeDef:String must be set to true"));
    }
    let max_length = parent
        .get("maxLength")
        .and_then(Value::as_u64)
        .map(|n| n as usize);
    Ok(Box::new(StringValidator { max_length }))
}

fn bytes_factory<'a>(
    parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() != Some(true) {
        return Err(ValidationError::schema("TypeDef:Bytes must be set to true"));
    }
    let max_length = parent
        .get("maxLength")
        .and_then(Value::as_u64)
        .map(|n| n as usize);
    Ok(Box::new(BytesValidator { max_length }))
}

fn enum_factory<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() == Some(true) {
        Ok(Box::new(EnumValidator))
    } else {
        Err(ValidationError::schema("TypeDef:Enum must be set to true"))
    }
}

fn struct_factory<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() == Some(true) {
        Ok(Box::new(StructValidator))
    } else {
        Err(ValidationError::schema("TypeDef:Struct must be set to true"))
    }
}

fn union_factory<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() == Some(true) {
        Ok(Box::new(UnionValidator))
    } else {
        Err(ValidationError::schema("TypeDef:Union must be set to true"))
    }
}

fn array_factory<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() == Some(true) {
        Ok(Box::new(ArrayValidator))
    } else {
        Err(ValidationError::schema("TypeDef:Array must be set to true"))
    }
}

fn record_factory<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() == Some(true) {
        Ok(Box::new(RecordValidator))
    } else {
        Err(ValidationError::schema("TypeDef:Record must be set to true"))
    }
}

fn timestamp_factory<'a>(
    _parent: &'a Map<String, Value>,
    value: &'a Value,
    _path: jsonschema::paths::Location,
) -> Result<Box<dyn Keyword>, ValidationError<'a>> {
    if value.as_bool() == Some(true) {
        Ok(Box::new(TimestampValidator))
    } else {
        Err(ValidationError::schema("TypeDef:Timestamp must be set to true"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn validator_for(schema: &Value) -> jsonschema::Validator {
        build_validator(schema).expect("validator should build")
    }

    #[test]
    fn validates_valid_struct_instance() {
        let schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": {
                "id": { "TypeDef:Uint32": true, "type": "integer" },
                "score": { "TypeDef:Float32": true, "type": "number" },
                "flag": { "TypeDef:Uint8": true, "type": "integer" },
                "count": { "TypeDef:Uint16": true, "type": "integer" }
            },
            "required": ["id", "score", "flag", "count"]
        });
        let validator = validator_for(&schema);
        let instance = json!({
            "id": 42,
            "score": 3.5,
            "flag": 1,
            "count": 1000
        });
        assert!(validator.is_valid(&instance));
    }

    #[test]
    fn rejects_uint32_out_of_range() {
        let schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": { "id": { "TypeDef:Uint32": true, "type": "integer" } },
            "required": ["id"]
        });
        let validator = validator_for(&schema);
        assert!(!validator.is_valid(&json!({"id": -1})));
        assert!(!validator.is_valid(&json!({"id": 5_000_000_000u64})));
    }

    #[test]
    fn validates_int8_range() {
        let schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": { "val": { "TypeDef:Int8": true, "type": "integer" } },
            "required": ["val"]
        });
        let validator = validator_for(&schema);
        assert!(validator.is_valid(&json!({"val": 0})));
        assert!(validator.is_valid(&json!({"val": 127})));
        assert!(validator.is_valid(&json!({"val": -128})));
        assert!(!validator.is_valid(&json!({"val": 128})));
        assert!(!validator.is_valid(&json!({"val": -129})));
    }

    #[test]
    fn validates_int16_and_int32_ranges() {
        let schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": {
                "i16": { "TypeDef:Int16": true, "type": "integer" },
                "i32": { "TypeDef:Int32": true, "type": "integer" }
            },
            "required": ["i16", "i32"]
        });
        let validator = validator_for(&schema);
        assert!(validator.is_valid(&json!({"i16": 32767, "i32": 2147483647})));
        assert!(validator.is_valid(&json!({"i16": -32768, "i32": -2147483648})));
        assert!(!validator.is_valid(&json!({"i16": 32768, "i32": 0})));
        assert!(!validator.is_valid(&json!({"i16": 0, "i32": 2147483648u64})));
    }

    #[test]
    fn validates_uint16_and_uint32_ranges() {
        let schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": {
                "u16": { "TypeDef:Uint16": true, "type": "integer" },
                "u32": { "TypeDef:Uint32": true, "type": "integer" }
            },
            "required": ["u16", "u32"]
        });
        let validator = validator_for(&schema);
        assert!(validator.is_valid(&json!({"u16": 65535, "u32": 4294967295u64})));
        assert!(!validator.is_valid(&json!({"u16": 65536, "u32": 0})));
        assert!(!validator.is_valid(&json!({"u16": -1, "u32": 0})));
    }

    #[test]
    fn validates_float_finiteness() {
        let schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": {
                "f32": { "TypeDef:Float32": true, "type": "number" },
                "f64": { "TypeDef:Float64": true, "type": "number" }
            },
            "required": ["f32", "f64"]
        });
        let validator = validator_for(&schema);
        assert!(validator.is_valid(&json!({"f32": 3.5, "f64": 2.5})));
        assert!(validator.is_valid(&json!({"f32": 0, "f64": 0})));
        assert!(!validator.is_valid(&json!({"f32": "x", "f64": 0})));
    }

    #[test]
    fn validates_boolean() {
        let schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": { "active": { "TypeDef:Boolean": true, "type": "boolean" } },
            "required": ["active"]
        });
        let validator = validator_for(&schema);
        assert!(validator.is_valid(&json!({"active": true})));
        assert!(validator.is_valid(&json!({"active": false})));
        assert!(!validator.is_valid(&json!({"active": "yes"})));
    }

    #[test]
    fn validates_string_and_maxlength() {
        let schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": {
                "name": { "TypeDef:String": true, "type": "string", "maxLength": 5 }
            },
            "required": ["name"]
        });
        let validator = validator_for(&schema);
        assert!(validator.is_valid(&json!({"name": "hi"})));
        assert!(validator.is_valid(&json!({"name": "hello"})));
        assert!(!validator.is_valid(&json!({"name": "toolong"})));
        assert!(!validator.is_valid(&json!({"name": 42})));
    }

    #[test]
    fn validates_bytes_and_maxlength() {
        let schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": {
                "blob": { "TypeDef:Bytes": true, "type": "string", "maxLength": 4 }
            },
            "required": ["blob"]
        });
        let validator = validator_for(&schema);
        assert!(validator.is_valid(&json!({"blob": "abcd"})));
        assert!(!validator.is_valid(&json!({"blob": "abcde"})));
        assert!(!validator.is_valid(&json!({"blob": 42})));
    }

    #[test]
    fn enum_validator_is_noop_and_builtin_enum_handles_membership() {
        let schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": {
                "status": {
                    "TypeDef:Enum": true,
                    "type": "string",
                    "enum": ["ok", "error", "pending"]
                }
            },
            "required": ["status"]
        });
        let validator = validator_for(&schema);
        assert!(validator.is_valid(&json!({"status": "ok"})));
        assert!(validator.is_valid(&json!({"status": "error"})));
        assert!(!validator.is_valid(&json!({"status": "unknown"})));
    }

    #[test]
    fn validates_timestamp_rfc3339() {
        let schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": {
                "created_at": { "TypeDef:Timestamp": true, "type": "string" }
            },
            "required": ["created_at"]
        });
        let validator = validator_for(&schema);
        assert!(validator.is_valid(&json!({"created_at": "2026-07-20T15:30:00Z"})));
        assert!(validator.is_valid(&json!({"created_at": "2026-07-20T15:30:00"})));
        assert!(validator.is_valid(&json!({"created_at": "2026-07-20T15:30:00+02:00"})));
        assert!(!validator.is_valid(&json!({"created_at": "not-a-date"})));
    }

    #[test]
    fn validates_array_type() {
        let schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": {
                "items": {
                    "TypeDef:Array": true,
                    "type": "array",
                    "items": { "TypeDef:Uint8": true, "type": "integer" }
                }
            },
            "required": ["items"]
        });
        let validator = validator_for(&schema);
        assert!(validator.is_valid(&json!({"items": [1, 2, 3]})));
        assert!(!validator.is_valid(&json!({"items": "not-array"})));
    }

    #[test]
    fn validates_record_type() {
        let schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": {
                "counts": {
                    "TypeDef:Record": true,
                    "type": "object",
                    "additionalProperties": { "TypeDef:Uint32": true, "type": "integer" }
                }
            },
            "required": ["counts"]
        });
        let validator = validator_for(&schema);
        assert!(validator.is_valid(&json!({"counts": {"a": 1, "b": 2}})));
        assert!(!validator.is_valid(&json!({"counts": "not-object"})));
    }

    #[test]
    fn validates_union_type() {
        let schema = json!({
            "TypeDef:Struct": true,
            "type": "object",
            "properties": {
                "packet": {
                    "TypeDef:Union": true,
                    "type": "object",
                    "properties": {
                        "type": { "type": "string" }
                    },
                    "required": ["type"]
                }
            },
            "required": ["packet"]
        });
        let validator = validator_for(&schema);
        assert!(validator.is_valid(&json!({"packet": {"type": "read"}})));
        assert!(!validator.is_valid(&json!({"packet": "not-object"})));
    }

    #[test]
    fn build_validator_returns_schema_error_for_malformed_keyword() {
        let schema = json!({"TypeDef:Uint32": "not-a-bool"});
        let err = build_validator(&schema).expect_err("should fail");
        assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
    }

    #[test]
    fn build_validator_maps_build_error_to_typedef_error() {
        let schema = json!([1, 2, 3]);
        let err = build_validator(&schema).expect_err("schema must be an object");
        assert!(matches!(err, TypedefError::Schema(_)), "got {err:?}");
    }
}