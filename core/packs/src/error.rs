//! Error type shared by the `.rpack` reader and the in-memory model's own
//! consistency checks (used by the pipeline writer before it serializes).

use std::fmt;

#[derive(Debug)]
pub enum RpackError {
    Io(std::io::Error),
    BadMagic,
    UnsupportedVersion {
        major: u16,
    },
    Truncated {
        needed: u64,
        available: u64,
    },
    Misaligned {
        offset: u64,
    },
    SectionOutOfBounds {
        section_id: u32,
    },
    SectionSizeMismatch {
        section_id: u32,
        len_bytes: u64,
        elem_size: u64,
    },
    MissingSection {
        section_id: u32,
    },
    ChecksumMismatch {
        section_id: u32,
    },
    Validation(String),
}

impl fmt::Display for RpackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RpackError::Io(e) => write!(f, "io error: {e}"),
            RpackError::BadMagic => write!(f, "bad magic: not an rpack file"),
            RpackError::UnsupportedVersion { major } => {
                write!(f, "unsupported format major version: {major}")
            }
            RpackError::Truncated { needed, available } => {
                write!(f, "truncated file: needed {needed} bytes, have {available}")
            }
            RpackError::Misaligned { offset } => {
                write!(f, "section offset {offset} is not 8-byte aligned")
            }
            RpackError::SectionOutOfBounds { section_id } => {
                write!(f, "section {section_id} extends past end of file")
            }
            RpackError::SectionSizeMismatch {
                section_id,
                len_bytes,
                elem_size,
            } => {
                write!(
                    f,
                    "section {section_id} length {len_bytes} is not a multiple of element size {elem_size}"
                )
            }
            RpackError::MissingSection { section_id } => {
                write!(f, "missing required section {section_id}")
            }
            RpackError::ChecksumMismatch { section_id } => {
                write!(f, "checksum mismatch in section {section_id}")
            }
            RpackError::Validation(msg) => write!(f, "model validation failed: {msg}"),
        }
    }
}

impl std::error::Error for RpackError {}

impl From<std::io::Error> for RpackError {
    fn from(e: std::io::Error) -> Self {
        RpackError::Io(e)
    }
}
