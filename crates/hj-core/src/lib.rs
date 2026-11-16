//! Hobby Jogger's application core: state, request gating and machine orchestration, independent
//! of the UI framework. See `docs/architecture.md`.

pub mod contour;
mod core;
pub mod map;
pub mod probe;
pub mod program;
pub mod settings;
pub mod state;
pub mod sync;

pub use crate::core::{Core, CoreError, Sink, Snapshot, StateUpdate};
