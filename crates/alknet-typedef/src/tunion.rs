//! TUnion discriminator dispatch (ADR-097 §4).
//!
//! TUnion supports two discriminator kinds: byte-offset (protocol
//! dispatch, e.g., SFTP type bytes) and field-name (typedef.ts string
//! pattern).

// TODO: implement