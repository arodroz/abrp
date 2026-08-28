//! Charging Stop optimiser: label-setting search producing Plans

/// Returns the name of this crate.
pub fn crate_name() -> &'static str {
    "optimiser"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_crate_name() {
        assert_eq!(crate_name(), "optimiser");
    }
}
