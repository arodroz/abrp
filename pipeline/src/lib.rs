//! Mac-side builders that produce installable Packs from open data feeds.

pub mod ch;
pub mod slice_import;
pub mod writer;

pub use ch::{ch_prepare, ChStats};
pub use slice_import::{build_base_model, ImportStats};
pub use writer::{write_rpack, PackMeta, WriteError};
