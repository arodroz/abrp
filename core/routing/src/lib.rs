//! Routing Engine: contraction-hierarchy road-graph queries

/// Returns the name of this crate.
pub fn crate_name() -> &'static str {
    "routing"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_crate_name() {
        assert_eq!(crate_name(), "routing");
    }
}
