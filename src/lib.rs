//! Scriblet core library.
//!
//! Everything that does not need a window lives here so it can be unit tested
//! on any host: the snippet model, SQLite storage, the expansion matcher,
//! template rendering, enterprise synchronization, import/export, and the
//! platform runtime that hooks the keyboard.

pub mod app;
pub mod autostart;
pub mod enterprise;
pub mod expansion;
pub mod instance;
pub mod model;
#[path = "runtime_winfix.rs"]
pub mod runtime;
#[cfg_attr(target_os = "windows", allow(dead_code))]
#[path = "runtime.rs"]
mod runtime_legacy;
pub mod storage;
pub mod template;
pub mod transfer;

/// Application name used for data directories, logs, and OS registration.
pub const APP_NAME: &str = "Scriblet";
/// Semantic version of the running build, sourced from Cargo.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
