//! Dev-only milestone-gate measurement tool: exercises the exact production
//! planning path the iOS app calls (`Planner::new` -> `load_chargers` ->
//! `plan`) against a pipeline-built corridor pack, printing timings and
//! Plan shape. Not a production code path.

use std::time::Instant;

use planner_ffi::{FfiPlan, FfiPlanRequest, FfiVehicle, Planner};

fn main() {
    let t_start = Instant::now();

    let mut args = std::env::args().skip(1);
    let pack_path = args
        .next()
        .expect("usage: plan_cli <region.rpack> <chargers.json>");
    let chargers_path = args
        .next()
        .expect("usage: plan_cli <region.rpack> <chargers.json>");

    let t0 = Instant::now();
    let planner = Planner::new(pack_path).expect("failed to open region pack");
    println!("pack open: {:.1}ms", t0.elapsed().as_secs_f64() * 1000.0);

    let t0 = Instant::now();
    let bytes = std::fs::read(&chargers_path).expect("failed to read chargers file");
    let count = planner
        .load_chargers(bytes, "cpack-1".to_string())
        .expect("failed to load chargers");
    println!(
        "charger load: {:.1}ms ({count} chargers)",
        t0.elapsed().as_secs_f64() * 1000.0
    );

    // LU -> Amsterdam golden request (matches
    // core/optimiser/tests/golden_corridor.rs::golden_lu_amsterdam and
    // app/PlannerKit/Tests/PlannerKitTests/GoldenPlanTests.swift exactly).
    let request = FfiPlanRequest {
        origin_lat: 49.6116,
        origin_lon: 6.1319,
        dest_lat: 52.3702,
        dest_lon: 4.8952,
        waypoints: vec![],
        depart_soc: 0.90,
        arrival_min_soc: 0.10,
        charger_arrival_min_soc: 0.10,
        charger_max_soc: 0.80,
        stops_bias: 1.0,
        temp_c: 20.0,
        headwind_ms: 0.0,
        battery_warmth: 1.0,
        offer_stop_free_alternative: false,
        vehicle: FfiVehicle::Ioniq5Lr2wd,
        reference_consumption_wh_per_km: None,
    };

    let t0 = Instant::now();
    let plan = planner.plan(request.clone()).expect("first plan() failed");
    println!(
        "first plan() (cold): {:.1}ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );
    println!(
        "cold start to first plan: {:.1}ms",
        t_start.elapsed().as_secs_f64() * 1000.0
    );

    for i in 1..=3 {
        let t0 = Instant::now();
        let _ = planner.plan(request.clone()).expect("warm plan() failed");
        println!(
            "plan() warm #{i}: {:.1}ms",
            t0.elapsed().as_secs_f64() * 1000.0
        );
    }

    // The slider-replan case the cross-call corridor cache (issue #38)
    // exists for: only `depart_soc` changes, so this should skip corridor
    // assembly and go straight to `search::solve`.
    let mut slider_request = request.clone();
    slider_request.depart_soc = 0.85;
    let t0 = Instant::now();
    let _ = planner
        .plan(slider_request)
        .expect("warm plan() soc=0.85 failed");
    println!(
        "plan() warm soc=0.85: {:.1}ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );

    print_plan_shape(&plan);
}

fn print_plan_shape(plan: &FfiPlan) {
    let arrival_soc = plan
        .legs
        .last()
        .map(|leg| leg.arrival_soc)
        .unwrap_or(f64::NAN);
    println!("--- plan shape ---");
    println!("legs: {}", plan.legs.len());
    println!("arrival soc: {arrival_soc:.3}");
    println!("total duration s: {:.0}", plan.total_time_s);
    println!("total distance m: {:.0}", plan.total_dist_m);
    println!("stops: {}", plan.stops.len());
    for stop in &plan.stops {
        println!(
            "  stop: {} power_kw={:.0} soc {:.3}->{:.3} charge_s={:.0}",
            stop.name, stop.power_kw, stop.arrival_soc, stop.depart_soc, stop.charge_s
        );
    }
    println!("polyline points: {}", plan.polyline.len());
    println!("soc curve points: {}", plan.soc_curve.len());
    println!(
        "stop-free alternative present: {}",
        plan.alternative.is_some()
    );
}
