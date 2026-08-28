//! Label-setting search over a `CandidateGraph` (wayfinder #33), ADR 0006
//! as amended by ADR 0010. State is `(node, SoC bucket)`; waypoint order is
//! enforced structurally by the graph (see `types.rs`), so it is not part
//! of the label. Ties break on fewer Charging Stops.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use crate::types::{
    CandidateGraph, Endpoint, LegFlag, NodeKind, Plan, PlanFlag, PlanLeg, SearchParams, Stop,
    SPEED_CAPS_KMH,
};

/// SoC bucket width (ADR 0006 point 2 / vertical slice #15): 2 % per bucket.
fn soc_bucket(soc: f64) -> i32 {
    (soc * 50.0).floor() as i32
}

/// A depart target this close to the arrival SoC is not a real charging
/// decision -- the vertical slice found these create phantom, zero-penalty
/// stops that just happen to sit on the winning path (RESULTS.md finding
/// 4). Below this bar a target is dropped rather than clamped, so no label
/// -- and therefore no Stop -- is ever created for it; a genuine bypass is
/// covered by the pass-through branch below.
const MIN_CHARGE_SOC: f64 = 0.02;

type Key = (u32, i32);

/// `time_ms` is an i64 heap key (float time isn't `Ord`); `stops` breaks
/// ties in both the heap order and the "is this better" test at a state,
/// per ADR 0006 point 3 ("ties -> fewer stops").
#[derive(Clone, Copy, PartialEq, Eq)]
struct HeapItem {
    time_ms: i64,
    stops: u32,
    node: u32,
    bucket: i32,
}

impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .time_ms
            .cmp(&self.time_ms)
            .then(other.stops.cmp(&self.stops))
    }
}

impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// How a label reached its (node, bucket) state: the edge taken and the
/// SoC decision made at the previous node. `child_soc` is the exact
/// (unbucketed) SoC on arrival at *this* label's own node, kept alongside
/// the bucket key so Plan assembly never has to re-derive it.
struct CameFrom {
    prev_key: Key,
    leg_idx: usize,
    cap_idx: usize,
    /// SoC leaving `prev_key`'s node feeding this edge (after any charging
    /// or pass-through decision there).
    prev_depart_soc: f64,
    /// Charge time spent at `prev_key`'s node immediately before this edge;
    /// 0.0 for a pass-through or a non-charger node.
    prev_charge_s: f64,
    child_soc: f64,
}

/// Lexicographic (time, stops) "strictly better" test, with a time epsilon
/// wide enough to treat near-equal float times as ties so the stops
/// tie-break actually fires.
fn better(new_time: f64, new_stops: u32, best_time: f64, best_stops: u32) -> bool {
    if new_time < best_time - 1e-6 {
        true
    } else if new_time < best_time + 1e-6 {
        new_stops < best_stops
    } else {
        false
    }
}

struct SearchState {
    best: HashMap<Key, (f64, u32, f64)>, // (time_s, stops, exact_soc)
    came_from: HashMap<Key, CameFrom>,
    heap: BinaryHeap<HeapItem>,
}

impl SearchState {
    fn relax(&mut self, to_key: Key, new_time: f64, new_stops: u32, new_soc: f64, cf: CameFrom) {
        let is_better = match self.best.get(&to_key) {
            None => true,
            Some(&(bt, bs, _)) => better(new_time, new_stops, bt, bs),
        };
        if is_better {
            self.best.insert(to_key, (new_time, new_stops, new_soc));
            self.came_from.insert(to_key, cf);
            self.heap.push(HeapItem {
                // Truncate, don't round: the staleness check on pop relies
                // on the reconstructed time never overshooting the exact
                // `best` it was pushed for.
                time_ms: (new_time * 1000.0) as i64,
                stops: new_stops,
                node: to_key.0,
                bucket: to_key.1,
            });
        }
    }
}

struct SearchResult {
    came_from: HashMap<Key, CameFrom>,
    final_key: Key,
}

fn charger_site_of(kind: &NodeKind) -> Option<u32> {
    match kind {
        NodeKind::Charger { site } => Some(*site),
        NodeKind::Waypoint {
            charger: Some(site),
            ..
        } => Some(*site),
        _ => None,
    }
}

fn waypoint_override(graph: &CandidateGraph, kind: &NodeKind) -> Option<f64> {
    match kind {
        NodeKind::Waypoint { wp, .. } => graph.waypoints[*wp as usize].depart_soc_override,
        _ => None,
    }
}

/// The floor a label must clear on arrival, per the leg's *destination*
/// node kind: the Destination Arrival SoC at Dest, the Charger Arrival SoC
/// at a Charger (or a Charger-carrying Waypoint), and a hard 0 % floor at a
/// plain Waypoint (it is not a Charging Stop, so neither relaxation rung
/// applies to it -- it only ever enforces "never negative"). `allow_negative`
/// (the last-resort fallback, ADR 0006 point 4) lifts every floor.
fn required_min(kind: &NodeKind, charger_min: f64, dest_min: f64, allow_negative: bool) -> f64 {
    if allow_negative {
        return f64::NEG_INFINITY;
    }
    match kind {
        NodeKind::Dest => dest_min,
        NodeKind::Waypoint { .. } => 0.0,
        NodeKind::Charger { .. } | NodeKind::Origin => charger_min,
    }
}

/// Memoised charge durations, one cumulative table per distinct Charger
/// power. `energy::charge_duration_s` integrates the Charging Curve
/// numerically, which is far too slow to call once per (label × depart
/// target) -- the search makes tens of thousands of such calls. Charge time
/// is an integral over SoC, so it is additive: one cumulative table
/// `T[k] = seconds to charge 0 → k·0.5 %` per (power, warmth) answers any
/// `(from, to)` as `T(to) − T(from)`, linearly interpolated between knots
/// (the curve is piecewise-linear and smooth at this resolution; the
/// interpolation error is well under a second). Vehicle and warmth are
/// fixed per `solve`, so tables are keyed by power alone.
struct ChargeTables {
    per_power: HashMap<u64, Vec<f64>>,
}

/// Table knot spacing in SoC (0.5 %).
const CHARGE_TABLE_STEP: f64 = 0.005;

impl ChargeTables {
    fn new() -> Self {
        ChargeTables {
            per_power: HashMap::new(),
        }
    }

    fn duration_s(
        &mut self,
        vehicle: &energy::VehicleModel,
        battery_warmth: f64,
        from_soc: f64,
        to_soc: f64,
        power_kw: f64,
    ) -> f64 {
        let table = self.per_power.entry(power_kw.to_bits()).or_insert_with(|| {
            let n = (1.0 / CHARGE_TABLE_STEP).round() as usize;
            let mut t = Vec::with_capacity(n + 1);
            t.push(0.0);
            for k in 1..=n {
                let slice = energy::charge_duration_s(
                    vehicle,
                    battery_warmth,
                    (k - 1) as f64 * CHARGE_TABLE_STEP,
                    k as f64 * CHARGE_TABLE_STEP,
                    power_kw,
                    false,
                );
                t.push(t[k - 1] + slice);
            }
            t
        });
        let interp = |soc: f64| -> f64 {
            let x = (soc.clamp(0.0, 1.0)) / CHARGE_TABLE_STEP;
            let k = (x.floor() as usize).min(table.len() - 2);
            let f = x - k as f64;
            table[k] + f * (table[k + 1] - table[k])
        };
        (interp(to_soc) - interp(from_soc)).max(0.0)
    }
}

/// One run of the label-setting search at fixed floors. `disable_charging`
/// turns every charger-capable node travel-only, for the stop-free
/// alternative (ADR 0010 point 5).
#[allow(clippy::too_many_arguments)]
fn run_search(
    graph: &CandidateGraph,
    params: &SearchParams,
    battery_wh: f64,
    charger_min: f64,
    dest_min: f64,
    allow_negative: bool,
    disable_charging: bool,
    charge_tables: &mut ChargeTables,
) -> Option<SearchResult> {
    let stop_overhead_s = 300.0 * params.stops_bias;
    let start_key: Key = (graph.origin, soc_bucket(params.depart_soc));

    let mut state = SearchState {
        best: HashMap::new(),
        came_from: HashMap::new(),
        heap: BinaryHeap::new(),
    };
    state.best.insert(start_key, (0.0, 0, params.depart_soc));
    state.heap.push(HeapItem {
        time_ms: 0,
        stops: 0,
        node: start_key.0,
        bucket: start_key.1,
    });

    while let Some(item) = state.heap.pop() {
        let key = (item.node, item.bucket);
        let Some(&(best_time, best_stops, soc)) = state.best.get(&key) else {
            continue;
        };
        let item_time = item.time_ms as f64 / 1000.0;
        // One-directional: truncation means item_time never overshoots the
        // exact time it was pushed for, so only a genuinely superseded
        // (worse) entry compares as stale here.
        if item_time > best_time + 1e-6 || item.stops > best_stops {
            continue; // stale heap entry, superseded since it was pushed
        }
        if item.node == graph.dest {
            return Some(SearchResult {
                came_from: state.came_from,
                final_key: key,
            });
        }

        let kind = graph.nodes[item.node as usize].kind;
        let chargeable = charger_site_of(&kind).is_some() && !disable_charging;
        // The departure-SoC override is a pinned constraint (ADR 0010 point
        // 4), not one of the two SoC floors the relaxation ladder touches --
        // except in the last-resort fallback (`allow_negative`), whose whole
        // point is to always find *some* path (ADR 0006 point 4: never
        // error), so it ignores the override too rather than getting stuck.
        let override_soc = if allow_negative {
            None
        } else {
            waypoint_override(graph, &kind)
        };

        if !chargeable {
            // A Waypoint with no way to charge (either it truly has none, or
            // charging is disabled for this run) pins a hard floor on the
            // SoC it may be left with (ADR 0010 point 4): below the
            // override, the label is dead here.
            if let Some(o) = override_soc {
                if soc + 1e-9 < o {
                    continue;
                }
            }
        }

        for (leg_idx, leg) in graph.nodes[item.node as usize].out.iter().enumerate() {
            let to_kind = graph.nodes[leg.to as usize].kind;
            let req_min = required_min(&to_kind, charger_min, dest_min, allow_negative);

            for (cap_idx, eval) in leg.evals.iter().enumerate() {
                if !chargeable {
                    let soc_after = soc - eval.energy_wh / battery_wh;
                    if soc_after + 1e-9 < req_min {
                        continue;
                    }
                    let stored_soc = if allow_negative {
                        soc_after.max(-1.0)
                    } else {
                        soc_after
                    };
                    state.relax(
                        (leg.to, soc_bucket(stored_soc)),
                        item_time + eval.time_s,
                        item.stops,
                        stored_soc,
                        CameFrom {
                            prev_key: key,
                            leg_idx,
                            cap_idx,
                            prev_depart_soc: soc,
                            prev_charge_s: 0.0,
                            child_soc: stored_soc,
                        },
                    );
                    continue;
                }

                // Pass-through: traverse the Charger without charging, so a
                // label can reach a better Charger beyond it (ADR 0006
                // point 2 / this ticket's spec point 3).
                let pass_through_allowed = override_soc.is_none_or(|o| soc + 1e-9 >= o);
                if pass_through_allowed {
                    let soc_after = soc - eval.energy_wh / battery_wh;
                    if soc_after + 1e-9 >= req_min {
                        let stored_soc = if allow_negative {
                            soc_after.max(-1.0)
                        } else {
                            soc_after
                        };
                        state.relax(
                            (leg.to, soc_bucket(stored_soc)),
                            item_time + eval.time_s,
                            item.stops,
                            stored_soc,
                            CameFrom {
                                prev_key: key,
                                leg_idx,
                                cap_idx,
                                prev_depart_soc: soc,
                                prev_charge_s: 0.0,
                                child_soc: stored_soc,
                            },
                        );
                    }
                }

                // Discrete depart-SoC branches (ADR 0006 point 2).
                let need_for_leg =
                    (eval.energy_wh / battery_wh + req_min + 0.05).min(params.charger_max_soc);
                let mut targets = [0.6, 0.7, 0.8, params.charger_max_soc, need_for_leg];
                targets.sort_by(|a, b| a.partial_cmp(b).unwrap());
                let power_kw = graph.sites[charger_site_of(&kind).unwrap() as usize].power_kw;

                let mut prev_target = f64::NAN;
                for &target in &targets {
                    if (target - prev_target).abs() < 1e-6 {
                        continue; // dedup
                    }
                    prev_target = target;
                    if target <= soc + MIN_CHARGE_SOC - 1e-9
                        || target > params.charger_max_soc + 1e-9
                    {
                        continue;
                    }
                    if let Some(o) = override_soc {
                        if target + 1e-9 < o {
                            continue;
                        }
                    }
                    let charge_s = charge_tables.duration_s(
                        params.vehicle,
                        params.battery_warmth,
                        soc,
                        target,
                        power_kw,
                    );
                    let soc_after = target - eval.energy_wh / battery_wh;
                    if soc_after + 1e-9 < req_min {
                        continue;
                    }
                    // Minimum-useful-charge ramp penalty (ADR 0010 point 3):
                    // 0 s at >= 10 min, up to +300 s at 0 min. Soft -- it
                    // only ever changes which stop wins, never whether one
                    // is possible.
                    let ramp_s = if charge_s < 600.0 {
                        (600.0 - charge_s) / 600.0 * 300.0
                    } else {
                        0.0
                    };
                    let new_time = item_time + charge_s + stop_overhead_s + ramp_s + eval.time_s;
                    let stored_soc = if allow_negative {
                        soc_after.max(-1.0)
                    } else {
                        soc_after
                    };
                    state.relax(
                        (leg.to, soc_bucket(stored_soc)),
                        new_time,
                        item.stops + 1,
                        stored_soc,
                        CameFrom {
                            prev_key: key,
                            leg_idx,
                            cap_idx,
                            prev_depart_soc: target,
                            prev_charge_s: charge_s,
                            child_soc: stored_soc,
                        },
                    );
                }
            }
        }
    }
    None
}

fn endpoint_of(kind: &NodeKind) -> Endpoint {
    match kind {
        NodeKind::Origin => Endpoint::Origin,
        NodeKind::Charger { site } => Endpoint::Charger { site: *site },
        NodeKind::Waypoint { wp, .. } => Endpoint::Waypoint { wp: *wp },
        NodeKind::Dest => Endpoint::Dest,
    }
}

fn reconstruct_chain(
    came_from: &HashMap<Key, CameFrom>,
    origin_key: Key,
    final_key: Key,
) -> Vec<Key> {
    let mut chain = vec![final_key];
    let mut cur = final_key;
    while cur != origin_key {
        let cf = &came_from[&cur];
        cur = cf.prev_key;
        chain.push(cur);
    }
    chain.reverse();
    chain
}

fn build_plan(
    graph: &CandidateGraph,
    params: &SearchParams,
    result: &SearchResult,
    chain: &[Key],
) -> Plan {
    let mut legs = Vec::new();
    let mut stops = Vec::new();
    let mut drive_time_s = 0.0;
    let mut charge_time_s = 0.0;
    let mut total_dist_m = 0.0;

    for w in chain.windows(2) {
        let (from_key, to_key) = (w[0], w[1]);
        let cf = &result.came_from[&to_key];
        let from_kind = graph.nodes[from_key.0 as usize].kind;
        let leg = &graph.nodes[from_key.0 as usize].out[cf.leg_idx];
        let eval = leg.evals[cf.cap_idx];

        drive_time_s += eval.time_s;
        total_dist_m += leg.dist_m;
        charge_time_s += cf.prev_charge_s;

        if cf.prev_charge_s > 0.0 {
            let site =
                charger_site_of(&from_kind).expect("a charge decision implies a chargeable node");
            let arrival_soc = if from_key.0 == graph.origin {
                params.depart_soc
            } else {
                result.came_from[&from_key].child_soc
            };
            stops.push(Stop {
                site,
                arrival_soc,
                depart_soc: cf.prev_depart_soc,
                charge_s: cf.prev_charge_s,
            });
        }

        legs.push(PlanLeg {
            from: endpoint_of(&from_kind),
            to: endpoint_of(&graph.nodes[to_key.0 as usize].kind),
            drive_s: eval.time_s,
            dist_m: leg.dist_m,
            energy_wh: eval.energy_wh,
            speed_cap_kmh: SPEED_CAPS_KMH[cf.cap_idx],
            depart_soc: cf.prev_depart_soc,
            arrival_soc: cf.child_soc,
            flags: Vec::new(),
            route_edges: leg.route_edges.clone(),
        });
    }

    let total_time_s = drive_time_s + charge_time_s;
    Plan {
        legs,
        stops,
        sites: graph.sites.clone(),
        drive_time_s,
        charge_time_s,
        total_time_s,
        total_dist_m,
        flags: Vec::new(),
        alternative: None,
    }
}

/// Flags a Plan and its Legs against the *originally wanted* floors (ADR
/// 0006 point 4): a no-op when the winning run used those floors already
/// (rung 0), so this can run unconditionally after every successful solve
/// rather than needing the caller to track which rung won.
fn apply_relaxation_flags(plan: &mut Plan, params: &SearchParams) {
    let mut any = false;
    for leg in &mut plan.legs {
        let wanted_floor = match leg.to {
            Endpoint::Dest => params.arrival_min_soc,
            Endpoint::Charger { .. } => params.charger_arrival_min_soc,
            Endpoint::Waypoint { .. } | Endpoint::Origin => 0.0,
        };
        if leg.arrival_soc + 1e-9 < wanted_floor {
            leg.flags.push(LegFlag::ArrivalSocBelowWanted);
            any = true;
        }
    }
    if any {
        plan.flags.push(PlanFlag::ArrivalSocBelowWanted);
    }
}

/// The relaxation ladder (ADR 0006 point 4): the two SoC floors are relaxed
/// in turn, returning the first feasible Plan. Corridor widening (the 4th
/// rung) is the corridor layer's business, not this pure search's.
const RELAXATION_RUNGS: [(FloorSource, FloorSource); 3] = [
    (FloorSource::Param, FloorSource::Param),
    (FloorSource::Zero, FloorSource::Param),
    (FloorSource::Zero, FloorSource::Zero),
];

#[derive(Clone, Copy)]
enum FloorSource {
    Param,
    Zero,
}

fn floor_value(src: FloorSource, param: f64) -> f64 {
    match src {
        FloorSource::Param => param,
        FloorSource::Zero => 0.0,
    }
}

fn solve_stop_free_alternative(
    graph: &CandidateGraph,
    params: &SearchParams,
    battery_wh: f64,
    charge_tables: &mut ChargeTables,
) -> Option<Plan> {
    for allow_negative in [false, true] {
        if let Some(result) = run_search(
            graph,
            params,
            battery_wh,
            0.0,
            0.0,
            allow_negative,
            true,
            charge_tables,
        ) {
            let start_key = (graph.origin, soc_bucket(params.depart_soc));
            let chain = reconstruct_chain(&result.came_from, start_key, result.final_key);
            let mut plan = build_plan(graph, params, &result, &chain);
            apply_relaxation_flags(&mut plan, params);
            return Some(plan);
        }
    }
    None
}

/// Solves for the time-minimising Plan over `graph` (ADR 0006/0010). Never
/// errors: an unreachable destination surfaces as a flagged Plan, per ADR
/// 0006 point 4.
pub fn solve(graph: &CandidateGraph, params: &SearchParams) -> Plan {
    let battery_wh = params.vehicle.usable_capacity_kwh * 1000.0;
    let start_key = (graph.origin, soc_bucket(params.depart_soc));
    let mut charge_tables = ChargeTables::new();

    let mut winning: Option<Plan> = None;
    for &(charger_src, dest_src) in &RELAXATION_RUNGS {
        let charger_min = floor_value(charger_src, params.charger_arrival_min_soc);
        let dest_min = floor_value(dest_src, params.arrival_min_soc);
        if let Some(result) = run_search(
            graph,
            params,
            battery_wh,
            charger_min,
            dest_min,
            false,
            false,
            &mut charge_tables,
        ) {
            let chain = reconstruct_chain(&result.came_from, start_key, result.final_key);
            let mut plan = build_plan(graph, params, &result, &chain);
            apply_relaxation_flags(&mut plan, params);
            winning = Some(plan);
            break;
        }
    }

    let mut plan = winning.unwrap_or_else(|| {
        // Last resort (ADR 0006 point 4): never error. Floors at 0 and SoC
        // allowed to go (bounded) negative always finds a path, since the
        // corridor layer guarantees the graph connects origin to dest.
        let result = run_search(
            graph,
            params,
            battery_wh,
            0.0,
            0.0,
            true,
            false,
            &mut charge_tables,
        )
        .expect("candidate graph must connect origin to dest structurally");
        let chain = reconstruct_chain(&result.came_from, start_key, result.final_key);
        let mut plan = build_plan(graph, params, &result, &chain);
        plan.flags.push(PlanFlag::RunsOutOfCharge);
        for leg in &mut plan.legs {
            if leg.arrival_soc <= 1e-9 {
                leg.flags.push(LegFlag::ReachesZeroSoc);
                break;
            }
        }
        plan
    });

    if params.offer_stop_free_alternative && plan.stops.len() == 1 && plan.stops[0].charge_s < 600.0
    {
        plan.alternative =
            solve_stop_free_alternative(graph, params, battery_wh, &mut charge_tables)
                .map(Box::new);
    }

    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CandidateLeg, CandidateNode, ChargerSite, LegEval, WaypointSpec};
    use energy::{Calibration, VehicleModel};

    fn vehicle() -> VehicleModel {
        VehicleModel::ioniq5_lr_2wd()
    }

    fn calibration() -> Calibration {
        Calibration::default()
    }

    fn params<'a>(vehicle: &'a VehicleModel, calibration: &'a Calibration) -> SearchParams<'a> {
        SearchParams {
            vehicle,
            calibration,
            depart_soc: 0.9,
            arrival_min_soc: 0.1,
            charger_arrival_min_soc: 0.1,
            charger_max_soc: 0.8,
            stops_bias: 1.0,
            battery_warmth: 1.0,
            offer_stop_free_alternative: false,
        }
    }

    /// One `LegEval` at every cap (uncapped energy/time; the caller
    /// overrides individual caps where a test cares).
    fn uniform_evals(time_s: f64, energy_wh: f64) -> [LegEval; 4] {
        [LegEval { time_s, energy_wh }; 4]
    }

    fn leg(to: u32, dist_m: f64, evals: [LegEval; 4]) -> CandidateLeg {
        CandidateLeg {
            to,
            dist_m,
            evals,
            route_edges: vec![],
        }
    }

    fn battery_wh() -> f64 {
        vehicle().usable_capacity_kwh * 1000.0
    }

    // --- direct feasible: 0 stops, no flags -----------------------------

    #[test]
    fn direct_feasible_has_no_stops_or_flags() {
        let v = vehicle();
        let c = calibration();
        let p = params(&v, &c);
        // 200 km at ~150 Wh/km = 30 kWh, ~43% of a 70 kWh pack: well inside
        // depart 0.9 -> arrival well above the 0.1 floor.
        let graph = CandidateGraph {
            nodes: vec![
                CandidateNode {
                    kind: NodeKind::Origin,
                    segment: 0,
                    out: vec![leg(1, 200_000.0, uniform_evals(7200.0, 30_000.0))],
                },
                CandidateNode {
                    kind: NodeKind::Dest,
                    segment: 0,
                    out: vec![],
                },
            ],
            sites: vec![],
            waypoints: vec![],
            origin: 0,
            dest: 1,
        };
        let plan = solve(&graph, &p);
        assert_eq!(plan.stops.len(), 0);
        assert!(plan.flags.is_empty());
        assert_eq!(plan.legs.len(), 1);
        assert!(plan.legs[0].flags.is_empty());
        assert!((plan.total_time_s - 7200.0).abs() < 1e-6);
    }

    // --- one stop needed: chosen charger, sensible target ----------------

    #[test]
    fn one_stop_needed_picks_charger_with_sensible_target_and_overhead() {
        let v = vehicle();
        let c = calibration();
        let p = params(&v, &c);
        let bwh = battery_wh();
        // origin -> charger: 50% of battery; charger -> dest: 50% of
        // battery. Depart 0.9 can't reach dest directly (would need 100%),
        // so one stop at the charger is required.
        let leg_energy = 0.5 * bwh;
        let graph = CandidateGraph {
            nodes: vec![
                CandidateNode {
                    kind: NodeKind::Origin,
                    segment: 0,
                    out: vec![leg(1, 150_000.0, uniform_evals(3600.0, leg_energy))],
                },
                CandidateNode {
                    kind: NodeKind::Charger { site: 0 },
                    segment: 0,
                    out: vec![leg(2, 150_000.0, uniform_evals(3600.0, leg_energy))],
                },
                CandidateNode {
                    kind: NodeKind::Dest,
                    segment: 0,
                    out: vec![],
                },
            ],
            sites: vec![ChargerSite {
                id: "c0".into(),
                name: "Charger 0".into(),
                lat: 0.0,
                lon: 0.0,
                power_kw: 150.0,
            }],
            waypoints: vec![],
            origin: 0,
            dest: 2,
        };
        let plan = solve(&graph, &p);
        assert!(plan.flags.is_empty());
        assert_eq!(plan.stops.len(), 1);
        let stop = &plan.stops[0];
        // need_for_leg = 0.5 + 0.1(charger_arrival_min_soc) + 0.05 = 0.65,
        // below every fixed target (0.6 is < 0.65+arrival, so 0.7 wins the
        // "just enough" test only loosely -- assert covers need+margin).
        assert!(
            stop.depart_soc >= 0.5 + p.charger_arrival_min_soc,
            "depart covers the next leg plus margin"
        );
        assert!(stop.depart_soc <= p.charger_max_soc + 1e-9);
        assert!(
            plan.total_time_s > plan.drive_time_s,
            "stop overhead/charge time must show up in total"
        );
        assert!(plan.charge_time_s > 0.0);
    }

    // --- two chargers, slow-near vs fast-far ------------------------------

    #[test]
    fn prefers_the_route_through_the_fast_charger_over_overcharging_at_the_slow_one() {
        let v = vehicle();
        let c = calibration();
        let p = params(&v, &c);
        let bwh = battery_wh();
        // Origin can reach either charger directly; each then covers the
        // rest of the trip to Dest alone (they are alternative routes, not
        // a chain). Both routes need the same total energy (0.80 of the
        // battery split differently), so this isolates charging speed as
        // the deciding factor:
        //  - via the slow (5 kW) charger: arrive at 0.2, must charge to the
        //    0.8 cap (the far leg alone would need 0.85, clamped) -- 0.6 of
        //    the battery at 5 kW is a ~30,000 s charge.
        //  - via the fast (1000 kW, i.e. uncapped by the charger -- limited
        //    only by the car's own curve) charger: arrive at 0.15, charge
        //    to exactly 0.8 (the only target that clears the leg's floor)
        //    -- 0.65 of the battery on the Ioniq 5 curve is on the order of
        //    ~1,000-1,500 s.
        // The ~20x gap in charge rate dwarfs the extra drive time and
        // detour energy of reaching the far charger, so the fast route
        // must win regardless of the curve's exact shape.
        let graph = CandidateGraph {
            nodes: vec![
                CandidateNode {
                    kind: NodeKind::Origin,
                    segment: 0,
                    out: vec![
                        leg(1, 50_000.0, uniform_evals(600.0, 0.10 * bwh)),
                        leg(2, 100_000.0, uniform_evals(1200.0, 0.15 * bwh)),
                    ],
                },
                CandidateNode {
                    kind: NodeKind::Charger { site: 0 }, // slow
                    segment: 0,
                    out: vec![leg(3, 500_000.0, uniform_evals(5000.0, 0.70 * bwh))],
                },
                CandidateNode {
                    kind: NodeKind::Charger { site: 1 }, // fast
                    segment: 0,
                    out: vec![leg(3, 450_000.0, uniform_evals(3000.0, 0.65 * bwh))],
                },
                CandidateNode {
                    kind: NodeKind::Dest,
                    segment: 0,
                    out: vec![],
                },
            ],
            sites: vec![
                ChargerSite {
                    id: "slow".into(),
                    name: "Slow".into(),
                    lat: 0.0,
                    lon: 0.0,
                    power_kw: 5.0,
                },
                ChargerSite {
                    id: "fast".into(),
                    name: "Fast".into(),
                    lat: 0.0,
                    lon: 0.0,
                    power_kw: 1000.0,
                },
            ],
            waypoints: vec![],
            origin: 0,
            dest: 3,
        };
        let mut p2 = p;
        p2.depart_soc = 0.3;
        let plan = solve(&graph, &p2);
        assert!(plan.flags.is_empty());
        assert_eq!(plan.stops.len(), 1);
        assert_eq!(
            plan.stops[0].site, 1,
            "picks the fast charger over overcharging at the slow one"
        );
        // The fast leg's floor (0.1) forces exactly one feasible target
        // (0.65 energy + 0.1 floor + 0.05 margin = 0.8, the charger cap):
        // deterministic regardless of the curve integration.
        assert!((plan.stops[0].depart_soc - 0.8).abs() < 1e-6);
        // A ~30,000 s slow-charger route would dwarf this; the fast route
        // must land far below it.
        assert!(
            plan.total_time_s < 10_000.0,
            "fast route must be far cheaper than the slow-charger alternative"
        );
    }

    // --- caps --------------------------------------------------------------

    #[test]
    fn speed_cap_kicks_in_when_uncapped_misses_the_floor() {
        let v = vehicle();
        let c = calibration();
        let p = params(&v, &c);
        let bwh = battery_wh();
        // Uncapped (idx 0) and 110 (idx 1) burn too much energy to clear
        // the 0.1 arrival floor from a 0.9 depart; 100 km/h (idx 2) is
        // slower but cheap enough to just clear it; 90 km/h (idx 3) also
        // clears it but is slower than the 100 km/h option.
        let evals = [
            LegEval {
                time_s: 3000.0,
                energy_wh: 0.85 * bwh,
            }, // uncapped: fails floor
            LegEval {
                time_s: 3200.0,
                energy_wh: 0.83 * bwh,
            }, // 110: fails floor
            LegEval {
                time_s: 3600.0,
                energy_wh: 0.75 * bwh,
            }, // 100: passes, faster than 90
            LegEval {
                time_s: 4000.0,
                energy_wh: 0.70 * bwh,
            }, // 90: passes, slower
        ];
        let graph = CandidateGraph {
            nodes: vec![
                CandidateNode {
                    kind: NodeKind::Origin,
                    segment: 0,
                    out: vec![leg(1, 300_000.0, evals)],
                },
                CandidateNode {
                    kind: NodeKind::Dest,
                    segment: 0,
                    out: vec![],
                },
            ],
            sites: vec![],
            waypoints: vec![],
            origin: 0,
            dest: 1,
        };
        let plan = solve(&graph, &p);
        assert_eq!(plan.stops.len(), 0);
        assert_eq!(plan.legs[0].speed_cap_kmh, Some(100.0));
    }

    #[test]
    fn uncapped_stays_uncapped_when_feasible_and_faster() {
        let v = vehicle();
        let c = calibration();
        let p = params(&v, &c);
        let bwh = battery_wh();
        let evals = [
            LegEval {
                time_s: 3000.0,
                energy_wh: 0.3 * bwh,
            }, // uncapped: feasible and fastest
            LegEval {
                time_s: 3200.0,
                energy_wh: 0.28 * bwh,
            },
            LegEval {
                time_s: 3600.0,
                energy_wh: 0.25 * bwh,
            },
            LegEval {
                time_s: 4000.0,
                energy_wh: 0.20 * bwh,
            },
        ];
        let graph = CandidateGraph {
            nodes: vec![
                CandidateNode {
                    kind: NodeKind::Origin,
                    segment: 0,
                    out: vec![leg(1, 300_000.0, evals)],
                },
                CandidateNode {
                    kind: NodeKind::Dest,
                    segment: 0,
                    out: vec![],
                },
            ],
            sites: vec![],
            waypoints: vec![],
            origin: 0,
            dest: 1,
        };
        let plan = solve(&graph, &p);
        assert_eq!(plan.legs[0].speed_cap_kmh, None);
    }

    // --- ramp penalty --------------------------------------------------

    #[test]
    fn ramp_penalty_flips_winner_to_the_capped_stop_free_route() {
        let v = vehicle();
        let c = calibration();
        let p = params(&v, &c);
        let bwh = battery_wh();
        // Two parallel origin->dest options via distinct legs is awkward
        // with one edge list, so model it as: a micro-stop route through a
        // charger (uncapped, fast drive, but needs a ~3 min top-up) versus
        // a slower capped direct drive that avoids the stop. Two out-edges
        // from origin: one to a charger then dest, one straight to dest at
        // a cap.
        //
        // Charger route: drive 3000s uncapped (idx0) then a tiny top-up
        // (from 0.05 to 0.10, a ~180s splash at 350kW), then dest leg 3000s
        // uncapped. Raw time ~6000s + ~180s charge + 300s overhead = 6480s
        // before the ramp penalty; ramp penalty adds (600-180)/600*300 =
        // 210s, total 6690s.
        //
        // Capped direct route: 100 km/h cap (idx2) takes 6600s uncapped
        // energy would fail the floor, but the capped variant clears it
        // with no stop at all: 6600s flat, cheaper than 6690s.
        let evals_to_charger = [
            LegEval {
                time_s: 3000.0,
                energy_wh: 0.85 * bwh,
            },
            LegEval {
                time_s: 3200.0,
                energy_wh: 0.85 * bwh,
            },
            LegEval {
                time_s: 3400.0,
                energy_wh: 0.85 * bwh,
            },
            LegEval {
                time_s: 3600.0,
                energy_wh: 0.85 * bwh,
            },
        ];
        let evals_charger_to_dest = [
            LegEval {
                time_s: 3000.0,
                energy_wh: 0.05 * bwh,
            },
            LegEval {
                time_s: 3000.0,
                energy_wh: 0.05 * bwh,
            },
            LegEval {
                time_s: 3000.0,
                energy_wh: 0.05 * bwh,
            },
            LegEval {
                time_s: 3000.0,
                energy_wh: 0.05 * bwh,
            },
        ];
        let evals_direct = [
            LegEval {
                time_s: 6000.0,
                energy_wh: 0.95 * bwh,
            }, // uncapped: fails floor
            LegEval {
                time_s: 6200.0,
                energy_wh: 0.92 * bwh,
            }, // fails floor
            LegEval {
                time_s: 6600.0,
                energy_wh: 0.80 * bwh,
            }, // 100 km/h: passes, stop-free
            LegEval {
                time_s: 7200.0,
                energy_wh: 0.70 * bwh,
            }, // 90 km/h: passes, slower
        ];
        let graph = CandidateGraph {
            nodes: vec![
                CandidateNode {
                    kind: NodeKind::Origin,
                    segment: 0,
                    out: vec![
                        leg(1, 300_000.0, evals_to_charger),
                        leg(2, 300_000.0, evals_direct),
                    ],
                },
                CandidateNode {
                    kind: NodeKind::Charger { site: 0 },
                    segment: 0,
                    out: vec![leg(2, 300_000.0, evals_charger_to_dest)],
                },
                CandidateNode {
                    kind: NodeKind::Dest,
                    segment: 0,
                    out: vec![],
                },
            ],
            sites: vec![ChargerSite {
                id: "c0".into(),
                name: "C0".into(),
                lat: 0.0,
                lon: 0.0,
                power_kw: 350.0,
            }],
            waypoints: vec![],
            origin: 0,
            dest: 2,
        };
        let mut p2 = p;
        p2.depart_soc = 0.9;
        p2.arrival_min_soc = 0.1;
        p2.charger_arrival_min_soc = 0.02;
        let plan = solve(&graph, &p2);
        assert_eq!(
            plan.stops.len(),
            0,
            "ramp penalty makes the capped stop-free route win"
        );
        assert_eq!(plan.legs[0].speed_cap_kmh, Some(100.0));
    }

    #[test]
    fn micro_stop_still_produced_when_it_is_the_only_feasible_plan() {
        let v = vehicle();
        let c = calibration();
        let p = params(&v, &c);
        let bwh = battery_wh();
        // Only route available is via the charger; no direct/capped
        // alternative exists at all. The ramp penalty must never block the
        // only feasible plan (ADR 0010 point 3: "soft, never hard").
        let evals_to_charger = uniform_evals(3000.0, 0.85 * bwh);
        let evals_charger_to_dest = uniform_evals(3000.0, 0.05 * bwh);
        let graph = CandidateGraph {
            nodes: vec![
                CandidateNode {
                    kind: NodeKind::Origin,
                    segment: 0,
                    out: vec![leg(1, 300_000.0, evals_to_charger)],
                },
                CandidateNode {
                    kind: NodeKind::Charger { site: 0 },
                    segment: 0,
                    out: vec![leg(2, 300_000.0, evals_charger_to_dest)],
                },
                CandidateNode {
                    kind: NodeKind::Dest,
                    segment: 0,
                    out: vec![],
                },
            ],
            sites: vec![ChargerSite {
                id: "c0".into(),
                name: "C0".into(),
                lat: 0.0,
                lon: 0.0,
                power_kw: 350.0,
            }],
            waypoints: vec![],
            origin: 0,
            dest: 2,
        };
        let mut p2 = p;
        p2.charger_arrival_min_soc = 0.02;
        let plan = solve(&graph, &p2);
        assert_eq!(plan.stops.len(), 1);
        assert!(plan.stops[0].charge_s > 0.0);
    }

    // --- pass-through exclusion -----------------------------------------

    #[test]
    fn charger_exactly_en_route_with_no_charge_needed_yields_zero_stops() {
        let v = vehicle();
        let c = calibration();
        let p = params(&v, &c);
        let bwh = battery_wh();
        // A charger sits midway but the trip never needs it: origin -> dest
        // energy is well inside range even when routed through the charger
        // node (same total energy as a direct trip would be).
        let graph = CandidateGraph {
            nodes: vec![
                CandidateNode {
                    kind: NodeKind::Origin,
                    segment: 0,
                    out: vec![leg(1, 100_000.0, uniform_evals(3600.0, 0.15 * bwh))],
                },
                CandidateNode {
                    kind: NodeKind::Charger { site: 0 },
                    segment: 0,
                    out: vec![leg(2, 100_000.0, uniform_evals(3600.0, 0.15 * bwh))],
                },
                CandidateNode {
                    kind: NodeKind::Dest,
                    segment: 0,
                    out: vec![],
                },
            ],
            sites: vec![ChargerSite {
                id: "c0".into(),
                name: "C0".into(),
                lat: 0.0,
                lon: 0.0,
                power_kw: 150.0,
            }],
            waypoints: vec![],
            origin: 0,
            dest: 2,
        };
        let plan = solve(&graph, &p);
        assert_eq!(plan.stops.len(), 0);
        assert!(plan.flags.is_empty());
        assert_eq!(
            plan.legs.len(),
            2,
            "the label still passes through the charger node"
        );
    }

    // --- waypoints -------------------------------------------------------

    fn waypoint_graph(override_soc: Option<f64>, wp_charger: Option<u32>) -> CandidateGraph {
        let bwh = VehicleModel::ioniq5_lr_2wd().usable_capacity_kwh * 1000.0;
        CandidateGraph {
            nodes: vec![
                CandidateNode {
                    kind: NodeKind::Origin,
                    segment: 0,
                    out: vec![leg(1, 100_000.0, uniform_evals(3600.0, 0.3 * bwh))],
                },
                CandidateNode {
                    kind: NodeKind::Waypoint {
                        wp: 0,
                        charger: wp_charger,
                    },
                    segment: 1,
                    out: vec![leg(2, 100_000.0, uniform_evals(3600.0, 0.3 * bwh))],
                },
                CandidateNode {
                    kind: NodeKind::Dest,
                    segment: 1,
                    out: vec![],
                },
            ],
            sites: if wp_charger.is_some() {
                vec![ChargerSite {
                    id: "wp".into(),
                    name: "WP".into(),
                    lat: 0.0,
                    lon: 0.0,
                    power_kw: 150.0,
                }]
            } else {
                vec![]
            },
            waypoints: vec![WaypointSpec {
                lat: 0.0,
                lon: 0.0,
                depart_soc_override: override_soc,
            }],
            origin: 0,
            dest: 2,
        }
    }

    #[test]
    fn plan_visits_the_waypoint_when_direct_leg_is_structurally_absent() {
        let v = vehicle();
        let c = calibration();
        let p = params(&v, &c);
        let graph = waypoint_graph(None, None);
        let plan = solve(&graph, &p);
        assert_eq!(plan.legs.len(), 2);
        assert_eq!(plan.legs[0].to, Endpoint::Waypoint { wp: 0 });
        assert_eq!(plan.legs[1].from, Endpoint::Waypoint { wp: 0 });
    }

    #[test]
    fn waypoint_override_above_arrival_soc_with_no_charger_forces_relaxation_flag() {
        let v = vehicle();
        let c = calibration();
        let p = params(&v, &c);
        // Arrival at the waypoint is 0.9 - 0.3 = 0.6; the override demands
        // 0.8, which is structurally impossible with no charger there, so
        // the strict solve fails and the ladder's flags surface.
        let graph = waypoint_graph(Some(0.8), None);
        let plan = solve(&graph, &p);
        assert!(
            !plan.flags.is_empty() || !plan.legs.is_empty(),
            "never errors"
        );
        // Every reachable route now discards the waypoint label under the
        // override, so even full relaxation of the SoC floors can't help
        // (the override itself isn't relaxed) -- it lands in the
        // RunsOutOfCharge fallback, which drives with the override ignored
        // to the letter but still flags infeasibility.
        assert!(plan.flags.contains(&PlanFlag::RunsOutOfCharge));
    }

    #[test]
    fn waypoint_override_with_charger_charges_up_to_the_override() {
        let v = vehicle();
        let c = calibration();
        let p = params(&v, &c);
        let graph = waypoint_graph(Some(0.8), Some(0));
        let plan = solve(&graph, &p);
        assert!(plan.flags.is_empty());
        assert_eq!(plan.stops.len(), 1);
        assert!(plan.stops[0].depart_soc + 1e-9 >= 0.8);
    }

    // --- relaxation ladder -------------------------------------------------

    #[test]
    fn infeasible_under_wanted_floor_but_feasible_at_relaxed_dest_floor_flags_correctly() {
        let v = vehicle();
        let c = calibration();
        let p = params(&v, &c);
        let bwh = battery_wh();
        // Direct trip arrives at exactly 0.05: below the 0.1 wanted floor,
        // but fine once the destination floor relaxes to 0.
        let mut p2 = p;
        p2.depart_soc = 0.9;
        p2.arrival_min_soc = 0.1;
        let graph = CandidateGraph {
            nodes: vec![
                CandidateNode {
                    kind: NodeKind::Origin,
                    segment: 0,
                    out: vec![leg(1, 400_000.0, uniform_evals(10_000.0, 0.85 * bwh))],
                },
                CandidateNode {
                    kind: NodeKind::Dest,
                    segment: 0,
                    out: vec![],
                },
            ],
            sites: vec![],
            waypoints: vec![],
            origin: 0,
            dest: 1,
        };
        let plan = solve(&graph, &p2);
        assert!(plan.flags.contains(&PlanFlag::ArrivalSocBelowWanted));
        assert!(plan.legs[0].flags.contains(&LegFlag::ArrivalSocBelowWanted));
        assert!((plan.legs[0].arrival_soc - 0.05).abs() < 1e-6);
    }

    #[test]
    fn totally_infeasible_falls_back_to_runs_out_of_charge_with_zero_soc_leg_flagged() {
        let v = vehicle();
        let c = calibration();
        let p = params(&v, &c);
        let bwh = battery_wh();
        // The single leg needs more energy than a full battery: no floor
        // relaxation can save this.
        let mut p2 = p;
        p2.depart_soc = 0.9;
        let graph = CandidateGraph {
            nodes: vec![
                CandidateNode {
                    kind: NodeKind::Origin,
                    segment: 0,
                    out: vec![leg(1, 900_000.0, uniform_evals(30_000.0, 1.3 * bwh))],
                },
                CandidateNode {
                    kind: NodeKind::Dest,
                    segment: 0,
                    out: vec![],
                },
            ],
            sites: vec![],
            waypoints: vec![],
            origin: 0,
            dest: 1,
        };
        let plan = solve(&graph, &p2);
        assert!(plan.flags.contains(&PlanFlag::RunsOutOfCharge));
        assert_eq!(plan.legs.len(), 1);
        assert!(plan.legs[0].flags.contains(&LegFlag::ReachesZeroSoc));
        assert!(plan.legs[0].arrival_soc <= 1e-9);
    }

    // --- ties -> fewer stops ------------------------------------------------

    #[test]
    fn ties_break_toward_fewer_stops() {
        let v = vehicle();
        let c = calibration();
        let p = params(&v, &c);
        let bwh = battery_wh();
        // A real charge duration isn't hand-computable (it's a numeric
        // integral over the vehicle's curve), so the tie is constructed by
        // calling the same `charge_duration_s` the search itself calls,
        // then setting the direct route's drive time to match exactly --
        // an honest equal-time construction rather than an approximation.
        //
        // Charger leg is set up so the discrete target menu collapses to
        // exactly one feasible target (0.8, forced by need_for_leg's
        // clamp to charger_max_soc), so the search can't pick a different,
        // non-tied target.
        let charger_power_kw = 150.0;
        let charge_from = 0.5;
        let charge_to = 0.8;
        let charge_s = energy::charge_duration_s(
            &v,
            p.battery_warmth,
            charge_from,
            charge_to,
            charger_power_kw,
            false,
        );
        let stop_overhead_s = 300.0 * p.stops_bias;
        let ramp_s = if charge_s < 600.0 {
            (600.0 - charge_s) / 600.0 * 300.0
        } else {
            0.0
        };
        let t1 = 1000.0; // origin -> charger drive time
        let t2 = 2000.0; // charger -> dest drive time
        let total_via_charger = t1 + charge_s + stop_overhead_s + ramp_s + t2;

        let graph = CandidateGraph {
            nodes: vec![
                CandidateNode {
                    kind: NodeKind::Origin,
                    segment: 0,
                    out: vec![
                        // Direct: 0 stops, drive time set to tie the
                        // via-charger route exactly. depart 0.9 -> arrival
                        // 0.6, comfortably above the 0.1 floor.
                        leg(1, 400_000.0, uniform_evals(total_via_charger, 0.3 * bwh)),
                        // depart 0.9 -> arrival at the charger 0.5, matching
                        // `charge_from` above.
                        leg(2, 40_000.0, uniform_evals(t1, 0.4 * bwh)),
                    ],
                },
                CandidateNode {
                    kind: NodeKind::Dest,
                    segment: 0,
                    out: vec![],
                },
                CandidateNode {
                    kind: NodeKind::Charger { site: 0 },
                    segment: 0,
                    // need_for_leg = 0.65 + 0.1 + 0.05 = 0.8 = charger_max_soc,
                    // and it's the only target of the menu whose soc_after
                    // (target - 0.65) clears the 0.1 floor.
                    out: vec![leg(1, 200_000.0, uniform_evals(t2, 0.65 * bwh))],
                },
            ],
            sites: vec![ChargerSite {
                id: "c0".into(),
                name: "C0".into(),
                lat: 0.0,
                lon: 0.0,
                power_kw: charger_power_kw,
            }],
            waypoints: vec![],
            origin: 0,
            dest: 1,
        };
        let plan = solve(&graph, &p);
        assert_eq!(
            plan.stops.len(),
            0,
            "tie between equal-time plans must favour fewer stops"
        );
        assert!(
            (plan.total_time_s - total_via_charger).abs() < 1e-6,
            "winning plan's time must match the tie exactly"
        );
    }

    // --- stop-free alternative --------------------------------------------

    #[test]
    fn stop_free_alternative_present_when_requested_and_winner_is_a_micro_stop() {
        let v = vehicle();
        let c = calibration();
        let p = params(&v, &c);
        let bwh = battery_wh();
        // Same shape as the ramp-penalty test but tuned so the micro-stop
        // route still wins overall (e.g. the capped alternative is even
        // slower here), while a stop-free alternative remains reachable at
        // relaxed floors.
        let evals_to_charger = uniform_evals(1000.0, 0.85 * bwh);
        let evals_charger_to_dest = uniform_evals(1000.0, 0.05 * bwh);
        let evals_direct = [
            LegEval {
                time_s: 6000.0,
                energy_wh: 0.95 * bwh,
            },
            LegEval {
                time_s: 6200.0,
                energy_wh: 0.92 * bwh,
            },
            LegEval {
                time_s: 9000.0,
                energy_wh: 0.80 * bwh,
            }, // stop-free but much slower
            LegEval {
                time_s: 9500.0,
                energy_wh: 0.70 * bwh,
            },
        ];
        let graph = CandidateGraph {
            nodes: vec![
                CandidateNode {
                    kind: NodeKind::Origin,
                    segment: 0,
                    out: vec![
                        leg(1, 300_000.0, evals_to_charger),
                        leg(2, 300_000.0, evals_direct),
                    ],
                },
                CandidateNode {
                    kind: NodeKind::Charger { site: 0 },
                    segment: 0,
                    out: vec![leg(2, 300_000.0, evals_charger_to_dest)],
                },
                CandidateNode {
                    kind: NodeKind::Dest,
                    segment: 0,
                    out: vec![],
                },
            ],
            sites: vec![ChargerSite {
                id: "c0".into(),
                name: "C0".into(),
                lat: 0.0,
                lon: 0.0,
                power_kw: 350.0,
            }],
            waypoints: vec![],
            origin: 0,
            dest: 2,
        };
        let mut p2 = p;
        p2.charger_arrival_min_soc = 0.02;
        p2.offer_stop_free_alternative = true;
        let plan = solve(&graph, &p2);
        assert_eq!(plan.stops.len(), 1);
        assert!(
            plan.stops[0].charge_s < 600.0,
            "winner must be a micro-stop for the alternative to trigger"
        );
        let alt = plan
            .alternative
            .expect("stop-free alternative must be attached");
        assert_eq!(alt.stops.len(), 0);
        assert!(alt.flags.contains(&PlanFlag::ArrivalSocBelowWanted));
    }
}
