//! Mac-side builders that produce installable Packs from open data feeds.

pub mod ch;
pub mod writer;

pub use ch::{ch_prepare, ChStats};
pub use writer::{write_rpack, PackMeta, WriteError};
