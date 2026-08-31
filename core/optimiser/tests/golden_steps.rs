//! The golden-step gate (wayfinder #66): needs the real
//! `~/abrp-data/dist-v2/corridor` artifacts (format 2.0 -- `dist/corridor`
//! is still v1 until #69, so it carries no guidance sections at all), so
//! every test here is `#[ignore]`, same as `golden_corridor.rs`.
//!
//! Each test plans the same LU -> {Amsterdam, Antwerp} routes as
//! `golden_corridor.rs`, derives steps per Leg via
//! `guidance::steps_for_route`, and pins them via `dump_steps`.

use std::time::Instant;

use energy::{Calibration, VehicleModel};
use guidance::Step;
use optimiser::types::Plan;
use optimiser::{plan, PlanRequest};
use packs::Rpack;
use routing::Router;

const LUXEMBOURG: (f64, f64) = (49.6116, 6.1319);
const AMSTERDAM: (f64, f64) = (52.3702, 4.8952);
const ANTWERP: (f64, f64) = (51.2194, 4.4025);

fn dist_root() -> std::path::PathBuf {
    let home =
        std::env::var("HOME").expect("HOME must be set to find ~/abrp-data/dist-v2/corridor");
    std::path::PathBuf::from(home).join("abrp-data/dist-v2/corridor")
}

fn open_fixture() -> (Rpack, Vec<optimiser::types::ChargerSite>) {
    let root = dist_root();
    let pack = Rpack::open(root.join("corridor.rpack")).expect("open corridor.rpack");
    assert!(
        pack.has_guidance(),
        "dist-v2/corridor.rpack must be a format 2.0 pack with guidance sections"
    );
    let json =
        std::fs::read(root.join("corridor-chargers.json")).expect("read corridor-chargers.json");
    let sites = optimiser::parse_cpack(&json).expect("parse corridor-chargers.json");
    (pack, sites)
}

fn request(origin: (f64, f64), dest: (f64, f64), depart_soc: f64) -> PlanRequest {
    PlanRequest {
        corridor: optimiser::CorridorRequest {
            origin,
            waypoints: vec![],
            dest,
            temp_c: 20.0,
            headwind_ms: 0.0,
        },
        depart_soc,
        arrival_min_soc: 0.10,
        charger_arrival_min_soc: 0.10,
        charger_max_soc: 0.80,
        stops_bias: 1.0,
        battery_warmth: 1.0,
        offer_stop_free_alternative: false,
    }
}

/// Prints every step of every Leg -- the re-pin artifact a human reads to
/// re-pin the golden constants below.
fn dump_steps(label: &str, steps_per_leg: &[Vec<Step>]) {
    println!("=== {label} ===");
    for (leg_idx, steps) in steps_per_leg.iter().enumerate() {
        for (i, s) in steps.iter().enumerate() {
            println!(
                "  leg {leg_idx} step {i}: {:?}/{:?} exit={:?} dist={:.0}m at=({:.5},{:.5}) name={:?} ref={:?} dest={:?} exit_ref={:?}",
                s.maneuver, s.modifier, s.exit_count, s.dist_from_leg_start_m, s.lat, s.lon, s.name, s.road_ref, s.dest, s.exit_ref
            );
        }
    }
}

/// Structural (always-true) invariants over one Leg's steps: `Depart` is
/// first at dist 0, `Arrive` is last at (approximately) the leg's edge
/// length sum, dists strictly increase, every `Roundabout` step has
/// `exit_count >= 1`, and no `Continue` step carries an empty name.
fn assert_leg_invariants(leg_idx: usize, steps: &[Step]) {
    use guidance::ManeuverType;

    let first = steps
        .first()
        .unwrap_or_else(|| panic!("leg {leg_idx}: no steps"));
    assert_eq!(
        first.maneuver,
        ManeuverType::Depart,
        "leg {leg_idx}: first step must be Depart"
    );
    assert_eq!(
        first.dist_from_leg_start_m, 0.0,
        "leg {leg_idx}: Depart must be at dist 0"
    );

    let last = steps.last().expect("checked non-empty above");
    assert_eq!(
        last.maneuver,
        ManeuverType::Arrive,
        "leg {leg_idx}: last step must be Arrive"
    );

    for w in steps.windows(2) {
        assert!(
            w[1].dist_from_leg_start_m > w[0].dist_from_leg_start_m,
            "leg {leg_idx}: distances must strictly increase: {:?} -> {:?}",
            w[0],
            w[1]
        );
    }

    for s in steps {
        if s.maneuver == ManeuverType::Roundabout {
            assert!(
                s.exit_count.is_some_and(|c| c >= 1),
                "leg {leg_idx}: every Roundabout step must have exit_count >= 1: {s:?}"
            );
        }
        if s.maneuver == ManeuverType::Continue {
            assert!(
                !s.name.is_empty(),
                "leg {leg_idx}: no Continue step should have an empty name: {s:?}"
            );
        }
    }
}

fn steps_for_plan(pack: &Rpack, plan: &Plan) -> Vec<Vec<Step>> {
    plan.legs
        .iter()
        .map(|leg| guidance::steps_for_route(pack, &leg.route_edges))
        .collect()
}

/// Asserts one pinned landmark step: type, modifier, name/road_ref/dest/
/// exit_ref exactly, distance within +/-10 m.
#[allow(clippy::too_many_arguments)]
fn assert_landmark(
    what: &str,
    step: &Step,
    maneuver: guidance::ManeuverType,
    modifier: guidance::ManeuverModifier,
    name: &str,
    road_ref: &str,
    dest: &str,
    exit_ref: &str,
    dist_m: f64,
) {
    assert_eq!(step.maneuver, maneuver, "{what}: maneuver");
    assert_eq!(step.modifier, modifier, "{what}: modifier");
    assert_eq!(step.name, name, "{what}: name");
    assert_eq!(step.road_ref, road_ref, "{what}: road_ref");
    assert_eq!(step.dest, dest, "{what}: dest");
    assert_eq!(step.exit_ref, exit_ref, "{what}: exit_ref");
    assert!(
        (step.dist_from_leg_start_m - dist_m).abs() <= 10.0,
        "{what}: dist expected {dist_m:.0} +/- 10m, got {:.0}",
        step.dist_from_leg_start_m
    );
}

#[test]
#[ignore = "needs ~/abrp-data/dist-v2/corridor artifacts (local-only, like golden_corridor.rs)"]
fn golden_steps_lu_amsterdam() {
    let (pack, sites) = open_fixture();
    let router = Router::new(&pack);
    let veh = VehicleModel::ioniq5_lr_2wd();
    let calib = Calibration::default();
    let req = request(LUXEMBOURG, AMSTERDAM, 0.90);

    let (result, _stats) = plan(&pack, &router, &sites, &veh, &calib, &req).unwrap();

    let t0 = Instant::now();
    let steps_per_leg = steps_for_plan(&pack, &result);
    let derive_ms = t0.elapsed().as_secs_f64() * 1000.0;

    dump_steps("golden_steps_lu_amsterdam", &steps_per_leg);
    println!(
        "golden_steps_lu_amsterdam: {} legs, {} charging stops",
        result.legs.len(),
        result.stops.len()
    );
    println!("golden_steps_lu_amsterdam: steps_for_route latency = {derive_ms:.2}ms");

    for (i, steps) in steps_per_leg.iter().enumerate() {
        assert_leg_invariants(i, steps);
    }
    assert!(
        derive_ms < 50.0,
        "steps_for_route took {derive_ms:.2}ms, expected < 50ms"
    );

    // Pinned observed values (run 2026-08-31 against dist-v2/corridor.rpack;
    // see the dump above for the full step-by-step re-pin artifact). The
    // Plan itself is identical to golden_corridor.rs's v1 pin -- #65 proved
    // every golden Plan bit-exact across v1/v2 (same route, byte-identical
    // chargers JSON): 1 charging stop (Nossegem Zaventem), but 3 legs,
    // because a Leg boundary falls at every Charger node the route passes
    // through, including PASS-THROUGH chargers with no Stop (see
    // `build_soc_curve`'s comment in core/ffi/src/mapping.rs). Leg count
    // != stop count.
    assert_eq!(
        result.stops.len(),
        1,
        "expected 1 charging stop (v1 golden)"
    );
    assert_eq!(
        steps_per_leg.len(),
        3,
        "expected 3 legs (1 stop + 1 pass-through boundary)"
    );
    assert_eq!(steps_per_leg[0].len(), 10, "leg 0 step count");
    assert_eq!(steps_per_leg[1].len(), 12, "leg 1 step count");
    assert_eq!(steps_per_leg[2].len(), 40, "leg 2 step count");

    assert_landmark(
        "leg 0 onramp onto A6",
        &steps_per_leg[0][6],
        guidance::ManeuverType::OnRamp,
        guidance::ManeuverModifier::Straight,
        "",
        "A 6",
        "Toutes Directions",
        "4",
        3853.0,
    );
    assert_landmark(
        "leg 1 offramp toward Nossegem",
        &steps_per_leg[1][9],
        guidance::ManeuverType::OffRamp,
        guidance::ManeuverModifier::SlightRight,
        "",
        "",
        "Steenokkerzeel;Kortenberg;Nossengem;Sterrebeek",
        "21",
        197994.0,
    );
    assert_landmark(
        "leg 2 roundabout, 2nd exit",
        &steps_per_leg[2][14],
        guidance::ManeuverType::Roundabout,
        guidance::ManeuverModifier::Straight,
        "Oude Rijksweg",
        "",
        "",
        "",
        148477.0,
    );
    assert_eq!(steps_per_leg[2][14].exit_count, Some(2));
    // The A2 keep-left toward Amsterdam: an unnamed motorway split whose
    // branches share the mainline ref -- pinned to lock the motorway-split
    // protection in the same-name fork suppression (guidance rule 4).
    assert_landmark(
        "leg 2 A2 fork toward Amsterdam",
        &steps_per_leg[2][21],
        guidance::ManeuverType::Fork,
        guidance::ManeuverModifier::SlightLeft,
        "",
        "A2",
        "Amsterdam",
        "",
        161769.0,
    );
    assert_landmark(
        "leg 2 final turn onto Rusland",
        &steps_per_leg[2][38],
        guidance::ManeuverType::Turn,
        guidance::ManeuverModifier::Right,
        "Rusland",
        "",
        "",
        "",
        202598.0,
    );
}

#[test]
#[ignore = "needs ~/abrp-data/dist-v2/corridor artifacts (local-only, like golden_corridor.rs)"]
fn golden_steps_lu_antwerp() {
    let (pack, sites) = open_fixture();
    let router = Router::new(&pack);
    let veh = VehicleModel::ioniq5_lr_2wd();
    let calib = Calibration::default();
    let req = request(LUXEMBOURG, ANTWERP, 0.90);

    let (result, _stats) = plan(&pack, &router, &sites, &veh, &calib, &req).unwrap();

    let t0 = Instant::now();
    let steps_per_leg = steps_for_plan(&pack, &result);
    let derive_ms = t0.elapsed().as_secs_f64() * 1000.0;

    dump_steps("golden_steps_lu_antwerp", &steps_per_leg);
    println!(
        "golden_steps_lu_antwerp: {} legs, {} charging stops",
        result.legs.len(),
        result.stops.len()
    );
    println!("golden_steps_lu_antwerp: steps_for_route latency = {derive_ms:.2}ms");

    for (i, steps) in steps_per_leg.iter().enumerate() {
        assert_leg_invariants(i, steps);
    }
    assert!(
        derive_ms < 50.0,
        "steps_for_route took {derive_ms:.2}ms, expected < 50ms"
    );

    // Pinned observed values (run 2026-08-31 against dist-v2/corridor.rpack;
    // see the dump above for the full step-by-step re-pin artifact). The
    // Plan itself is identical to golden_corridor.rs's v1 stop-free pin
    // (#65: bit-exact across v1/v2): 0 charging stops, but 2 legs, because
    // the route passes through a Charger node (Capellen area, ~13 km in)
    // without stopping -- a PASS-THROUGH Leg boundary, not a Stop (see
    // `build_soc_curve`'s comment in core/ffi/src/mapping.rs). Leg count
    // != stop count.
    assert_eq!(result.stops.len(), 0, "expected stop-free (v1 golden)");
    assert_eq!(
        steps_per_leg.len(),
        2,
        "expected 2 legs (stop-free, 1 pass-through boundary)"
    );
    assert_eq!(steps_per_leg[0].len(), 10, "leg 0 step count");
    assert_eq!(steps_per_leg[1].len(), 26, "leg 1 step count");

    assert_landmark(
        "leg 0 onramp onto A6",
        &steps_per_leg[0][6],
        guidance::ManeuverType::OnRamp,
        guidance::ManeuverModifier::Straight,
        "",
        "A 6",
        "Toutes Directions",
        "4",
        3853.0,
    );
    assert_landmark(
        "leg 1 fork toward Bruxelles/Namur",
        &steps_per_leg[1][2],
        guidance::ManeuverType::Fork,
        guidance::ManeuverModifier::SlightLeft,
        "Autoroute des Ardennes",
        "E411",
        "Bruxelles;Namur",
        "",
        46756.0,
    );
    assert_landmark(
        "leg 1 final turn onto Eiermarkt",
        &steps_per_leg[1][24],
        guidance::ManeuverType::Turn,
        guidance::ManeuverModifier::Right,
        "Eiermarkt",
        "",
        "",
        "",
        237921.0,
    );
}
