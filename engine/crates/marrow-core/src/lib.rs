//! Genealogy engine: decides what happened to every line between commits (docs/specs/genealogy.md).

pub mod align;
pub mod constants;
pub mod content;
pub mod correspondence;
pub mod error;
pub mod histogram;
pub mod history;
pub mod language;
pub mod moves;
pub mod normalize;
pub mod pipeline;
pub mod select;

pub use error::Error;

/// Stable hash of a repo-relative path, so stored rows can name a file without storing its path.
pub fn path_hash(path: &str) -> String {
    format!("{:016x}", xxhash_rust::xxh3::xxh3_64(path.as_bytes()))
}

pub use content::ContentId;
pub use pipeline::{trace_repository, DecidingLayer, Fate, Position, State, Trace, TracedLine};
