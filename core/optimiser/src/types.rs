//! Shared contracts of the Charging Stop optimiser (wayfinder #33): the
//! candidate graph the corridor layer assembles and the pure search
//! consumes, and the Plan the search produces. ADR 0006 as amended by
//! ADR 0010.

use energy::{Calibration, VehicleModel};

/// The per-Leg Speed Cap alternatives (ADR 0010 point 1): every candidate
/// leg is evaluated once per entry, index-aligned with
/// `CandidateLeg::evals`. `None` = uncapped.
pub const SPEED_CAPS_KMH: [Option<f64>; 4] = [None, Some(110.0), Some(100.0), Some(90.0)];

/// One DC charging site from the Charger Pack (already filtered to
/// >= 50 kW CCS by the pack build). `id` is the cpack record id.
#[derive(Debug, Clone, PartialEq)]
pub struct ChargerSite {
    pub id: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub power_kw: f64,
}

/// A user waypoint (ADR 0010 point 4). The optional override pins the
/// minimum SoC a label may leave this waypoint with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaypointSpec {
    pub lat: f64,
    pub lon: f64,
    pub depart_soc_override: Option<f64>,
}

/// One (time, energy) outcome of driving a candidate leg under one Speed
/// Cap, index-aligned with [`SPEED_CAPS_KMH`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LegEval {
    pub time_s: f64,
    pub energy_wh: f64,
}

/// A directed edge of the candidate graph. `route_edges` are pack edge ids
/// of the underlying road route (kept for geometry/SoC-curve extraction
/// downstream; empty in synthetic tests).
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateLeg {
    pub to: u32,
    pub dist_m: f64,
    pub evals: [LegEval; 4],
    pub route_edges: Vec<u32>,
}

/// What a candidate-graph node is. `Charger`/`Waypoint` carry indices into
/// `CandidateGraph::sites` / `::waypoints`. A waypoint that physically is a
/// charging site carries both (`Waypoint { charger: Some(site) }`), which is
/// how "charging at a waypoint falls out naturally" (ADR 0010 point 4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NodeKind {
    Origin,
    Charger { site: u32 },
    Waypoint { wp: u32, charger: Option<u32> },
    Dest,
}

/// One node of the candidate graph. `segment` is the waypoint-interval the
/// node lies in: 0 = origin→wp0, 1 = wp0→wp1, …, `n_waypoints` = last
/// wp→dest. Waypoint node `wp` is the articulation point between segments
/// `wp` and `wp + 1` (its own `segment` is `wp + 1`, the one it opens);
/// legs never cross a segment boundary except by ending at the waypoint
/// node, which is what makes skipping a waypoint structurally impossible.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateNode {
    pub kind: NodeKind,
    pub segment: u32,
    pub out: Vec<CandidateLeg>,
}

/// The assembled search input: origin, per-segment Charger candidates,
/// waypoints, destination, with every leg pre-evaluated per Speed Cap.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateGraph {
    pub nodes: Vec<CandidateNode>,
    pub sites: Vec<ChargerSite>,
    pub waypoints: Vec<WaypointSpec>,
    /// Index of the origin node in `nodes` (by convention 0).
    pub origin: u32,
    /// Index of the destination node in `nodes`.
    pub dest: u32,
}

/// Search-time parameters (SoC fractions in 0..1).
#[derive(Debug, Clone, Copy)]
pub struct SearchParams<'a> {
    pub vehicle: &'a VehicleModel,
    pub calibration: &'a Calibration,
    pub depart_soc: f64,
    pub arrival_min_soc: f64,
    pub charger_arrival_min_soc: f64,
    pub charger_max_soc: f64,
    /// Stops Bias factor multiplying the 300 s per-stop overhead
    /// (ADR 0006 point 3): few-long 3.0, quickest 1.0, many-short 0.33.
    pub stops_bias: f64,
    /// 0 = cold Charging Curve, 1 = warm (ADR 0003; caller decides).
    pub battery_warmth: f64,
    /// When true and the winning Plan's only Charging Stop is a micro-stop
    /// (charge under 10 min), also produce the stop-free alternative Plan
    /// (ADR 0010 point 5).
    pub offer_stop_free_alternative: bool,
}

/// Why a Plan or Leg is flagged (ADR 0006 point 4: relaxation never errors).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanFlag {
    /// Some Leg arrives below the wanted floor (which relaxation step is
    /// recorded on the Leg itself).
    ArrivalSocBelowWanted,
    /// Even fully relaxed no feasible Plan exists; the returned Plan runs
    /// out of charge and the Leg that first reaches 0 % is flagged.
    RunsOutOfCharge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegFlag {
    ArrivalSocBelowWanted,
    ReachesZeroSoc,
}

/// One endpoint of a Plan Leg, resolved to a human-referenceable thing.
#[derive(Debug, Clone, PartialEq)]
pub enum Endpoint {
    Origin,
    Charger { site: u32 },
    Waypoint { wp: u32 },
    Dest,
}

/// One driven Leg of the Plan.
#[derive(Debug, Clone, PartialEq)]
pub struct PlanLeg {
    pub from: Endpoint,
    pub to: Endpoint,
    pub drive_s: f64,
    pub dist_m: f64,
    pub energy_wh: f64,
    /// The chosen Speed Cap, `None` when uncapped (ADR 0010 point 1).
    pub speed_cap_kmh: Option<f64>,
    pub depart_soc: f64,
    pub arrival_soc: f64,
    pub flags: Vec<LegFlag>,
    /// Pack edge ids of the road route under this Leg (empty in synthetic
    /// search tests).
    pub route_edges: Vec<u32>,
}

/// One Charging Stop of the Plan.
#[derive(Debug, Clone, PartialEq)]
pub struct Stop {
    /// Index into the graph's `sites`.
    pub site: u32,
    pub arrival_soc: f64,
    pub depart_soc: f64,
    pub charge_s: f64,
}

/// The Plan contract (ADR 0006/0010). Always returned when the road graph
/// connects origin and destination; infeasibility surfaces as flags.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub legs: Vec<PlanLeg>,
    pub stops: Vec<Stop>,
    /// The candidate graph's Charger sites, so the Plan is self-contained:
    /// `Stop::site` and `Endpoint::Charger::site` index into THIS list (the
    /// corridor-filtered subset), not the full Charger Pack.
    pub sites: Vec<ChargerSite>,
    pub drive_time_s: f64,
    pub charge_time_s: f64,
    pub total_time_s: f64,
    pub total_dist_m: f64,
    pub flags: Vec<PlanFlag>,
    /// The opt-in stop-free alternative (ADR 0010 point 5), present only
    /// when requested and the winning Plan's single stop is a micro-stop.
    pub alternative: Option<Box<Plan>>,
}
