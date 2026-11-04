//! Platform-neutral building blocks of Meeting Notes AI.
//!
//! Everything here is plain Rust without macOS frameworks, so it can be unit
//! tested on any platform. The app crate wires these pieces into the
//! summary pipeline, the settings and the UI.

pub mod audio_decode;
pub mod cleanup;
pub mod diff;
pub mod ollama_cloud;
