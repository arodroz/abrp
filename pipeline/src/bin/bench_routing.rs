//! Dev-only benchmark: times `routing::Router` point-to-point and
//! many-to-many queries against a real `.rpack`, and optionally cross-checks
//! CH costs against plain Dijkstra over the base (uncontracted) graph
//! rebuilt from a slice directory. Not a production code path.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use packs::Rpack;
use pipeline::build_base_model;
use routing::{reference, Router};

/// Fixed origin: Luxembourg City.
const ORIGIN_LAT: f32 = 49.6116;
const ORIGIN_LON: f32 = 6.1319;

const DEFAULT_DEST: &str = "52.3676,4.9041"; // Amsterdam

const RELATIVE_TOLERANCE: f64 = 1e-3;

/// Numerical-recipes LCG, matching `pipeline/tests/common::Lcg`, duplicated
/// here since that helper lives in a test-only module this binary can't
/// depend on.
struct Lcg(u64);

impl Lcg {
    fn new(seed: u64) -> Self {
        Lcg(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

struct Args {
    pack: PathBuf,
    base_slice_dir: Option<PathBuf>,
    dest: (f32, f32),
}

fn parse_args() -> Args {
    let mut pack = None;
    let mut base_slice_dir = None;
    let mut dest = DEFAULT_DEST.to_string();

    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut next_value = || {
            args.next()
                .unwrap_or_else(|| panic!("{flag} requires a value"))
        };
        match flag.as_str() {
            "--pack" => pack = Some(PathBuf::from(next_value())),
            "--base-slice-dir" => base_slice_dir = Some(PathBuf::from(next_value())),
            "--dest" => dest = next_value(),
            other => panic!("unknown argument: {other}"),
        }
    }

    let (lat_str, lon_str) = dest
        .split_once(',')
        .unwrap_or_else(|| panic!("--dest must be 'lat,lon', got '{dest}'"));
    let dest = (
        lat_str.trim().parse().expect("--dest lat must be a f32"),
        lon_str.trim().parse().expect("--dest lon must be a f32"),
    );

    Args {
        pack: pack.expect("--pack is required"),
        base_slice_dir,
        dest,
    }
}

/// Snaps `(lat, lon)` to a node, falling back to a brute-force nearest-node
/// scan (and saying so) when the query point falls outside the pack's grid
/// coverage entirely.
fn snap_or_brute_force(pack: &Rpack, lat: f32, lon: f32, label: &str) -> u32 {
    if let Some(id) = pack.snap(lat, lon) {
        return id;
    }
    println!(
        "{label}: snap({lat}, {lon}) returned None (outside grid coverage); falling back to brute-force nearest-node search"
    );
    let cos_lat = (lat as f64).to_radians().cos();
    let mut best = (0u32, f64::MAX);
    for (i, n) in pack.nodes().iter().enumerate() {
        let dlat = (n.lat - lat) as f64;
        let dlon = (n.lon - lon) as f64 * cos_lat;
        let d2 = dlat * dlat + dlon * dlon;
        if d2 < best.1 {
            best = (i as u32, d2);
        }
    }
    best.0
}

/// Generates `count` distinct node ids by snapping seeded-LCG random
/// coordinates drawn from the pack's own bounding box, skipping anything in
/// `exclude` or already produced, until `count` distinct ids are found.
fn distinct_snapped_nodes(pack: &Rpack, seed: u64, count: usize, exclude: &[u32]) -> Vec<u32> {
    let nodes = pack.nodes();
    let min_lat = nodes.iter().map(|n| n.lat).fold(f32::INFINITY, f32::min);
    let max_lat = nodes
        .iter()
        .map(|n| n.lat)
        .fold(f32::NEG_INFINITY, f32::max);
    let min_lon = nodes.iter().map(|n| n.lon).fold(f32::INFINITY, f32::min);
    let max_lon = nodes
        .iter()
        .map(|n| n.lon)
        .fold(f32::NEG_INFINITY, f32::max);

    let mut rng = Lcg::new(seed);
    let mut seen: HashSet<u32> = exclude.iter().copied().collect();
    let mut out = Vec::with_capacity(count);
    let mut attempts = 0u64;
    while out.len() < count {
        attempts += 1;
        if attempts > 50_000_000 {
            panic!("failed to find {count} distinct snapped nodes after {attempts} attempts");
        }
        let lat = min_lat + rng.next_f64() as f32 * (max_lat - min_lat);
        let lon = min_lon + rng.next_f64() as f32 * (max_lon - min_lon);
        if let Some(id) = pack.snap(lat, lon) {
            if seen.insert(id) {
                out.push(id);
            }
        }
    }
    out
}

fn costs_close(a: f64, b: f64) -> bool {
    let scale = a.abs().max(b.abs()).max(1.0);
    (a - b).abs() / scale <= RELATIVE_TOLERANCE
}

fn main() {
    let args = parse_args();

    let pack = Rpack::open(&args.pack).expect("failed to open pack");
    println!(
        "opened pack '{}' ({} nodes, {} edges)",
        pack.region_name(),
        pack.node_count(),
        pack.edges().len()
    );
    let router = Router::new(&pack);

    let origin_node = snap_or_brute_force(&pack, ORIGIN_LAT, ORIGIN_LON, "origin");
    let dest_node = snap_or_brute_force(&pack, args.dest.0, args.dest.1, "destination");
    println!("origin ({ORIGIN_LAT}, {ORIGIN_LON}) -> node {origin_node}");
    println!(
        "destination ({}, {}) -> node {dest_node}",
        args.dest.0, args.dest.1
    );

    // --- p2p: 1 warmup + 10 timed runs. ---
    let _ = router.p2p(origin_node, dest_node);
    let mut times_ms = Vec::with_capacity(10);
    let mut route = None;
    for _ in 0..10 {
        let t0 = Instant::now();
        route = router.p2p(origin_node, dest_node);
        times_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
    }
    let min_ms = times_ms.iter().cloned().fold(f64::INFINITY, f64::min);
    let mean_ms = times_ms.iter().sum::<f64>() / times_ms.len() as f64;
    match &route {
        Some(r) => println!(
            "p2p: min={min_ms:.3}ms mean={mean_ms:.3}ms cost_seconds={:.2} length_m={:.2} edges={}",
            r.cost_seconds,
            r.length_m,
            r.edges.len()
        ),
        None => println!("p2p: min={min_ms:.3}ms mean={mean_ms:.3}ms -> no route found"),
    }

    // --- many-to-many: origin + 200 random nodes vs. 200 random nodes + dest. ---
    let random_nodes = distinct_snapped_nodes(&pack, 0x5115_c0de, 200, &[origin_node, dest_node]);
    let mut sources = vec![origin_node];
    sources.extend_from_slice(&random_nodes);
    let mut targets = random_nodes.clone();
    targets.push(dest_node);

    let t0 = Instant::now();
    let table = router.many_to_many(&sources, &targets);
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let n_pairs = sources.len() * targets.len();
    assert_eq!(table.len(), sources.len());
    println!(
        "many_to_many: {} sources x {} targets = {n_pairs} pairs, total={elapsed_ms:.3}ms, ms/pair={:.6}",
        sources.len(),
        targets.len(),
        elapsed_ms / n_pairs as f64
    );

    // --- optional Dijkstra-vs-CH cross-check against the base graph. ---
    if let Some(dir) = &args.base_slice_dir {
        let (base, _stats) =
            build_base_model(dir).expect("failed to rebuild base model from --base-slice-dir");

        let mut pairs = vec![(origin_node, dest_node)];
        let extra = distinct_snapped_nodes(&pack, 0xBEEF_5EED, 10, &[origin_node, dest_node]);
        for chunk in extra.as_chunks::<2>().0 {
            pairs.push((chunk[0], chunk[1]));
        }

        for (s, t) in pairs {
            let expected = reference::dijkstra_cost(&base, s, t);
            let got = router.p2p(s, t).map(|r| r.cost_seconds);
            let pass = match (expected, got) {
                (None, None) => true,
                (Some(e), Some(g)) => costs_close(e, g),
                _ => false,
            };
            println!(
                "dijkstra-vs-ch [{s} -> {t}]: reference={expected:?} ch={got:?} -> {}",
                if pass { "PASS" } else { "FAIL" }
            );
        }
    }
}
