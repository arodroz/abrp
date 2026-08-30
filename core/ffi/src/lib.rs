//! UniFFI boundary exposing the Planner to Swift (wayfinder #34, ADR 0004):
//! one coarse `Planner` object, proc-macro exports, no UDL. Rust owns
//! Routing, the Energy Model and Charging Stop search (pure CPU, no I/O, no
//! async runtime); Swift owns everything else. See
//! docs/adr/0004-rust-boundary-uniffi.md.

mod error;
mod mapping;
mod planner;
mod triplog;
mod types;

pub use error::PlannerError;
pub use planner::{verify_region_pack, Planner};
pub use types::{
    FfiCalibrationResult, FfiGeoPoint, FfiLeg, FfiLegInput, FfiPlan, FfiPlanAlt, FfiPlanRequest,
    FfiSocPoint, FfiStop, FfiTripFit, FfiVehicle, FfiWaypoint,
};

uniffi::setup_scaffolding!();
