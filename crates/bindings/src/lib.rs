//! SpacetimeDB client bindings for Dar's module: tables, views, reducers and procedures.
//! `src/generated` is written by `cargo xtask bindings` from the built module and ignored by
//! Git; the module's names and arguments are the contract the client and tests rely on.
#![allow(clippy::all, unused_imports, non_camel_case_types, non_snake_case)]

#[path = "generated/mod.rs"]
mod generated;
pub use generated::*;
