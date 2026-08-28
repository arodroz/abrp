//! Mac-side builders that produce installable Packs from open data feeds

/// Returns the name of this crate.
pub fn crate_name() -> &'static str {
    "pipeline"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_crate_name() {
        assert_eq!(crate_name(), "pipeline");
    }
}
