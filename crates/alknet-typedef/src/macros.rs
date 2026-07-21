//! Macros for generating repetitive code across the 17 TypeDef kinds.
//!
//! These macros eliminate boilerplate in validation and data access.
//! Each macro takes a compact specification and generates the full
//! implementation, ensuring consistency across all types.

// ---------------------------------------------------------------------------
// Validation macros
// ---------------------------------------------------------------------------

/// Generate a signed integer validator struct and its factory closure.
#[macro_export]
macro_rules! define_int_validator {
    ($validator_struct:ident, $factory_fn:ident, $keyword:literal, $min:literal, $max:literal) => {
        struct $validator_struct;
        impl jsonschema::Keyword for $validator_struct {
            fn validate<'i>(
                &self,
                instance: &'i serde_json::Value,
            ) -> Result<(), jsonschema::ValidationError<'i>> {
                match instance.as_i64() {
                    Some(n) if ($min..=$max).contains(&n) => Ok(()),
                    _ => Err(jsonschema::ValidationError::custom(concat!(
                        "expected an integer in range [",
                        stringify!($min),
                        ", ",
                        stringify!($max),
                        "]"
                    ))),
                }
            }
            fn is_valid(&self, instance: &serde_json::Value) -> bool {
                instance
                    .as_i64()
                    .is_some_and(|n| ($min..=$max).contains(&n))
            }
        }

        fn $factory_fn<'a>(
            _parent: &'a serde_json::Map<String, serde_json::Value>,
            value: &'a serde_json::Value,
            _path: jsonschema::paths::Location,
        ) -> Result<Box<dyn jsonschema::Keyword>, jsonschema::ValidationError<'a>> {
            if value.as_bool() == Some(true) {
                Ok(Box::new($validator_struct))
            } else {
                Err(jsonschema::ValidationError::schema(concat!(
                    $keyword,
                    " must be set to true"
                )))
            }
        }
    };
}

/// Generate an unsigned integer validator struct and its factory closure.
#[macro_export]
macro_rules! define_uint_validator {
    ($validator_struct:ident, $factory_fn:ident, $keyword:literal, $max:literal) => {
        struct $validator_struct;
        impl jsonschema::Keyword for $validator_struct {
            fn validate<'i>(
                &self,
                instance: &'i serde_json::Value,
            ) -> Result<(), jsonschema::ValidationError<'i>> {
                match instance.as_u64() {
                    Some(n) if n <= $max => Ok(()),
                    _ => Err(jsonschema::ValidationError::custom(concat!(
                        "expected an unsigned integer in range [0, ",
                        stringify!($max),
                        "]"
                    ))),
                }
            }
            fn is_valid(&self, instance: &serde_json::Value) -> bool {
                instance.as_u64().is_some_and(|n| n <= $max)
            }
        }

        fn $factory_fn<'a>(
            _parent: &'a serde_json::Map<String, serde_json::Value>,
            value: &'a serde_json::Value,
            _path: jsonschema::paths::Location,
        ) -> Result<Box<dyn jsonschema::Keyword>, jsonschema::ValidationError<'a>> {
            if value.as_bool() == Some(true) {
                Ok(Box::new($validator_struct))
            } else {
                Err(jsonschema::ValidationError::schema(concat!(
                    $keyword,
                    " must be set to true"
                )))
            }
        }
    };
}

/// Generate a float validator struct and its factory closure.
#[macro_export]
macro_rules! define_float_validator {
    ($validator_struct:ident, $factory_fn:ident, $keyword:literal, $error_msg:literal) => {
        struct $validator_struct;
        impl jsonschema::Keyword for $validator_struct {
            fn validate<'i>(
                &self,
                instance: &'i serde_json::Value,
            ) -> Result<(), jsonschema::ValidationError<'i>> {
                match instance.as_f64() {
                    Some(f) if f.is_finite() => Ok(()),
                    _ => Err(jsonschema::ValidationError::custom($error_msg)),
                }
            }
            fn is_valid(&self, instance: &serde_json::Value) -> bool {
                instance.as_f64().is_some_and(|f| f.is_finite())
            }
        }

        fn $factory_fn<'a>(
            _parent: &'a serde_json::Map<String, serde_json::Value>,
            value: &'a serde_json::Value,
            _path: jsonschema::paths::Location,
        ) -> Result<Box<dyn jsonschema::Keyword>, jsonschema::ValidationError<'a>> {
            if value.as_bool() == Some(true) {
                Ok(Box::new($validator_struct))
            } else {
                Err(jsonschema::ValidationError::schema(concat!(
                    $keyword,
                    " must be set to true"
                )))
            }
        }
    };
}

/// Generate a simple type-check validator (object/array/boolean) and its factory.
#[macro_export]
macro_rules! define_type_validator {
    ($validator_struct:ident, $factory_fn:ident, $keyword:literal, $check_method:ident, $error_msg:literal) => {
        struct $validator_struct;
        impl jsonschema::Keyword for $validator_struct {
            fn validate<'i>(
                &self,
                instance: &'i serde_json::Value,
            ) -> Result<(), jsonschema::ValidationError<'i>> {
                if instance.$check_method() {
                    Ok(())
                } else {
                    Err(jsonschema::ValidationError::custom($error_msg))
                }
            }
            fn is_valid(&self, instance: &serde_json::Value) -> bool {
                instance.$check_method()
            }
        }

        fn $factory_fn<'a>(
            _parent: &'a serde_json::Map<String, serde_json::Value>,
            value: &'a serde_json::Value,
            _path: jsonschema::paths::Location,
        ) -> Result<Box<dyn jsonschema::Keyword>, jsonschema::ValidationError<'a>> {
            if value.as_bool() == Some(true) {
                Ok(Box::new($validator_struct))
            } else {
                Err(jsonschema::ValidationError::schema(concat!(
                    $keyword,
                    " must be set to true"
                )))
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Data access macros
// ---------------------------------------------------------------------------

/// Generate a pair of read/write functions for a fixed-size endian-sensitive type.
#[macro_export]
macro_rules! define_read_write_endian {
    ($rust_ty:ty, $read_name:ident, $write_name:ident, $size:literal) => {
        #[doc = concat!(
            "Read a `",
            stringify!($rust_ty),
            "` at `offset` from `buffer`, applying `endian`."
        )]
        pub fn $read_name(
            buffer: &[u8],
            offset: usize,
            field_path: &str,
            endian: $crate::Endian,
        ) -> Result<$rust_ty, $crate::TypedefError> {
            let bytes: [u8; $size] = $crate::data_access::read_array(buffer, offset, field_path)?;
            Ok(match endian {
                $crate::Endian::Little => <$rust_ty>::from_le_bytes(bytes),
                $crate::Endian::Big => <$rust_ty>::from_be_bytes(bytes),
            })
        }

        #[doc = concat!(
            "Write a `",
            stringify!($rust_ty),
            "` `value` at `offset` into `buffer`, applying `endian`."
        )]
        pub fn $write_name(
            buffer: &mut [u8],
            offset: usize,
            value: $rust_ty,
            field_path: &str,
            endian: $crate::Endian,
        ) -> Result<(), $crate::TypedefError> {
            let bytes = match endian {
                $crate::Endian::Little => value.to_le_bytes(),
                $crate::Endian::Big => value.to_be_bytes(),
            };
            $crate::data_access::write_array(buffer, offset, bytes, field_path)
        }
    };
}

/// Generate a pair of read/write functions for a fixed-size endian-insensitive type.
#[macro_export]
macro_rules! define_read_write_ne {
    ($rust_ty:ty, $read_name:ident, $write_name:ident, $size:literal, $read_expr:expr) => {
        #[doc = concat!(
            "Read a `",
            stringify!($rust_ty),
            "` at `offset` from `buffer`."
        )]
        pub fn $read_name(
            buffer: &[u8],
            offset: usize,
            field_path: &str,
        ) -> Result<$rust_ty, $crate::TypedefError> {
            let bytes: [u8; $size] = $crate::data_access::read_array(buffer, offset, field_path)?;
            Ok($read_expr(bytes))
        }

        #[doc = concat!(
            "Write a `",
            stringify!($rust_ty),
            "` `value` at `offset` into `buffer`."
        )]
        pub fn $write_name(
            buffer: &mut [u8],
            offset: usize,
            value: $rust_ty,
            field_path: &str,
        ) -> Result<(), $crate::TypedefError> {
            $crate::data_access::write_array(buffer, offset, value.to_ne_bytes(), field_path)
        }
    };
}
