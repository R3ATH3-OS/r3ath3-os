//! Glue Duty. The rules and the text grid are one source for the wasm module
//! and the native replay. See `docs/WASM.md`.
//!
//! The wasm build is `no_std`. The native replay is a std binary that links
//! this crate, so `no_std` is on only for `wasm32` (a host cdylib in the same
//! package would otherwise need its own panic handler).
#![cfg_attr(target_arch = "wasm32", no_std)]

pub mod logic;
pub mod text;

#[cfg(target_arch = "wasm32")]
mod abi;
