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
    FfiGeoPoint, FfiLeg, FfiPlan, FfiPlanAlt, FfiPlanRequest, FfiSocPoint, FfiStop, FfiVehicle,
    FfiWaypoint,
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

fn ffi_leg_of(plan: &Plan, leg: &PlanLeg) -> FfiLeg {
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
    }
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

/// Builds the non-recursive alternative-plan record; `polyline`/`soc_curve`
/// are the alternative `Plan`'s own (built by the caller, which has the
/// `&Rpack` this module deliberately doesn't need).
pub fn ffi_plan_alt_of(
    plan: &Plan,
    polyline: Vec<FfiGeoPoint>,
    soc_curve: Vec<FfiSocPoint>,
) -> FfiPlanAlt {
    FfiPlanAlt {
        legs: plan.legs.iter().map(|l| ffi_leg_of(plan, l)).collect(),
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
    alternative: Option<FfiPlanAlt>,
) -> FfiPlan {
    FfiPlan {
        legs: plan.legs.iter().map(|l| ffi_leg_of(plan, l)).collect(),
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

/// Concatenates each Leg's `route_edges` geometry (dropping the duplicated
/// junction vertex between consecutive edges within a Leg), downsampled to
/// ~every 5th point while always keeping each Leg's first and last vertex,
/// to bound the `RustBuffer` (ADR 0004 point 3). Needs `&Rpack`, so unlike
/// the rest of this module it is exercised by the Swift golden test rather
/// than a Rust unit test.
pub fn build_polyline(pack: &Rpack, plan: &Plan) -> Vec<FfiGeoPoint> {
    const KEEP_EVERY: usize = 5;
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
        let Some(last_idx) = leg_pts.len().checked_sub(1) else {
            continue;
        };
        for (i, p) in leg_pts.into_iter().enumerate() {
            if i != 0 && i != last_idx && i % KEEP_EVERY != 0 {
                continue;
            }
            if out.last() == Some(&p) {
                continue;
            }
            out.push(p);
        }
    }

    out
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
        let ffi = ffi_plan_of(&plan, vec![], vec![], None);

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
}
