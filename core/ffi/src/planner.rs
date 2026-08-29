//! The one coarse `Planner` object Swift talks to (ADR 0004 point 3).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use energy::{edge_energy_wh, Conditions, EdgeInput};
use optimiser::{ChargerSite, PlanCache};
use packs::Rpack;
use routing::Router;

use crate::error::PlannerError;
use crate::mapping::{
    build_polyline, build_soc_curve, calibrate_stub, calibration_of, corridor_request_of,
    ffi_plan_alt_of, ffi_plan_of, map_assemble_error, plan_request_of, validate_cpack_format,
    vehicle_of,
};
use crate::types::{FfiLegInput, FfiPlan, FfiPlanRequest};

/// Interior mutability for the things that change after construction: the
/// loaded Charger sites, the cross-call corridor cache (issue #38), and the
/// cancel flag (ADR 0004 point 4). The open `Rpack` itself never mutates, so
/// it needs no lock.
#[derive(uniffi::Object)]
pub struct Planner {
    pack: Rpack,
    /// `Router`'s per-edge `from` table, precomputed once here: rebuilding
    /// it inside every `plan()` (an O(edges) pass allocating ~100 MB at
    /// eu-west scale) was most of a warm plan's cost (issue #50).
    from_of_edge: Vec<u32>,
    chargers: Mutex<Option<Vec<ChargerSite>>>,
    plan_cache: Mutex<PlanCache>,
    cancel_flag: AtomicBool,
}

#[uniffi::export]
impl Planner {
    /// Mmaps the Region Pack at `region_pack_path`. `Router` is deliberately
    /// NOT built here and cached: `routing::Router` borrows the pack, and
    /// pairing an owned `Rpack` with a `Router` borrowing it in the same
    /// struct is self-referential. Instead its one expensive input
    /// (`from_of_edge`) is precomputed here, and each `plan()` builds a
    /// throwaway `Router` borrowing both -- construction is then O(1).
    #[uniffi::constructor]
    pub fn new(region_pack_path: String) -> Result<Arc<Self>, PlannerError> {
        let pack = Rpack::open(&region_pack_path).map_err(|e| PlannerError::PackMissing {
            message: format!("failed to open region pack at {region_pack_path}: {e}"),
        })?;
        let from_of_edge = Router::precompute_from_of_edge(&pack);
        Ok(Arc::new(Self {
            pack,
            from_of_edge,
            chargers: Mutex::new(None),
            plan_cache: Mutex::new(PlanCache::new()),
            cancel_flag: AtomicBool::new(false),
        }))
    }

    /// Parses a Charger Pack (`format` must be `"cpack-1"`) and stores its
    /// sites, replacing any previously loaded set. Returns the site count.
    /// Clears the corridor cache (issue #38): the charger set is a key
    /// assembly input but, unlike the rest, isn't cheap to compare on every
    /// `plan()` call, so a fresh load just invalidates outright.
    pub fn load_chargers(&self, bytes: Vec<u8>, format: String) -> Result<u32, PlannerError> {
        validate_cpack_format(&format)?;
        let sites = optimiser::parse_cpack(&bytes).map_err(|e| PlannerError::InvalidRequest {
            message: e.to_string(),
        })?;
        let count = sites.len() as u32;
        *self.chargers.lock().expect("chargers mutex poisoned") = Some(sites);
        self.plan_cache
            .lock()
            .expect("plan cache mutex poisoned")
            .clear();
        Ok(count)
    }

    /// Runs the Charging Stop optimiser once (ADR 0006/0010) and maps the
    /// winning `Plan` (and its opt-in stop-free alternative, if any) to the
    /// wire shape, building the polyline and SoC curve from the pack's edge
    /// geometry along the way.
    pub fn plan(&self, request: FfiPlanRequest) -> Result<FfiPlan, PlannerError> {
        self.cancel_flag.store(false, Ordering::Relaxed);

        let sites_guard = self.chargers.lock().expect("chargers mutex poisoned");
        let sites = sites_guard
            .as_ref()
            .ok_or_else(|| PlannerError::PackMissing {
                message: "no chargers loaded: call load_chargers before plan".to_string(),
            })?;

        let vehicle = vehicle_of(request.vehicle);
        let calibration = calibration_of(&vehicle, request.reference_consumption_wh_per_km);
        let corridor = corridor_request_of(&request);
        let plan_request = plan_request_of(&request, corridor);

        let router = Router::with_from_of_edge(&self.pack, &self.from_of_edge);
        let mut cache = self.plan_cache.lock().expect("plan cache mutex poisoned");
        let (plan, _stats) = optimiser::plan_with_cache(
            &self.pack,
            &router,
            sites,
            &vehicle,
            &calibration,
            &plan_request,
            &mut cache,
            Some(&self.cancel_flag),
        )
        .map_err(map_assemble_error)?;

        let polyline = build_polyline(&self.pack, &plan);
        let soc_curve = build_soc_curve(&plan);
        let alternative = plan
            .alternative
            .as_ref()
            .map(|alt| ffi_plan_alt_of(alt, build_polyline(&self.pack, alt), build_soc_curve(alt)));

        Ok(ffi_plan_of(&plan, polyline, soc_curve, alternative))
    }

    /// Sets the cancel flag `plan()` polls (ADR 0004 point 4); `plan()`
    /// clears it again on its next call.
    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::Relaxed);
    }

    /// One `edge_energy_wh` call for UI what-ifs (ADR 0004 point 3):
    /// `delta_v` 0, `road_class` 0, the uncalibrated physics core.
    pub fn energy(&self, input: FfiLegInput) -> f64 {
        let vehicle = vehicle_of(input.vehicle);
        let calibration = energy::Calibration::default();
        let conditions = Conditions {
            temp_c: input.temp_c,
            headwind_ms: input.headwind_ms,
            altitude_m: 0.0,
        };
        edge_energy_wh(
            &vehicle,
            &calibration,
            &conditions,
            &EdgeInput {
                distance_m: input.distance_m,
                speed_kmh: input.speed_kmh,
                delta_v_kmh: 0.0,
                ascent_m: input.ascent_m,
                descent_m: input.descent_m,
                road_class: 0,
            },
        )
    }

    /// Stubbed until Trip Logs (M4).
    pub fn calibrate(&self) -> Result<(), PlannerError> {
        calibrate_stub()
    }
}
