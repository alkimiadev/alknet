//! Abort cascade logic for nested calls (ADR-016).
//!
//! When `call.aborted` arrives for a parent request, the protocol cascades
//! the abort to all non-terminal descendants in the call tree. Default
//! policy is `abort-dependents`; `continue-running` is an opt-in.

// TODO: implement
