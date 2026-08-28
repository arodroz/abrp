//! Top-level Charging Stop optimiser entry point (wayfinder #33): assembles
//! the corridor, runs the search, and owns the corridor-widening rung
//! (ADR 0006 point 1) that sits outside `search::solve`'s own SoC
//! relaxation ladder (ADR 0006 point 4). NOT part of the crate yet -- see
//! the commented `pub mod plan_api;` in `lib.rs`; it compiles once
//! `search::solve` exists (wayfinder #33's search half).

use std::sync::Arc;

use energy::{Calibration, VehicleModel};
use packs::Rpack;
use routing::Router;

use crate::corridor::{self, AssembleError, AssemblyStats, CorridorRequest};
use crate::search;
use crate::types::{CandidateGraph, ChargerSite, Plan, PlanFlag, SearchParams};

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
    plan_with_cache(
        pack,
        router,
        sites,
        veh,
        calib,
        req,
        &mut PlanCache::new(),
        cancel,
    )
}

/// As [`plan_with_cancel`], but skips corridor assembly on a repeat call
/// whose route-shaping inputs are unchanged (issue #38): a slider replan
/// that only touches SoC/bias fields goes straight to `search::solve`
/// against the cached `CandidateGraph`, since assembly (not solve) is
/// ~975ms of a warm plan's ~1s cost. `cache` is caller-owned so it survives
/// across calls; a throwaway one (as `plan`/`plan_with_cancel` pass) makes
/// this behave exactly like an uncached assembly every time.
#[allow(clippy::too_many_arguments)]
pub fn plan_with_cache(
    pack: &Rpack,
    router: &Router,
    sites: &[ChargerSite],
    veh: &VehicleModel,
    calib: &Calibration,
    req: &PlanRequest,
    cache: &mut PlanCache,
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
    let key = PlanKey::new(&req.corridor, veh, calib);

    let (graph, stats) = assemble_cached(
        &mut cache.narrow,
        &key,
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

    let (wide_graph, wide_stats) = assemble_cached(
        &mut cache.wide,
        &key,
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

// ---------------------------------------------------------------------
// Cross-call corridor cache (issue #38)
// ---------------------------------------------------------------------

/// A bit-exact snapshot of every input `corridor::assemble` reads, verified
/// against its signature: `origin`/`dest`/`temp_c`/`headwind_ms` (via
/// `CorridorRequest`), the Vehicle Model and Calibration scalars `eval_leg`
/// and the full-battery feasibility pre-filter use, and each waypoint's
/// `lat`/`lon`. Also carries each waypoint's `depart_soc_override`: assembly
/// itself never reads it, but it rides untouched into the cached
/// `CandidateGraph::waypoints`, which `search::solve` (not assembly) later
/// reads it back out of -- so it must invalidate the cache like any other
/// input, even though it is otherwise an SoC field.
///
/// Deliberately excludes `depart_soc`, `arrival_min_soc`,
/// `charger_arrival_min_soc`, `charger_max_soc`, `stops_bias`,
/// `battery_warmth`, and `offer_stop_free_alternative`: none of those reach
/// `corridor::assemble`, only `search::solve` via `SearchParams`.
///
/// Floats compare bit-for-bit via `to_bits()` so `Eq` can be derived. The
/// charger set is deliberately not part of this key -- `Planner::
/// load_chargers` clears the cache instead, since comparing the whole site
/// list on every call would cost more than the assembly it's saving.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PlanKey {
    origin: (u64, u64),
    waypoints: Vec<(u64, u64, Option<u64>)>,
    dest: (u64, u64),
    temp_c: u64,
    headwind_ms: u64,
    vehicle: VehicleKeyBits,
    calib: CalibKeyBits,
}

/// `VehicleModel`'s scalars, bit-exact; `warm_curve`/`cold_curve` are
/// `&'static` tables (always one of the fixed statics in `energy::vehicle`),
/// so their address alone identifies which curve is in play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VehicleKeyBits {
    usable_capacity_kwh: u64,
    mass_kg: u64,
    cda_m2: u64,
    crr: u64,
    eta_drive: u64,
    eta_regen: u64,
    p_aux_w: u64,
    warm_curve_ptr: usize,
    cold_curve_ptr: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CalibKeyBits {
    k_aero: u64,
    k_roll: u64,
    k_hvac: u64,
}

impl PlanKey {
    fn new(corridor: &CorridorRequest, veh: &VehicleModel, calib: &Calibration) -> Self {
        Self {
            origin: (corridor.origin.0.to_bits(), corridor.origin.1.to_bits()),
            waypoints: corridor
                .waypoints
                .iter()
                .map(|w| {
                    (
                        w.lat.to_bits(),
                        w.lon.to_bits(),
                        w.depart_soc_override.map(f64::to_bits),
                    )
                })
                .collect(),
            dest: (corridor.dest.0.to_bits(), corridor.dest.1.to_bits()),
            temp_c: corridor.temp_c.to_bits(),
            headwind_ms: corridor.headwind_ms.to_bits(),
            vehicle: VehicleKeyBits {
                usable_capacity_kwh: veh.usable_capacity_kwh.to_bits(),
                mass_kg: veh.mass_kg.to_bits(),
                cda_m2: veh.cda_m2.to_bits(),
                crr: veh.crr.to_bits(),
                eta_drive: veh.eta_drive.to_bits(),
                eta_regen: veh.eta_regen.to_bits(),
                p_aux_w: veh.p_aux_w.to_bits(),
                warm_curve_ptr: veh.warm_curve.as_ptr() as usize,
                cold_curve_ptr: veh.cold_curve.as_ptr() as usize,
            },
            calib: CalibKeyBits {
                k_aero: calib.k_aero.to_bits(),
                k_roll: calib.k_roll.to_bits(),
                k_hvac: calib.k_hvac.to_bits(),
            },
        }
    }
}

type CacheSlot = Option<(PlanKey, Arc<CandidateGraph>, AssemblyStats)>;

/// One assembled `CandidateGraph` per corridor width (ADR 0006 point 1 only
/// ever widens once, narrow then wide, so two slots are the whole shape).
/// No TTL, no LRU: each slot just holds whatever was assembled at that
/// width most recently.
#[derive(Debug, Default)]
pub struct PlanCache {
    narrow: CacheSlot,
    wide: CacheSlot,
}

impl PlanCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drops both slots (call when the charger set changes: it's a key
    /// input to assembly but not part of `PlanKey`, see there).
    pub fn clear(&mut self) {
        self.narrow = None;
        self.wide = None;
    }
}

/// Returns `slot`'s graph if its key matches, else assembles fresh and
/// stores it. A cancelled assembly may be incomplete (workers in the
/// parallel leg-evaluation loop stop early, per pair, once `cancel` is
/// observed -- see `corridor::assemble`), so it is returned to the caller
/// but never written into `slot`.
#[allow(clippy::too_many_arguments)]
fn assemble_cached(
    slot: &mut CacheSlot,
    key: &PlanKey,
    pack: &Rpack,
    router: &Router,
    sites: &[ChargerSite],
    veh: &VehicleModel,
    calib: &Calibration,
    corridor_req: &CorridorRequest,
    corridor_m: f64,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(Arc<CandidateGraph>, AssemblyStats), AssembleError> {
    if let Some((cached_key, graph, stats)) = slot {
        if cached_key == key {
            return Ok((graph.clone(), *stats));
        }
    }
    let (graph, stats) = corridor::assemble(
        pack,
        router,
        sites,
        veh,
        calib,
        corridor_req,
        corridor_m,
        cancel,
    )?;
    let graph = Arc::new(graph);
    if !cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
        *slot = Some((key.clone(), graph.clone(), stats));
    }
    Ok((graph, stats))
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

    fn base_req() -> PlanRequest {
        PlanRequest {
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
        }
    }

    #[test]
    fn plan_with_cache_reuses_the_cached_graph_on_a_same_key_repeat_call() {
        let (_dir, path) = tiny_pack();
        let pack = Rpack::open(&path).expect("open tiny.rpack");
        let router = Router::new(&pack);
        let veh = VehicleModel::ioniq5_lr_2wd();
        let calib = Calibration::default();
        let mut cache = PlanCache::new();
        let req = base_req();

        plan_with_cache(&pack, &router, &[], &veh, &calib, &req, &mut cache, None)
            .expect("first plan_with_cache");
        let graph1 = cache
            .narrow
            .as_ref()
            .expect("narrow slot populated")
            .1
            .clone();

        plan_with_cache(&pack, &router, &[], &veh, &calib, &req, &mut cache, None)
            .expect("second plan_with_cache");
        let graph2 = cache
            .narrow
            .as_ref()
            .expect("narrow slot populated")
            .1
            .clone();

        assert!(
            Arc::ptr_eq(&graph1, &graph2),
            "same-key call should reuse the cached graph"
        );
    }

    #[test]
    fn plan_with_cache_hits_on_a_depart_soc_only_change_and_still_plans_correctly() {
        let (_dir, path) = tiny_pack();
        let pack = Rpack::open(&path).expect("open tiny.rpack");
        let router = Router::new(&pack);
        let veh = VehicleModel::ioniq5_lr_2wd();
        let calib = Calibration::default();
        let mut cache = PlanCache::new();

        let mut req = base_req();
        req.depart_soc = 0.9;
        let (plan_a, _) =
            plan_with_cache(&pack, &router, &[], &veh, &calib, &req, &mut cache, None)
                .expect("first plan_with_cache");
        let graph1 = cache
            .narrow
            .as_ref()
            .expect("narrow slot populated")
            .1
            .clone();

        req.depart_soc = 0.5;
        let (plan_b, _) =
            plan_with_cache(&pack, &router, &[], &veh, &calib, &req, &mut cache, None)
                .expect("second plan_with_cache");
        let graph2 = cache
            .narrow
            .as_ref()
            .expect("narrow slot populated")
            .1
            .clone();

        assert!(
            Arc::ptr_eq(&graph1, &graph2),
            "a depart_soc-only change should hit the cache"
        );

        // Same route, no charging stop: arrival SoC should shift by exactly
        // the depart_soc delta, proving the reused graph still plans
        // correctly for the new SoC.
        let arrival_a = plan_a.legs.last().expect("one leg").arrival_soc;
        let arrival_b = plan_b.legs.last().expect("one leg").arrival_soc;
        assert!(
            (arrival_b - arrival_a - (req.depart_soc - 0.9)).abs() < 1e-6,
            "arrival soc should track the new depart_soc exactly: {arrival_a} vs {arrival_b}"
        );
    }

    #[test]
    fn plan_with_cache_misses_when_conditions_change() {
        let (_dir, path) = tiny_pack();
        let pack = Rpack::open(&path).expect("open tiny.rpack");
        let router = Router::new(&pack);
        let veh = VehicleModel::ioniq5_lr_2wd();
        let calib = Calibration::default();
        let mut cache = PlanCache::new();

        let mut req = base_req();
        plan_with_cache(&pack, &router, &[], &veh, &calib, &req, &mut cache, None)
            .expect("first plan_with_cache");
        let graph1 = cache
            .narrow
            .as_ref()
            .expect("narrow slot populated")
            .1
            .clone();

        req.corridor.temp_c = -5.0;
        plan_with_cache(&pack, &router, &[], &veh, &calib, &req, &mut cache, None)
            .expect("second plan_with_cache");
        let graph2 = cache
            .narrow
            .as_ref()
            .expect("narrow slot populated")
            .1
            .clone();

        assert!(
            !Arc::ptr_eq(&graph1, &graph2),
            "a conditions change should miss the cache and reassemble"
        );
    }

    #[test]
    fn plan_with_cache_misses_after_clear() {
        let (_dir, path) = tiny_pack();
        let pack = Rpack::open(&path).expect("open tiny.rpack");
        let router = Router::new(&pack);
        let veh = VehicleModel::ioniq5_lr_2wd();
        let calib = Calibration::default();
        let mut cache = PlanCache::new();
        let req = base_req();

        plan_with_cache(&pack, &router, &[], &veh, &calib, &req, &mut cache, None)
            .expect("first plan_with_cache");
        let graph1 = cache
            .narrow
            .as_ref()
            .expect("narrow slot populated")
            .1
            .clone();

        cache.clear();
        plan_with_cache(&pack, &router, &[], &veh, &calib, &req, &mut cache, None)
            .expect("second plan_with_cache");
        let graph2 = cache
            .narrow
            .as_ref()
            .expect("narrow slot populated")
            .1
            .clone();

        assert!(
            !Arc::ptr_eq(&graph1, &graph2),
            "a cleared cache should miss on the next call"
        );
    }
}
