//! Aligned static `OffsetMap` — Mode 2 of the two layout modes (ADR-096).
//!
//! Fields have fixed positions with natural alignment padding.
//! Variable-length fields get a 4-byte length prefix at a known offset;
//! the variable data is not included in the static layout. Used for
//! mmap-friendly formats (metatensor, safetensors).

// TODO: implement