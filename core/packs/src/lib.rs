//! Region/Map/Charger Pack binary format: writers and mmap readers

/// Returns the name of this crate.
pub fn crate_name() -> &'static str {
    "packs"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_crate_name() {
        assert_eq!(crate_name(), "packs");
    }
}
