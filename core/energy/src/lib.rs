//! Energy Model: hybrid physics-core + calibrated-scalars prediction of a
//! Leg's energy use, per ADR 0003 (`docs/adr/0003-hybrid-energy-model.md`)
//! and `docs/research/ioniq5-energy-model.md`. Pure functions, no external
//! dependencies.

mod calibration;
mod charging;
mod conditions;
mod edge;
mod vehicle;

pub use calibration::Calibration;
pub use charging::charge_duration_s;
pub use conditions::{air_density, p_hvac_w, Conditions};
pub use edge::{edge_energy_wh, EdgeInput};
pub use vehicle::VehicleModel;

#[cfg(test)]
mod gate_tests;
