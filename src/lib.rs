//! Library face of xfs-win-driver.
//!
//! Exposes the modules that the binary's `main.rs` consumes so that
//! integration tests under `tests/` can also reach them. The CLI
//! entry point still lives in `main.rs`; this file is purely a
//! re-export shim. No additional logic.
//!
//! License: GPL-3.0-or-later (see Cargo.toml).

pub mod mount;
pub mod overlay;
pub mod probe;
