//! UniFFI boundary exposing the planner to Swift.
//!
//! NOTE: package name `ffi`, but the crate dir is core/ffi; no uniffi
//! dependency yet — that arrives in a later ticket.

/// Returns the name of this crate.
pub fn crate_name() -> &'static str {
    "ffi"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_crate_name() {
        assert_eq!(crate_name(), "ffi");
    }
}
