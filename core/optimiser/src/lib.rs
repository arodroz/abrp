//! Charging Stop optimiser (wayfinder #33): the pure label-setting search
//! (ADR 0006 as amended by ADR 0010) over a `CandidateGraph` assembled by
//! the corridor layer from a Region Pack, the routing kernel, the Energy
//! Model and a Charger Pack. `plan_api::plan` is the top-level entry.

pub mod corridor;
pub mod plan_api;
pub mod search;
pub mod types;

pub use corridor::{
    assemble, parse_cpack, AssembleError, AssemblyStats, CorridorRequest, CpackError,
};
pub use plan_api::{plan, PlanRequest};
pub use search::solve;
pub use types::*;
