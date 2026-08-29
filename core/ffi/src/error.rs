//! The one error enum crossing the boundary (ADR 0004 point 5), surfacing
//! as Swift `throws`. `thiserror`-free: each variant carries its own
//! pre-formatted `String` message rather than a derived `Display` over
//! structured fields.

/// Errors a `Planner` call can throw.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Error)]
pub enum PlannerError {
    /// Required data isn't loaded yet: either the Region Pack failed to
    /// open, or `plan()`/`energy()` was called before `load_chargers`.
    PackMissing {
        message: String,
    },
    InvalidRequest {
        message: String,
    },
    NoRouteFound {
        message: String,
    },
    /// The caller's `cancel()` was observed before the Plan completed.
    Cancelled {
        message: String,
    },
}

impl std::fmt::Display for PlannerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlannerError::PackMissing { message } => write!(f, "pack missing: {message}"),
            PlannerError::InvalidRequest { message } => write!(f, "invalid request: {message}"),
            PlannerError::NoRouteFound { message } => write!(f, "no route found: {message}"),
            PlannerError::Cancelled { message } => write!(f, "cancelled: {message}"),
        }
    }
}

impl std::error::Error for PlannerError {}
