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
    plan_with_cancel(pack, router, sites, veh, calib, req, None)
}

/// As [`plan`], but polls `cancel` (ADR 0004 point 4) between the two
/// heaviest stages -- corridor assembly (also polled internally, per pair,
/// in its parallel leg-evaluation loop) and `search::solve`. `solve` itself
/// is never polled: it runs in ~11ms against assembly's much larger share
/// of the ~1s plan budget, so a mid-search check would not meaningfully
/// shorten a cancelled call.
pub fn plan_with_cancel(
    pack: &Rpack,
    router: &Router,
    sites: &[ChargerSite],
    veh: &VehicleModel,
    calib: &Calibration,
    req: &PlanRequest,
    cancel: Option<&std::sync::atomic::AtomicBool>,
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
        cancel,
    )?;
    if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
        return Err(AssembleError::Cancelled);
    }
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
        cancel,
    )?;
    if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
        return Err(AssembleError::Cancelled);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use packs::{
        EdgeHot, GeomVertex, NodeRecord, RegionGraphModel, Rpack, SnapGridModel,
        CH_MIDDLE_NODE_NONE,
    };
    use pipeline::{write_rpack, PackMeta};
    use std::sync::atomic::AtomicBool;

    /// A minimal two-node, one-edge `.rpack` (mirrors `packs`' own
    /// `valid_model()` fixture), written to a tempfile so it can be opened
    /// with the real mmap `Rpack::open` path.
    fn tiny_pack() -> (tempfile::TempDir, std::path::PathBuf) {
        let model = RegionGraphModel {
            nodes: vec![
                NodeRecord {
                    lat: 49.5,
                    lon: 6.0,
                },
                NodeRecord {
                    lat: 49.6,
                    lon: 6.1,
                },
            ],
            csr_first_edge: vec![0, 1, 1],
            edges: vec![EdgeHot {
                target: 1,
                length_m: 10_000.0,
                speed_kmh: 100.0,
                ascent_m: 0.0,
                descent_m: 0.0,
                road_class: 0,
                _pad: [0; 3],
                ch_middle_node: CH_MIDDLE_NODE_NONE,
                geom_offset: 0,
                geom_count: 2,
            }],
            ch_order: vec![0, 1],
            geometry: vec![
                GeomVertex {
                    lat: 49.5,
                    lon: 6.0,
                    elev_m: 250,
                    _pad: 0,
                },
                GeomVertex {
                    lat: 49.6,
                    lon: 6.1,
                    elev_m: 260,
                    _pad: 0,
                },
            ],
            snap_grid: SnapGridModel {
                min_lat: 49.5,
                min_lon: 6.0,
                cell_size_deg: 0.5,
                n_rows: 1,
                n_cols: 1,
                cell_offsets: vec![0, 2],
                node_ids: vec![0, 1],
            },
        };
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("tiny.rpack");
        write_rpack(
            &model,
            &PackMeta {
                osm_snapshot_epoch: 0,
                region_id: 0,
                region_name: "tiny".to_string(),
            },
            &path,
        )
        .expect("write tiny.rpack");
        (dir, path)
    }

    #[test]
    fn plan_with_cancel_returns_cancelled_when_flag_is_preset() {
        let (_dir, path) = tiny_pack();
        let pack = Rpack::open(&path).expect("open tiny.rpack");
        let router = Router::new(&pack);
        let veh = VehicleModel::ioniq5_lr_2wd();
        let calib = Calibration::default();
        let req = PlanRequest {
            corridor: CorridorRequest {
                origin: (49.5, 6.0),
                waypoints: vec![],
                dest: (49.6, 6.1),
                temp_c: 20.0,
                headwind_ms: 0.0,
            },
            depart_soc: 0.9,
            arrival_min_soc: 0.1,
            charger_arrival_min_soc: 0.1,
            charger_max_soc: 0.8,
            stops_bias: 1.0,
            battery_warmth: 1.0,
            offer_stop_free_alternative: false,
        };
        let cancel = AtomicBool::new(true);

        let result = plan_with_cancel(&pack, &router, &[], &veh, &calib, &req, Some(&cancel));

        assert!(matches!(result, Err(AssembleError::Cancelled)));
    }
}
