//! Energy Model: per-Leg energy prediction from a Vehicle Model

/// Returns the name of this crate.
pub fn crate_name() -> &'static str {
    "energy"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_crate_name() {
        assert_eq!(crate_name(), "energy");
    }
}
