//! Pure request/response mapping between the wire (`Ffi*`) shapes and the
//! optimiser's own types. Kept as free functions, deliberately independent
//! of `Planner`/`Rpack`, so they're unit-testable without a Region Pack
//! fixture -- the two functions that genuinely need one (`build_polyline`
//! needs `pack.geometry_for_edge`; everything else here is pure data) live
//! at the bottom and are exercised instead by the Swift golden test
//! (`app/PlannerKit/Tests/PlannerKitTests/GoldenPlanTests.swift`).

use energy::{Calibration, VehicleModel};
use optimiser::{
    AssembleError, ChargerSite, CorridorRequest, Endpoint, Plan, PlanLeg, Stop, WaypointSpec,
};
use packs::Rpack;

use crate::error::PlannerError;
use crate::types::{
    FfiGeoPoint, FfiLeg, FfiManeuver, FfiManeuverModifier, FfiPlan, FfiPlanAlt, FfiPlanRequest,
    FfiSocPoint, FfiStep, FfiStop, FfiVehicle, FfiWaypoint,
};

const CPACK_FORMAT: &str = "cpack-1";

pub fn vehicle_of(v: FfiVehicle) -> VehicleModel {
    match v {
        FfiVehicle::Ioniq5Lr2wd => VehicleModel::ioniq5_lr_2wd(),
        FfiVehicle::Ioniq5LrAwd => VehicleModel::ioniq5_lr_awd(),
    }
}

pub fn calibration_of(
    vehicle: &VehicleModel,
    reference_consumption_wh_per_km: Option<f64>,
) -> Calibration {
    match reference_consumption_wh_per_km {
        Some(wh_per_km) => Calibration::from_reference_consumption(vehicle, wh_per_km),
        None => Calibration::default(),
    }
}

pub fn corridor_request_of(req: &FfiPlanRequest) -> CorridorRequest {
    CorridorRequest {
        origin: (req.origin_lat, req.origin_lon),
        waypoints: req
            .waypoints
            .iter()
            .map(|w: &FfiWaypoint| WaypointSpec {
                lat: w.lat,
                lon: w.lon,
                depart_soc_override: w.depart_soc_override,
            })
            .collect(),
        dest: (req.dest_lat, req.dest_lon),
        temp_c: req.temp_c,
        headwind_ms: req.headwind_ms,
    }
}

pub fn plan_request_of(req: &FfiPlanRequest, corridor: CorridorRequest) -> optimiser::PlanRequest {
    optimiser::PlanRequest {
        corridor,
        depart_soc: req.depart_soc,
        arrival_min_soc: req.arrival_min_soc,
        charger_arrival_min_soc: req.charger_arrival_min_soc,
        charger_max_soc: req.charger_max_soc,
        stops_bias: req.stops_bias,
        battery_warmth: req.battery_warmth,
        offer_stop_free_alternative: req.offer_stop_free_alternative,
    }
}

/// Maps a Charger Pack format tag; only `"cpack-1"` (`corridor::parse_cpack`)
/// is supported today.
pub fn validate_cpack_format(format: &str) -> Result<(), PlannerError> {
    if format != CPACK_FORMAT {
        return Err(PlannerError::InvalidRequest {
            message: format!("unsupported charger pack format: {format}"),
        });
    }
    Ok(())
}

/// ADR 0006 point 4's connectivity failures both surface as "no route";
/// cancellation (ADR 0004 point 4) passes through as-is.
pub fn map_assemble_error(e: AssembleError) -> PlannerError {
    let message = e.to_string();
    match e {
        AssembleError::SnapFailed { .. } | AssembleError::NoRoute { .. } => {
            PlannerError::NoRouteFound { message }
        }
        AssembleError::Cancelled => PlannerError::Cancelled { message },
    }
}

fn endpoint_label(plan: &Plan, ep: &Endpoint) -> String {
    match ep {
        Endpoint::Origin => "Origin".to_string(),
        Endpoint::Dest => "Dest".to_string(),
        Endpoint::Waypoint { wp } => format!("Waypoint {}", wp + 1),
        Endpoint::Charger { site } => plan.sites[*site as usize].name.clone(),
    }
}

fn ffi_leg_of(plan: &Plan, leg: &PlanLeg, steps: Vec<FfiStep>) -> FfiLeg {
    FfiLeg {
        from_label: endpoint_label(plan, &leg.from),
        to_label: endpoint_label(plan, &leg.to),
        drive_s: leg.drive_s,
        dist_m: leg.dist_m,
        energy_wh: leg.energy_wh,
        speed_cap_kmh: leg.speed_cap_kmh,
        depart_soc: leg.depart_soc,
        arrival_soc: leg.arrival_soc,
        flags: leg.flags.iter().map(|f| format!("{f:?}")).collect(),
        steps,
    }
}

/// Zips `plan.legs` with `leg_steps` into `FfiLeg`s; defensive against a
/// length mismatch (a missing leg just gets empty steps) even though
/// `build_leg_steps` always returns one entry per leg.
fn ffi_legs_of(plan: &Plan, mut leg_steps: Vec<Vec<FfiStep>>) -> Vec<FfiLeg> {
    leg_steps.resize(plan.legs.len(), Vec::new());
    plan.legs
        .iter()
        .zip(leg_steps)
        .map(|(leg, steps)| ffi_leg_of(plan, leg, steps))
        .collect()
}

fn ffi_stop_of(sites: &[ChargerSite], stop: &Stop) -> FfiStop {
    let site = &sites[stop.site as usize];
    FfiStop {
        name: site.name.clone(),
        charger_id: site.id.clone(),
        lat: site.lat,
        lon: site.lon,
        power_kw: site.power_kw,
        arrival_soc: stop.arrival_soc,
        depart_soc: stop.depart_soc,
        charge_s: stop.charge_s,
    }
}

/// ~2km-spaced samples across the whole Plan, SoC linearly interpolated
/// within each Leg (uniform energy density -- "the slice did the same"),
/// plus a vertical-jump pair at each Charging Stop. Pure over `Plan`, so
/// (unlike `build_polyline`) this is unit-tested directly below.
pub fn build_soc_curve(plan: &Plan) -> Vec<FfiSocPoint> {
    const SAMPLE_M: f64 = 2_000.0;

    let mut points = Vec::new();
    let mut cum_before_leg = 0.0;
    let mut next_sample = 0.0;
    let mut stop_idx = 0usize;

    for leg in &plan.legs {
        while next_sample < cum_before_leg + leg.dist_m {
            if next_sample >= cum_before_leg {
                let frac = if leg.dist_m > 0.0 {
                    (next_sample - cum_before_leg) / leg.dist_m
                } else {
                    0.0
                };
                points.push(FfiSocPoint {
                    dist_m: next_sample,
                    soc: leg.depart_soc + (leg.arrival_soc - leg.depart_soc) * frac,
                });
            }
            next_sample += SAMPLE_M;
        }

        let leg_end = cum_before_leg + leg.dist_m;
        points.push(FfiSocPoint {
            dist_m: leg_end,
            soc: leg.arrival_soc,
        });

        // A Charging Stop consumes its site's slot in `plan.stops` in
        // order; a Charger `Leg::to` with no matching Stop is a pass-through
        // (ADR 0006's MIN_CHARGE_SOC floor), so no jump point is added.
        if let Endpoint::Charger { site } = leg.to {
            if stop_idx < plan.stops.len() && plan.stops[stop_idx].site == site {
                points.push(FfiSocPoint {
                    dist_m: leg_end,
                    soc: plan.stops[stop_idx].depart_soc,
                });
                stop_idx += 1;
            }
        }

        cum_before_leg = leg_end;
    }

    points
}

/// Builds the non-recursive alternative-plan record; `polyline`/`soc_curve`/
/// `leg_steps` are the alternative `Plan`'s own (built by the caller, which
/// has the `&Rpack` this module deliberately doesn't need).
pub fn ffi_plan_alt_of(
    plan: &Plan,
    polyline: Vec<FfiGeoPoint>,
    soc_curve: Vec<FfiSocPoint>,
    leg_steps: Vec<Vec<FfiStep>>,
) -> FfiPlanAlt {
    FfiPlanAlt {
        legs: ffi_legs_of(plan, leg_steps),
        stops: plan
            .stops
            .iter()
            .map(|s| ffi_stop_of(&plan.sites, s))
            .collect(),
        drive_time_s: plan.drive_time_s,
        charge_time_s: plan.charge_time_s,
        total_time_s: plan.total_time_s,
        total_dist_m: plan.total_dist_m,
        flags: plan.flags.iter().map(|f| format!("{f:?}")).collect(),
        soc_curve,
        polyline,
    }
}

pub fn ffi_plan_of(
    plan: &Plan,
    polyline: Vec<FfiGeoPoint>,
    soc_curve: Vec<FfiSocPoint>,
    leg_steps: Vec<Vec<FfiStep>>,
    alternative: Option<FfiPlanAlt>,
) -> FfiPlan {
    FfiPlan {
        legs: ffi_legs_of(plan, leg_steps),
        stops: plan
            .stops
            .iter()
            .map(|s| ffi_stop_of(&plan.sites, s))
            .collect(),
        drive_time_s: plan.drive_time_s,
        charge_time_s: plan.charge_time_s,
        total_time_s: plan.total_time_s,
        total_dist_m: plan.total_dist_m,
        flags: plan.flags.iter().map(|f| format!("{f:?}")).collect(),
        soc_curve,
        polyline,
        alternative,
    }
}

/// Distance in metres from `p` to the SEGMENT `a`-`b` (projection clamped to
/// the segment, not the infinite line: on looping geometry -- a cloverleaf
/// ramp, a hairpin -- a point can sit far past the chord's ends yet nearly
/// collinear with it, and line distance would call that zero and let DP drop
/// it), using an equirectangular approximation (`cos(mean lat)` scaling of
/// longitude) -- accurate enough at the few-metre tolerances this is used
/// at. Degenerate `a == b` falls back to straight-line distance to `a`.
fn perpendicular_distance_m(p: &FfiGeoPoint, a: &FfiGeoPoint, b: &FfiGeoPoint) -> f64 {
    const M_PER_DEG_LAT: f64 = 111_320.0;
    let mean_lat_rad = ((a.lat + b.lat) / 2.0).to_radians();
    let cos_lat = mean_lat_rad.cos();
    let m_per_deg_lon = M_PER_DEG_LAT * cos_lat;

    let ax = a.lon * m_per_deg_lon;
    let ay = a.lat * M_PER_DEG_LAT;
    let bx = b.lon * m_per_deg_lon;
    let by = b.lat * M_PER_DEG_LAT;
    let px = p.lon * m_per_deg_lon;
    let py = p.lat * M_PER_DEG_LAT;

    let dx = bx - ax;
    let dy = by - ay;
    let len_sq = dx * dx + dy * dy;
    if len_sq == 0.0 {
        return ((px - ax).powi(2) + (py - ay).powi(2)).sqrt();
    }

    let t = (((px - ax) * dx + (py - ay) * dy) / len_sq).clamp(0.0, 1.0);
    let cx = ax + t * dx;
    let cy = ay + t * dy;
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

/// Douglas-Peucker simplification at `tolerance_m` metres, iterative (an
/// explicit stack of index ranges rather than recursion, so a long Leg's
/// point list can't blow the stack). Always keeps `points`' first and last
/// vertex.
fn simplify_dp(points: &[FfiGeoPoint], tolerance_m: f64) -> Vec<FfiGeoPoint> {
    if points.len() < 3 {
        return points.to_vec();
    }

    // `keep[i]` marks whether `points[i]` survives simplification.
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;

    let mut stack: Vec<(usize, usize)> = vec![(0, points.len() - 1)];
    while let Some((start, end)) = stack.pop() {
        if end <= start + 1 {
            continue;
        }
        let (mut split_idx, mut max_dist) = (0usize, 0.0f64);
        for i in (start + 1)..end {
            let dist = perpendicular_distance_m(&points[i], &points[start], &points[end]);
            if dist > max_dist {
                max_dist = dist;
                split_idx = i;
            }
        }
        if max_dist >= tolerance_m {
            keep[split_idx] = true;
            stack.push((start, split_idx));
            stack.push((split_idx, end));
        }
    }

    points
        .iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

/// Concatenates each Leg's `route_edges` geometry (dropping the duplicated
/// junction vertex between consecutive edges within a Leg), simplified with
/// Douglas-Peucker at a 3 m tolerance -- applied per Leg so Leg boundaries
/// (Charging Stops, Waypoints) stay exact -- to bound the `RustBuffer` (ADR
/// 0004 point 3) with an actual geometric error guarantee instead of index
/// decimation. Needs `&Rpack`, so unlike the rest of this module it is
/// exercised by the Swift golden test rather than a Rust unit test.
pub fn build_polyline(pack: &Rpack, plan: &Plan) -> Vec<FfiGeoPoint> {
    const SIMPLIFY_TOLERANCE_M: f64 = 3.0;
    let mut out: Vec<FfiGeoPoint> = Vec::new();

    for leg in &plan.legs {
        if leg.route_edges.is_empty() {
            continue;
        }
        let mut leg_pts: Vec<FfiGeoPoint> = Vec::new();
        for (i, &edge_idx) in leg.route_edges.iter().enumerate() {
            let edge = &pack.edges()[edge_idx as usize];
            let verts = pack.geometry_for_edge(edge);
            let skip = if i == 0 { 0 } else { 1 };
            for v in verts.iter().skip(skip) {
                leg_pts.push(FfiGeoPoint {
                    lat: v.lat as f64,
                    lon: v.lon as f64,
                });
            }
        }
        if leg_pts.is_empty() {
            continue;
        }
        for p in simplify_dp(&leg_pts, SIMPLIFY_TOLERANCE_M) {
            if out.last() == Some(&p) {
                continue;
            }
            out.push(p);
        }
    }

    out
}

fn ffi_maneuver_of(m: guidance::ManeuverType) -> FfiManeuver {
    match m {
        guidance::ManeuverType::Depart => FfiManeuver::Depart,
        guidance::ManeuverType::Arrive => FfiManeuver::Arrive,
        guidance::ManeuverType::Turn => FfiManeuver::Turn,
        guidance::ManeuverType::Continue => FfiManeuver::Continue,
        guidance::ManeuverType::OffRamp => FfiManeuver::OffRamp,
        guidance::ManeuverType::OnRamp => FfiManeuver::OnRamp,
        guidance::ManeuverType::Fork => FfiManeuver::Fork,
        guidance::ManeuverType::EndOfRoad => FfiManeuver::EndOfRoad,
        guidance::ManeuverType::Roundabout => FfiManeuver::Roundabout,
    }
}

fn ffi_maneuver_modifier_of(m: guidance::ManeuverModifier) -> FfiManeuverModifier {
    match m {
        guidance::ManeuverModifier::Straight => FfiManeuverModifier::Straight,
        guidance::ManeuverModifier::SlightLeft => FfiManeuverModifier::SlightLeft,
        guidance::ManeuverModifier::SlightRight => FfiManeuverModifier::SlightRight,
        guidance::ManeuverModifier::Left => FfiManeuverModifier::Left,
        guidance::ManeuverModifier::Right => FfiManeuverModifier::Right,
        guidance::ManeuverModifier::SharpLeft => FfiManeuverModifier::SharpLeft,
        guidance::ManeuverModifier::SharpRight => FfiManeuverModifier::SharpRight,
        guidance::ManeuverModifier::UTurn => FfiManeuverModifier::UTurn,
    }
}

fn ffi_step_of(s: guidance::Step) -> FfiStep {
    FfiStep {
        maneuver: ffi_maneuver_of(s.maneuver),
        modifier: ffi_maneuver_modifier_of(s.modifier),
        exit_count: s.exit_count,
        name: s.name,
        road_ref: s.road_ref,
        dest: s.dest,
        dest_ref: s.dest_ref,
        exit_ref: s.exit_ref,
        lat: s.lat,
        lon: s.lon,
        dist_from_leg_start_m: s.dist_from_leg_start_m,
    }
}

/// One `guidance::steps_for_route` call per Leg, mapped to the wire shape.
/// Needs `&Rpack`, so like `build_polyline` this is exercised by the Swift
/// golden test rather than a Rust unit test.
pub fn build_leg_steps(pack: &Rpack, plan: &Plan) -> Vec<Vec<FfiStep>> {
    plan.legs
        .iter()
        .map(|leg| {
            guidance::steps_for_route(pack, &leg.route_edges)
                .into_iter()
                .map(ffi_step_of)
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use optimiser::{Endpoint, LegFlag, PlanFlag, PlanLeg, Stop};

    #[test]
    fn vehicle_of_maps_both_variants() {
        assert_eq!(
            vehicle_of(FfiVehicle::Ioniq5Lr2wd).mass_kg,
            VehicleModel::ioniq5_lr_2wd().mass_kg
        );
        assert_eq!(
            vehicle_of(FfiVehicle::Ioniq5LrAwd).mass_kg,
            VehicleModel::ioniq5_lr_awd().mass_kg
        );
    }

    #[test]
    fn calibration_of_defaults_when_reference_consumption_is_none() {
        let veh = VehicleModel::ioniq5_lr_2wd();
        assert_eq!(calibration_of(&veh, None), Calibration::default());
    }

    #[test]
    fn calibration_of_derives_from_reference_consumption_when_set() {
        let veh = VehicleModel::ioniq5_lr_2wd();
        let calib = calibration_of(&veh, Some(180.0));
        assert_eq!(calib, Calibration::from_reference_consumption(&veh, 180.0));
    }

    #[test]
    fn validate_cpack_format_accepts_cpack1_and_rejects_others() {
        assert!(validate_cpack_format("cpack-1").is_ok());
        assert!(matches!(
            validate_cpack_format("cpack-2"),
            Err(PlannerError::InvalidRequest { .. })
        ));
    }

    #[test]
    fn map_assemble_error_routes_connectivity_failures_to_no_route_found() {
        assert!(matches!(
            map_assemble_error(AssembleError::SnapFailed { lat: 1.0, lon: 2.0 }),
            PlannerError::NoRouteFound { .. }
        ));
        assert!(matches!(
            map_assemble_error(AssembleError::NoRoute {
                from: (1.0, 2.0),
                to: (3.0, 4.0)
            }),
            PlannerError::NoRouteFound { .. }
        ));
    }

    #[test]
    fn map_assemble_error_passes_cancelled_through() {
        assert!(matches!(
            map_assemble_error(AssembleError::Cancelled),
            PlannerError::Cancelled { .. }
        ));
    }

    /// A tiny two-leg, one-stop Plan built directly (no Rpack/Router
    /// needed): Origin -> Charger -> Dest.
    fn tiny_plan() -> Plan {
        let sites = vec![ChargerSite {
            id: "ndw:1".to_string(),
            name: "Fastned Test".to_string(),
            lat: 50.0,
            lon: 5.0,
            power_kw: 150.0,
        }];
        Plan {
            legs: vec![
                PlanLeg {
                    from: Endpoint::Origin,
                    to: Endpoint::Charger { site: 0 },
                    drive_s: 3_600.0,
                    dist_m: 100_000.0,
                    energy_wh: 18_000.0,
                    speed_cap_kmh: None,
                    depart_soc: 0.9,
                    arrival_soc: 0.2,
                    flags: vec![],
                    route_edges: vec![],
                },
                PlanLeg {
                    from: Endpoint::Charger { site: 0 },
                    to: Endpoint::Dest,
                    drive_s: 1_800.0,
                    dist_m: 50_000.0,
                    energy_wh: 9_000.0,
                    speed_cap_kmh: Some(110.0),
                    depart_soc: 0.8,
                    arrival_soc: 0.5,
                    flags: vec![LegFlag::ArrivalSocBelowWanted],
                    route_edges: vec![],
                },
            ],
            stops: vec![Stop {
                site: 0,
                arrival_soc: 0.2,
                depart_soc: 0.8,
                charge_s: 900.0,
            }],
            sites,
            drive_time_s: 5_400.0,
            charge_time_s: 900.0,
            total_time_s: 6_300.0,
            total_dist_m: 150_000.0,
            flags: vec![PlanFlag::ArrivalSocBelowWanted],
            alternative: None,
        }
    }

    #[test]
    fn ffi_plan_of_labels_endpoints_and_carries_totals() {
        let plan = tiny_plan();
        let ffi = ffi_plan_of(&plan, vec![], vec![], vec![], None);

        assert_eq!(ffi.legs.len(), 2);
        assert_eq!(ffi.legs[0].from_label, "Origin");
        assert_eq!(ffi.legs[0].to_label, "Fastned Test");
        assert_eq!(ffi.legs[1].from_label, "Fastned Test");
        assert_eq!(ffi.legs[1].to_label, "Dest");
        assert_eq!(ffi.legs[1].speed_cap_kmh, Some(110.0));
        assert_eq!(ffi.legs[1].flags, vec!["ArrivalSocBelowWanted".to_string()]);

        assert_eq!(ffi.stops.len(), 1);
        assert_eq!(ffi.stops[0].name, "Fastned Test");
        assert_eq!(ffi.stops[0].charger_id, "ndw:1");
        assert_eq!(ffi.stops[0].charge_s, 900.0);

        assert_eq!(ffi.total_time_s, 6_300.0);
        assert_eq!(ffi.total_dist_m, 150_000.0);
        assert_eq!(ffi.flags, vec!["ArrivalSocBelowWanted".to_string()]);
        assert!(ffi.alternative.is_none());
    }

    #[test]
    fn build_soc_curve_samples_every_2km_and_jumps_at_the_stop() {
        let plan = tiny_plan();
        let curve = build_soc_curve(&plan);

        // First leg: depart 0.9 -> arrival 0.2 over 100km, sampled every 2km
        // (51 points at 0, 2000, ..., 100000) plus the Charging Stop's jump
        // to 0.8; second leg: depart 0.8 -> arrival 0.5 over 50km (26 points
        // at 100000, 102000, ..., 150000).
        assert_eq!(
            curve.first(),
            Some(&FfiSocPoint {
                dist_m: 0.0,
                soc: 0.9
            })
        );
        let stop_arrival = curve
            .iter()
            .find(|p| p.dist_m == 100_000.0 && (p.soc - 0.2).abs() < 1e-9)
            .expect("arrival-at-stop point present");
        let stop_depart = curve
            .iter()
            .find(|p| p.dist_m == 100_000.0 && (p.soc - 0.8).abs() < 1e-9)
            .expect("post-charge jump point present");
        assert_eq!(stop_arrival.dist_m, stop_depart.dist_m);
        assert_eq!(
            curve.last(),
            Some(&FfiSocPoint {
                dist_m: 150_000.0,
                soc: 0.5
            })
        );

        // Monotone-decreasing within each leg, ignoring the one jump.
        let mut prev: Option<f64> = None;
        for p in &curve {
            if let Some(prev_soc) = prev {
                if p.soc > prev_soc + 1e-9 {
                    // Only the post-charge jump may increase SoC.
                    assert!(
                        (p.soc - 0.8).abs() < 1e-9,
                        "unexpected SoC increase at {p:?}"
                    );
                }
            }
            prev = Some(p.soc);
        }
    }

    #[test]
    fn simplify_dp_preserves_endpoints() {
        let points = vec![
            FfiGeoPoint {
                lat: 50.0000,
                lon: 5.0000,
            },
            FfiGeoPoint {
                lat: 50.0001,
                lon: 5.0005,
            },
            FfiGeoPoint {
                lat: 50.0002,
                lon: 5.0010,
            },
            FfiGeoPoint {
                lat: 50.0100,
                lon: 5.0200,
            },
        ];
        let simplified = simplify_dp(&points, 3.0);
        assert_eq!(simplified.first(), points.first());
        assert_eq!(simplified.last(), points.last());
    }

    #[test]
    fn simplify_dp_collapses_collinear_points_to_two() {
        // Points on an exact straight line (constant lat): zero perpendicular
        // distance from every intermediate point to the endpoint-to-endpoint line.
        let points: Vec<FfiGeoPoint> = (0..10)
            .map(|i| FfiGeoPoint {
                lat: 50.0,
                lon: 5.0 + i as f64 * 0.001,
            })
            .collect();
        let simplified = simplify_dp(&points, 3.0);
        assert_eq!(simplified, vec![points[0], points[9]]);
    }

    #[test]
    fn simplify_dp_keeps_a_right_angle_corner_at_any_tolerance_up_to_its_offset() {
        let a = FfiGeoPoint {
            lat: 50.0,
            lon: 5.00,
        };
        let corner = FfiGeoPoint {
            lat: 50.00018,
            lon: 5.005,
        };
        let c = FfiGeoPoint {
            lat: 50.0,
            lon: 5.02,
        };
        let points = vec![a, corner, c];

        let offset_m = perpendicular_distance_m(&corner, &a, &c);
        assert!(
            offset_m > 1.0,
            "test fixture's corner offset should be well above float noise: {offset_m}"
        );

        // Tolerance exactly at the corner's own offset: the ">=" split threshold
        // means it still survives.
        let simplified = simplify_dp(&points, offset_m);
        assert_eq!(
            simplified, points,
            "corner should survive when tolerance == its own offset"
        );

        // And at any tighter tolerance too.
        let simplified_tighter = simplify_dp(&points, offset_m * 0.5);
        assert_eq!(
            simplified_tighter, points,
            "corner should survive at a tighter tolerance too"
        );
    }

    #[test]
    fn simplify_dp_keeps_a_collinear_overshoot_hairpin() {
        // An out-and-back collinear with the chord (a hairpin overshooting past the
        // endpoint): zero distance to the INFINITE line through the endpoints, so
        // line-based DP would drop it -- segment-clamped distance must keep it.
        let a = FfiGeoPoint {
            lat: 50.0,
            lon: 5.000,
        };
        let overshoot = FfiGeoPoint {
            lat: 50.0,
            lon: 5.003,
        };
        let b = FfiGeoPoint {
            lat: 50.0,
            lon: 5.001,
        };
        let simplified = simplify_dp(&[a, overshoot, b], 3.0);
        assert_eq!(
            simplified,
            vec![a, overshoot, b],
            "a collinear overshoot past the chord's end must survive simplification"
        );
    }

    #[test]
    fn simplify_dp_dropped_points_deviate_no_more_than_tolerance() {
        const TOLERANCE_M: f64 = 3.0;
        // A synthetic zigzag: small lat wiggles (sub-tolerance) alternating with
        // one large excursion that must survive simplification.
        let points: Vec<FfiGeoPoint> = vec![
            FfiGeoPoint {
                lat: 50.000000,
                lon: 5.0000,
            },
            FfiGeoPoint {
                lat: 50.000005,
                lon: 5.0010,
            }, // ~0.56m wiggle
            FfiGeoPoint {
                lat: 50.000000,
                lon: 5.0020,
            },
            FfiGeoPoint {
                lat: 50.000200,
                lon: 5.0030,
            }, // ~22m excursion, must survive
            FfiGeoPoint {
                lat: 50.000000,
                lon: 5.0040,
            },
            FfiGeoPoint {
                lat: 50.000004,
                lon: 5.0050,
            }, // ~0.45m wiggle
            FfiGeoPoint {
                lat: 50.000000,
                lon: 5.0060,
            },
        ];
        let simplified = simplify_dp(&points, TOLERANCE_M);

        assert_eq!(simplified.first(), points.first());
        assert_eq!(simplified.last(), points.last());
        assert!(
            simplified.contains(&points[3]),
            "the large excursion must survive simplification"
        );

        // Every dropped point must deviate no more than TOLERANCE_M from the
        // line segment connecting its enclosing surviving neighbors.
        let mut seg = 0usize;
        for p in &points {
            if seg + 1 < simplified.len() && *p == simplified[seg + 1] {
                seg += 1;
            }
            if seg + 1 >= simplified.len() {
                break;
            }
            let dist = perpendicular_distance_m(p, &simplified[seg], &simplified[seg + 1]);
            assert!(
                dist <= TOLERANCE_M + 1e-9,
                "point {p:?} deviates {dist}m from its enclosing simplified segment (tolerance {TOLERANCE_M}m)"
            );
        }
    }
}
