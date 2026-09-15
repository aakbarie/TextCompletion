//! Scriblet core library.
//!
//! Everything that does not need a window lives here so it can be unit tested
//! on any host: the snippet model, SQLite storage, the expansion matcher,
//! template rendering, enterprise synchronization, import/export, the platform
//! keyboard runtime, and the native Phase 2 work-surface primitives.

pub mod app;
pub mod autostart;
pub mod commands;
pub mod completion;
pub mod enterprise;
pub mod expansion;
pub mod instance;
pub mod model;
pub mod note;
pub mod note_index;
pub mod provenance;
#[path = "runtime_winfix.rs"]
pub mod runtime;
#[cfg_attr(target_os = "windows", allow(dead_code))]
#[path = "runtime.rs"]
mod runtime_legacy;
pub mod storage;
pub mod template;
pub mod transfer;
pub mod workspace;
pub mod writer;

/// Application name used for data directories, logs, and OS registration.
pub const APP_NAME: &str = "Scriblet";
/// Semantic version of the running build, sourced from Cargo.
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
