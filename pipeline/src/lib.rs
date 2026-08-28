//! Mac-side builders that produce installable Packs from open data feeds.

pub mod writer;

pub use writer::{write_rpack, PackMeta, WriteError};
