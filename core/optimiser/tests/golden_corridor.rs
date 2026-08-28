//! The golden-plan gate (wayfinder #33): needs the real
//! `~/abrp-data/dist/corridor` artifacts, so every test here is `#[ignore]`,
//! analogous to #31's local-only corridor validation.
//!
//! Each test calls `optimiser::plan` end-to-end (corridor assembly + the
//! label-setting search) and pins the resulting `Plan` stop-by-stop via
//! `dump_plan`. Tolerances: stop site NAMES exact, stop count exact, SoCs
//! within ±0.02, `charge_s` within ±10%, totals within ±5%, `speed_cap_kmh`
//! exact (the caps are a discrete set, ADR 0010 point 1).

use energy::{Calibration, VehicleModel};
use optimiser::types::{Endpoint, Plan, WaypointSpec};
use optimiser::{plan, PlanRequest};
use packs::Rpack;
use routing::Router;
use std::time::Instant;

const LUXEMBOURG: (f64, f64) = (49.6116, 6.1319);
const AMSTERDAM: (f64, f64) = (52.3702, 4.8952);
const ANTWERP: (f64, f64) = (51.2194, 4.4025);

fn dist_root() -> std::path::PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set to find ~/abrp-data/dist/corridor");
    std::path::PathBuf::from(home).join("abrp-data/dist/corridor")
}

fn open_fixture() -> (Rpack, Vec<optimiser::types::ChargerSite>) {
    let root = dist_root();
    let pack = Rpack::open(root.join("corridor.rpack")).expect("open corridor.rpack");
    let json =
        std::fs::read(root.join("corridor-chargers.json")).expect("read corridor-chargers.json");
    let sites = optimiser::parse_cpack(&json).expect("parse corridor-chargers.json");
    (pack, sites)
}

fn endpoint_label(plan: &Plan, ep: &Endpoint) -> String {
    match ep {
        Endpoint::Origin => "Origin".to_string(),
        Endpoint::Dest => "Dest".to_string(),
        Endpoint::Waypoint { wp } => format!("Waypoint[{wp}]"),
        Endpoint::Charger { site } => plan.sites[*site as usize].name.clone(),
    }
}

fn stop_site_name(plan: &Plan, stop_idx: usize) -> &str {
    &plan.sites[plan.stops[stop_idx].site as usize].name
}

/// Prints a `Plan` stop-by-stop: every driven Leg (endpoints, cap, dist,
/// energy, SoC in/out) interleaved implicitly with the Stops list (site
/// name, SoC in/out, charge duration), then totals and flags. This is what
/// a human re-running the ignored tests reads to re-pin the golden
/// constants below.
fn dump_plan(label: &str, plan: &Plan) {
    println!("=== {label} ===");
    for (i, leg) in plan.legs.iter().enumerate() {
        println!(
            "  leg {i}: {} -> {} cap={:?} dist_m={:.0} energy_wh={:.0} drive_s={:.0} soc {:.3}->{:.3} flags={:?}",
            endpoint_label(plan, &leg.from),
            endpoint_label(plan, &leg.to),
            leg.speed_cap_kmh,
            leg.dist_m,
            leg.energy_wh,
            leg.drive_s,
            leg.depart_soc,
            leg.arrival_soc,
            leg.flags
        );
    }
    for (i, stop) in plan.stops.iter().enumerate() {
        let site = &plan.sites[stop.site as usize];
        println!(
            "  stop {i}: {} soc {:.3}->{:.3} charge_s={:.0} ({:.1} min)",
            site.name,
            stop.arrival_soc,
            stop.depart_soc,
            stop.charge_s,
            stop.charge_s / 60.0
        );
    }
    println!(
        "  totals: drive_s={:.0} charge_s={:.0} total_s={:.0} dist_m={:.0} flags={:?}",
        plan.drive_time_s, plan.charge_time_s, plan.total_time_s, plan.total_dist_m, plan.flags
    );
    if let Some(alt) = &plan.alternative {
        println!("  -- alternative plan --");
        dump_plan(&format!("{label} (alternative)"), alt);
    }
}

fn base_params(depart_soc: f64) -> (f64, f64, f64, f64, f64, f64) {
    // (depart_soc, arrival_min_soc, charger_arrival_min_soc, charger_max_soc, stops_bias, battery_warmth)
    (depart_soc, 0.10, 0.10, 0.80, 1.0, 1.0)
}

fn request(
    origin: (f64, f64),
    waypoints: Vec<WaypointSpec>,
    dest: (f64, f64),
    depart_soc: f64,
) -> PlanRequest {
    let (
        depart_soc,
        arrival_min_soc,
        charger_arrival_min_soc,
        charger_max_soc,
        stops_bias,
        battery_warmth,
    ) = base_params(depart_soc);
    PlanRequest {
        corridor: optimiser::CorridorRequest {
            origin,
            waypoints,
            dest,
            temp_c: 20.0,
            headwind_ms: 0.0,
        },
        depart_soc,
        arrival_min_soc,
        charger_arrival_min_soc,
        charger_max_soc,
        stops_bias,
        battery_warmth,
        offer_stop_free_alternative: false,
    }
}

/// Asserts a fraction is within `tol` of `expected` (SoC tolerance ±0.02).
fn assert_soc_close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= 0.02,
        "{what}: expected {expected:.3} +/- 0.02, got {actual:.3}"
    );
}

/// Asserts `actual` is within `pct` percent of `expected`.
fn assert_pct_close(actual: f64, expected: f64, pct: f64, what: &str) {
    let tol = expected.abs() * pct / 100.0;
    assert!(
        (actual - expected).abs() <= tol,
        "{what}: expected {expected:.1} +/- {pct}%, got {actual:.1}"
    );
}

#[test]
#[ignore = "needs ~/abrp-data/dist/corridor artifacts (local-only, like #31's corridor validation)"]
fn golden_lu_amsterdam() {
    let (pack, sites) = open_fixture();
    let router = Router::new(&pack);
    let veh = VehicleModel::ioniq5_lr_2wd();
    let calib = Calibration::default();
    let req = request(LUXEMBOURG, vec![], AMSTERDAM, 0.90);

    let (result, _stats) = plan(&pack, &router, &sites, &veh, &calib, &req).unwrap();
    dump_plan("golden_lu_amsterdam", &result);

    // Pinned observed values (run 2026-08-28 against corridor.rpack /
    // corridor-chargers.json). Reference: LU->Amsterdam, depart 0.90,
    // floors 0.10/0.10, max 0.80, bias 1.0, warm 1.0, 20C: total 14830s
    // (drive 13864 + charge 966), 414.9km, 1 stop at "Hyperfast charge
    // laadpalen Nossegem Zaventem" (396 kW), soc 0.129->0.765, 966s.
    assert_eq!(result.stops.len(), 1);
    assert_eq!(
        stop_site_name(&result, 0),
        "Hyperfast charge laadpalen Nossegem Zaventem"
    );
    assert_soc_close(result.stops[0].arrival_soc, 0.129, "stop 0 arrival soc");
    assert_soc_close(result.stops[0].depart_soc, 0.765, "stop 0 depart soc");
    assert_pct_close(result.stops[0].charge_s, 966.0, 10.0, "stop 0 charge_s");
    assert_pct_close(result.drive_time_s, 13864.0, 5.0, "drive_time_s");
    assert_pct_close(result.charge_time_s, 966.0, 10.0, "charge_time_s");
    assert_pct_close(result.total_time_s, 14830.0, 5.0, "total_time_s");
    assert_pct_close(result.total_dist_m, 414_900.0, 5.0, "total_dist_m");
    assert!(
        result.flags.is_empty(),
        "expected a feasible plan: {:?}",
        result.flags
    );
}

#[test]
#[ignore = "needs ~/abrp-data/dist/corridor artifacts (local-only, like #31's corridor validation)"]
fn golden_lu_antwerp_stop_free_under_speed_caps() {
    // The ADR 0010 motivating case (the planner-UI prototype's absurd
    // "+7% * 1min" micro-stop) resolved: Speed Caps let the winning Plan
    // skip the stop entirely rather than take a sub-10-minute charge.
    let (pack, sites) = open_fixture();
    let router = Router::new(&pack);
    let veh = VehicleModel::ioniq5_lr_2wd();
    let calib = Calibration::default();
    let req = request(LUXEMBOURG, vec![], ANTWERP, 0.90);

    let (result, _stats) = plan(&pack, &router, &sites, &veh, &calib, &req).unwrap();
    dump_plan("golden_lu_antwerp_stop_free_under_speed_caps", &result);

    // Pinned observed values. Reference: LU->Antwerp depart 0.90:
    // stop-free, leg 1 cap Some(90.0) to "SuperChargy - Aire de Capellen
    // direction Arlon", leg 2 cap Some(110.0) to Dest, arrival soc 0.101,
    // total 8767s.
    assert!(
        result.stops.is_empty(),
        "expected the stop-free Plan (ADR 0010 point 3 ramp penalty), got stops: {:?}",
        result.stops
    );
    assert!(
        !result.legs.iter().any(|l| l.drive_s < 600.0
            && matches!(l.to, Endpoint::Charger { .. })
            && result.stops.iter().any(|s| s.charge_s < 600.0)),
        "no sub-10-minute Charging Stop should survive (ADR 0010 point 3)"
    );
    assert_eq!(result.legs.len(), 2);
    assert_eq!(result.legs[0].speed_cap_kmh, Some(90.0));
    assert_eq!(
        endpoint_label(&result, &result.legs[0].to),
        "SuperChargy - Aire de Capellen direction Arlon"
    );
    assert_eq!(result.legs[1].speed_cap_kmh, Some(110.0));
    assert!(matches!(result.legs[1].to, Endpoint::Dest));
    assert_soc_close(result.legs[1].arrival_soc, 0.101, "arrival soc");
    assert_pct_close(result.total_time_s, 8767.0, 5.0, "total_time_s");
}

#[test]
#[ignore = "needs ~/abrp-data/dist/corridor artifacts (local-only, like #31's corridor validation)"]
fn golden_speed_cap_exercised() {
    // Depart low enough (0.30) that a stop is unavoidable; also exercises
    // the entry-node forward-gap/fanout exemption (a low-SoC departure
    // needs a charger closer than FORWARD_GAP_M away).
    let (pack, sites) = open_fixture();
    let router = Router::new(&pack);
    let veh = VehicleModel::ioniq5_lr_2wd();
    let calib = Calibration::default();
    let req = request(LUXEMBOURG, vec![], ANTWERP, 0.30);

    let (result, _stats) = plan(&pack, &router, &sites, &veh, &calib, &req).unwrap();
    dump_plan("golden_speed_cap_exercised", &result);

    // Pinned observed values. Reference: LU->Antwerp depart 0.30: 1 stop
    // at "SuperChargy - Aire de Capellen direction Arlon" (160 kW) soc
    // 0.247->0.800 (993s, 16.6 min), then a Some(100.0)-capped leg,
    // arriving 0.111.
    assert_eq!(result.stops.len(), 1);
    assert_eq!(
        stop_site_name(&result, 0),
        "SuperChargy - Aire de Capellen direction Arlon"
    );
    assert_soc_close(result.stops[0].arrival_soc, 0.247, "stop 0 arrival soc");
    assert_soc_close(result.stops[0].depart_soc, 0.800, "stop 0 depart soc");
    assert_pct_close(result.stops[0].charge_s, 993.0, 10.0, "stop 0 charge_s");

    let capped_leg = result.legs.last().expect("at least one leg after the stop");
    assert_eq!(capped_leg.speed_cap_kmh, Some(100.0));
    assert!(matches!(capped_leg.to, Endpoint::Dest));
    assert_soc_close(capped_leg.arrival_soc, 0.111, "final arrival soc");
}

#[test]
#[ignore = "needs ~/abrp-data/dist/corridor artifacts (local-only, like #31's corridor validation)"]
fn golden_waypoint() {
    let (pack, sites) = open_fixture();
    let router = Router::new(&pack);
    let veh = VehicleModel::ioniq5_lr_2wd();
    let calib = Calibration::default();
    let req = request(
        LUXEMBOURG,
        vec![WaypointSpec {
            lat: ANTWERP.0,
            lon: ANTWERP.1,
            depart_soc_override: None,
        }],
        AMSTERDAM,
        0.90,
    );

    let (result, _stats) = plan(&pack, &router, &sites, &veh, &calib, &req).unwrap();
    dump_plan("golden_waypoint", &result);

    // Leg-sequence shape: the Waypoint endpoint appears strictly between
    // Origin and Dest (ADR 0010 point 4: waypoint segments are structural,
    // never skippable).
    let kinds: Vec<&str> = result
        .legs
        .iter()
        .flat_map(|l| [&l.from, &l.to])
        .map(|ep| match ep {
            Endpoint::Origin => "Origin",
            Endpoint::Dest => "Dest",
            Endpoint::Waypoint { .. } => "Waypoint",
            Endpoint::Charger { .. } => "Charger",
        })
        .collect();
    assert_eq!(kinds.first(), Some(&"Origin"));
    assert_eq!(kinds.last(), Some(&"Dest"));
    let wp_pos = kinds
        .iter()
        .position(|k| *k == "Waypoint")
        .expect("Waypoint endpoint must appear in the leg sequence");
    assert!(wp_pos > 0 && wp_pos < kinds.len() - 1);

    // Pinned observed values. Reference: LU->Amsterdam via Antwerp
    // waypoint (no override), depart 0.90: 1 stop (Nossegem 0.129->0.800,
    // 1033s), legs pass Waypoint#0 between Origin and Dest, arrive 0.174,
    // total 15449s.
    assert_eq!(result.stops.len(), 1);
    assert_eq!(
        stop_site_name(&result, 0),
        "Hyperfast charge laadpalen Nossegem Zaventem"
    );
    assert_soc_close(result.stops[0].arrival_soc, 0.129, "stop 0 arrival soc");
    assert_soc_close(result.stops[0].depart_soc, 0.800, "stop 0 depart soc");
    assert_pct_close(result.stops[0].charge_s, 1033.0, 10.0, "stop 0 charge_s");
    let last_leg = result.legs.last().unwrap();
    assert_soc_close(last_leg.arrival_soc, 0.174, "final arrival soc");
    assert_pct_close(result.total_time_s, 15449.0, 5.0, "total_time_s");
}

#[test]
#[ignore = "needs ~/abrp-data/dist/corridor artifacts (local-only, like #31's corridor validation)"]
fn perf_warm_plan() {
    // ADR 0001's <1s "warm" bar, exercised on the real top-level entry
    // point: one warm-up `plan()` call (discarded), then a timed second
    // call.
    let (pack, sites) = open_fixture();
    let router = Router::new(&pack);
    let veh = VehicleModel::ioniq5_lr_2wd();
    let calib = Calibration::default();
    let req = request(LUXEMBOURG, vec![], AMSTERDAM, 0.90);

    let _ = plan(&pack, &router, &sites, &veh, &calib, &req).unwrap();

    let t1 = Instant::now();
    let _ = plan(&pack, &router, &sites, &veh, &calib, &req).unwrap();
    let warm_ms = t1.elapsed().as_secs_f64() * 1000.0;

    println!("perf_warm_plan: warm_plan_ms={warm_ms:.1}");
    assert!(
        warm_ms < 1000.0,
        "warm plan() took {warm_ms:.1}ms, expected < 1000ms"
    );
}
