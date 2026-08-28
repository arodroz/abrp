//! Top-level Charging Stop optimiser entry point (wayfinder #33): assembles
//! the corridor, runs the search, and owns the corridor-widening rung
//! (ADR 0006 point 1) that sits outside `search::solve`'s own SoC
//! relaxation ladder (ADR 0006 point 4). NOT part of the crate yet -- see
//! the commented `pub mod plan_api;` in `lib.rs`; it compiles once
//! `search::solve` exists (wayfinder #33's search half).

use energy::{Calibration, VehicleModel};
use packs::Rpack;
use routing::Router;

use crate::corridor::{self, AssembleError, AssemblyStats, CorridorRequest};
use crate::search;
use crate::types::{ChargerSite, Plan, PlanFlag, SearchParams};

/// Narrow corridor width tried first (ADR 0006 point 1).
const NARROW_CORRIDOR_M: f64 = 3_000.0;
/// Widened corridor width tried when the narrow Plan is still infeasible.
const WIDE_CORRIDOR_M: f64 = 10_000.0;

/// A full Charging Stop optimiser request: the corridor plus the search's
/// SoC/objective parameters (ADR 0006 points 2-3, ADR 0010 points 1-5).
pub struct PlanRequest {
    pub corridor: CorridorRequest,
    pub depart_soc: f64,
    pub arrival_min_soc: f64,
    pub charger_arrival_min_soc: f64,
    pub charger_max_soc: f64,
    pub stops_bias: f64,
    pub battery_warmth: f64,
    pub offer_stop_free_alternative: bool,
}

/// Assembles the candidate graph and solves it, widening the corridor once
/// (ADR 0006 point 1: "widen to 10 km if infeasible") if the narrow-corridor
/// Plan still carries an infeasibility flag. `search::solve`'s own SoC
/// relaxation ladder (ADR 0006 point 4: Charger Arrival SoC -> 0%,
/// Destination Arrival SoC -> 0%) already ran before returning that Plan --
/// ADR 0006 orders corridor widening *after* those relaxations, so this is
/// the outer-most rung, tried only once both have already failed to avoid
/// the flag.
pub fn plan(
    pack: &Rpack,
    router: &Router,
    sites: &[ChargerSite],
    veh: &VehicleModel,
    calib: &Calibration,
    req: &PlanRequest,
) -> Result<(Plan, AssemblyStats), AssembleError> {
    let params = SearchParams {
        vehicle: veh,
        calibration: calib,
        depart_soc: req.depart_soc,
        arrival_min_soc: req.arrival_min_soc,
        charger_arrival_min_soc: req.charger_arrival_min_soc,
        charger_max_soc: req.charger_max_soc,
        stops_bias: req.stops_bias,
        battery_warmth: req.battery_warmth,
        offer_stop_free_alternative: req.offer_stop_free_alternative,
    };

    let (graph, stats) = corridor::assemble(
        pack,
        router,
        sites,
        veh,
        calib,
        &req.corridor,
        NARROW_CORRIDOR_M,
    )?;
    let narrow_plan = search::solve(&graph, &params);

    let needs_widening = narrow_plan.flags.iter().any(|f| {
        matches!(
            f,
            PlanFlag::ArrivalSocBelowWanted | PlanFlag::RunsOutOfCharge
        )
    });
    if !needs_widening {
        return Ok((narrow_plan, stats));
    }

    let (wide_graph, wide_stats) = corridor::assemble(
        pack,
        router,
        sites,
        veh,
        calib,
        &req.corridor,
        WIDE_CORRIDOR_M,
    )?;
    let wide_plan = search::solve(&wide_graph, &params);

    // Prefer whichever Plan has fewer flags; on a tie, prefer the widened
    // one (a wider corridor found no worse a Plan, so it's at least as
    // representative of what's actually reachable).
    if wide_plan.flags.len() <= narrow_plan.flags.len() {
        Ok((wide_plan, wide_stats))
    } else {
        Ok((narrow_plan, stats))
    }
}
