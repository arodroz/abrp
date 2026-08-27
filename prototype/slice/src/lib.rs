// Throwaway performance prototype. See README.md.
//
// Single-file lib on purpose: this crate exists to measure, not to be maintained.

uniffi::setup_scaffolding!();

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use fast_paths::{FastGraph, PathCalculator};
use memmap2::Mmap;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// ---------------------------------------------------------------------
// Pack file formats (shared with bin/build_pack.rs)
// ---------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
pub struct NodesFile {
    pub lat: Vec<f32>,
    pub lon: Vec<f32>,
}

#[derive(Serialize, Deserialize, Clone, Copy)]
pub struct EdgeMeta {
    pub from: u32,
    pub to: u32,
    pub length_m: f32,
    pub speed_kmh: f32,
    /// Offset/len into the *point* array of geometry.bin (multiply by 8 for byte offset:
    /// each point is two little-endian f32s, lat then lon).
    pub geom_offset: u32,
    pub geom_len: u32,
}

/// `edges` is sorted by (from, to); `from_start[n]..from_start[n+1]` is the (already
/// sorted-by-`to`) slice of outgoing edges for node `n` -- a CSR row index, used instead
/// of a `(from,to) -> edge` HashMap to keep the corridor-scale pack's resident memory
/// down. Geometry itself lives in a separate raw file (geometry.bin) so it can be
/// mmapped instead of fully deserialized into RAM.
#[derive(Serialize, Deserialize)]
pub struct EdgesMetaFile {
    pub edges: Vec<EdgeMeta>,
    pub from_start: Vec<u32>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Charger {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub power_kw: f32,
    pub country: String,
}

// ---------------------------------------------------------------------
// Energy model (ADR 0003 simplified per prototype spec, flat grade)
// ---------------------------------------------------------------------

const USABLE_KWH: f64 = 70.0;
const BATTERY_WH: f64 = USABLE_KWH * 1000.0;
const MASS_KG: f64 = 2050.0;
const CDA: f64 = 0.72;
const CRR: f64 = 0.009;
const ETA_DRIVE: f64 = 0.85;
const AUX_W: f64 = 1000.0;
const RHO: f64 = 1.225;
const G: f64 = 9.81;
const URBAN_FACTOR: f64 = 1.15;
const URBAN_SPEED_THRESHOLD_KMH: f64 = 50.0;
/// Rough average consumption used only for the straight-line energy-bound prune.
const AVG_WH_PER_M: f64 = 0.18;

/// Energy in Wh to cross one edge at its (assumed constant) speed.
fn edge_energy_wh(length_m: f64, speed_kmh: f64) -> f64 {
    let v_ms = speed_kmh / 3.6;
    let p_watt = 0.5 * RHO * CDA * v_ms.powi(3) + MASS_KG * G * CRR * v_ms + AUX_W;
    let t_s = length_m / v_ms;
    let mut e_wh = p_watt * t_s / ETA_DRIVE / 3600.0;
    if speed_kmh <= URBAN_SPEED_THRESHOLD_KMH {
        e_wh *= URBAN_FACTOR;
    }
    e_wh
}

/// Warm 800V charging curve, digitised from docs/research/ioniq5-energy-model.md
/// section 4.1 (peak ~220-225kW low-mid SoC, thermal dip ~52-54%, hard taper above 80%).
const CURVE: &[(f64, f64)] = &[
    (0.00, 50.0),
    (0.10, 115.0),
    (0.15, 187.0),
    (0.30, 220.0),
    (0.50, 225.0),
    (0.55, 120.0),
    (0.80, 130.0),
    (0.90, 70.0),
    (1.00, 10.0),
];

fn curve_power_kw(soc: f64) -> f64 {
    let soc = soc.clamp(0.0, 1.0);
    if soc <= CURVE[0].0 {
        return CURVE[0].1;
    }
    for w in CURVE.windows(2) {
        let (s0, p0) = w[0];
        let (s1, p1) = w[1];
        if soc <= s1 {
            let f = (soc - s0) / (s1 - s0);
            return p0 + f * (p1 - p0);
        }
    }
    CURVE.last().unwrap().1
}

/// Numerically integrates the charging curve, capped by the charger's own power.
fn charge_time_s(from_soc: f64, to_soc: f64, charger_kw: f64) -> f64 {
    if to_soc <= from_soc {
        return 0.0;
    }
    const STEPS: usize = 50;
    let dsoc = (to_soc - from_soc) / STEPS as f64;
    let mut t = 0.0;
    let mut soc = from_soc;
    for _ in 0..STEPS {
        let mid = soc + dsoc / 2.0;
        let p_kw = curve_power_kw(mid).min(charger_kw).max(1.0);
        let energy_kwh = USABLE_KWH * dsoc;
        t += energy_kwh / p_kw * 3600.0;
        soc += dsoc;
    }
    t
}

// ---------------------------------------------------------------------
// Geometry helpers
// ---------------------------------------------------------------------

const EARTH_R_M: f64 = 6_371_000.0;

pub fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let p1 = lat1.to_radians();
    let p2 = lat2.to_radians();
    let dphi = (lat2 - lat1).to_radians();
    let dlambda = (lon2 - lon1).to_radians();
    let a = (dphi / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlambda / 2.0).sin().powi(2);
    2.0 * EARTH_R_M * a.sqrt().asin()
}

// ---------------------------------------------------------------------
// Planner
// ---------------------------------------------------------------------

#[derive(uniffi::Object)]
pub struct Planner {
    graph: FastGraph,
    calculator: Mutex<PathCalculator>,
    node_lat: Vec<f32>,
    node_lon: Vec<f32>,
    /// Sorted by (from, to); looked up via `from_start` (a CSR row index) + binary
    /// search instead of a `(from,to) -> edge` HashMap, to keep resident memory down
    /// at corridor scale (millions of edges).
    edges: Vec<EdgeMeta>,
    from_start: Vec<u32>,
    /// mmap of geometry.bin (raw little-endian f32 pairs); only the handful of edges
    /// actually touched by route reconstruction / corridor projection / soc_curve are
    /// ever paged in, unlike a fully-deserialized Vec.
    geometry_mmap: Mmap,
    chargers: Vec<Charger>,
    /// Nodes with at least one outgoing / incoming edge (a handful of nodes are
    /// directed dead-ends -- oneway-only cul-de-sacs -- and must not be snap targets
    /// for the wrong direction of travel).
    has_out: Vec<bool>,
    has_in: Vec<bool>,
    /// Uniform lat/lon grid (~0.1 degree cells) over node indices, so nearest-node
    /// snapping doesn't have to linearly scan all nodes -- at corridor scale
    /// (1.5M+ nodes) that made every candidate-charger snap cost a full pass over the
    /// whole graph, which was the actual optimiser_ms bottleneck (not CH queries).
    grid: HashMap<(i32, i32), Vec<u32>>,
}

const GRID_CELL_DEG: f64 = 0.1;

fn grid_cell(lat: f64, lon: f64) -> (i32, i32) {
    ((lat / GRID_CELL_DEG).floor() as i32, (lon / GRID_CELL_DEG).floor() as i32)
}

#[uniffi::export]
impl Planner {
    #[uniffi::constructor]
    pub fn new(pack_dir: String) -> Arc<Self> {
        let dir = Path::new(&pack_dir);
        let graph: FastGraph = bincode::deserialize_from(BufReader::new(
            File::open(dir.join("graph.bin")).expect("open graph.bin"),
        ))
        .expect("decode graph.bin");
        let nodes: NodesFile = bincode::deserialize_from(BufReader::new(
            File::open(dir.join("nodes.bin")).expect("open nodes.bin"),
        ))
        .expect("decode nodes.bin");
        let edges_meta: EdgesMetaFile = bincode::deserialize_from(BufReader::new(
            File::open(dir.join("edges_meta.bin")).expect("open edges_meta.bin"),
        ))
        .expect("decode edges_meta.bin");
        let chargers: Vec<Charger> = bincode::deserialize_from(BufReader::new(
            File::open(dir.join("chargers.bin")).expect("open chargers.bin"),
        ))
        .expect("decode chargers.bin");
        let geometry_file = File::open(dir.join("geometry.bin")).expect("open geometry.bin");
        let geometry_mmap = unsafe { Mmap::map(&geometry_file).expect("mmap geometry.bin") };

        let mut has_out = vec![false; nodes.lat.len()];
        let mut has_in = vec![false; nodes.lat.len()];
        for e in &edges_meta.edges {
            has_out[e.from as usize] = true;
            has_in[e.to as usize] = true;
        }
        let calculator = fast_paths::create_calculator(&graph);

        let mut grid: HashMap<(i32, i32), Vec<u32>> = HashMap::new();
        for i in 0..nodes.lat.len() {
            grid.entry(grid_cell(nodes.lat[i] as f64, nodes.lon[i] as f64)).or_default().push(i as u32);
        }

        Arc::new(Planner {
            calculator: Mutex::new(calculator),
            node_lat: nodes.lat,
            node_lon: nodes.lon,
            edges: edges_meta.edges,
            from_start: edges_meta.from_start,
            geometry_mmap,
            chargers,
            has_out,
            has_in,
            grid,
            graph,
        })
    }

    pub fn plan_json(&self, request_json: String) -> String {
        let total_start = Instant::now();
        let req: Value = match serde_json::from_str(&request_json) {
            Ok(v) => v,
            Err(e) => return error_response(&format!("bad request json: {e}")),
        };
        plan_impl(self, &req, total_start)
    }
}

impl Planner {
    /// Nearest-node search via the grid index, restricted to nodes usable in `filter`
    /// (a handful of nodes are directed dead-ends and must not be snap targets for the
    /// wrong direction of travel -- see `has_out`/`has_in`). Expands the search ring
    /// one grid cell at a time (almost always resolves at radius 1, i.e. a 3x3-cell
    /// ~30km window) and only falls back to a full linear scan if that fails
    /// completely, which should not happen for coordinates inside the pack's extent.
    fn nearest_node(&self, lat: f64, lon: f64, filter: &[bool]) -> u32 {
        let (clat, clon) = grid_cell(lat, lon);
        let mut radius = 1i32;
        while radius <= 50 {
            let mut best: Option<u32> = None;
            let mut best_d = f64::MAX;
            for dy in -radius..=radius {
                for dx in -radius..=radius {
                    let Some(idxs) = self.grid.get(&(clat + dy, clon + dx)) else { continue };
                    for &i in idxs {
                        if !filter[i as usize] {
                            continue;
                        }
                        let d = haversine_m(lat, lon, self.node_lat[i as usize] as f64, self.node_lon[i as usize] as f64);
                        if d < best_d {
                            best_d = d;
                            best = Some(i);
                        }
                    }
                }
            }
            if let Some(b) = best {
                return b;
            }
            radius += 1;
        }
        // Fallback: full linear scan (only reached for coordinates far outside the pack).
        let mut best = 0usize;
        let mut best_d = f64::MAX;
        for i in 0..self.node_lat.len() {
            if !filter[i] {
                continue;
            }
            let d = haversine_m(lat, lon, self.node_lat[i] as f64, self.node_lon[i] as f64);
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        best as u32
    }

    /// CSR lookup: binary search the (from,to)-sorted `edges` slice for node `from`.
    fn find_edge(&self, from: u32, to: u32) -> Option<usize> {
        let start = *self.from_start.get(from as usize)? as usize;
        let end = *self.from_start.get(from as usize + 1)? as usize;
        let slice = &self.edges[start..end];
        slice.binary_search_by_key(&to, |e| e.to).ok().map(|i| start + i)
    }

    /// Reads one edge's geometry out of the mmapped geometry.bin on demand -- only
    /// touched for route reconstruction, corridor projection, and soc_curve sampling,
    /// so this never materializes the whole graph's geometry in resident memory.
    fn edge_geometry(&self, e: &EdgeMeta) -> Vec<(f64, f64)> {
        let byte_start = e.geom_offset as usize * 8;
        let byte_len = e.geom_len as usize * 8;
        let bytes = &self.geometry_mmap[byte_start..byte_start + byte_len];
        bytes
            .chunks_exact(8)
            .map(|c| {
                let lat = f32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f64;
                let lon = f32::from_le_bytes([c[4], c[5], c[6], c[7]]) as f64;
                (lat, lon)
            })
            .collect()
    }
}

fn error_response(msg: &str) -> String {
    json!({
        "total_time_s": 0.0, "drive_time_s": 0.0, "charge_time_s": 0.0, "total_dist_m": 0.0,
        "route_geojson": {"type": "LineString", "coordinates": []},
        "stops": [], "soc_curve": [],
        "timings": {"route_ms": 0.0, "optimiser_ms": 0.0, "total_ms": 0.0},
        "error": msg,
    })
    .to_string()
}

// ---------------------------------------------------------------------
// Leg queries (CH query + edge lookup -> time/energy/geometry)
// ---------------------------------------------------------------------

#[derive(Clone)]
struct Leg {
    time_s: f64,
    energy_wh: f64,
    dist_m: f64,
    /// (lat, lon) in travel order, junction points deduplicated across edges.
    geometry: Vec<(f64, f64)>,
}

fn query_leg(p: &Planner, calc: &mut PathCalculator, from: u32, to: u32) -> Option<Leg> {
    if from == to {
        return Some(Leg { time_s: 0.0, energy_wh: 0.0, dist_m: 0.0, geometry: vec![] });
    }
    let path = calc.calc_path(&p.graph, from as usize, to as usize)?;
    let nodes = path.get_nodes();
    let mut dist_m = 0.0;
    let mut energy_wh = 0.0;
    let mut time_s = 0.0;
    let mut geometry: Vec<(f64, f64)> = Vec::new();
    for w in nodes.windows(2) {
        let a = w[0] as u32;
        let b = w[1] as u32;
        let idx = p.find_edge(a, b)?;
        let e = &p.edges[idx];
        let g = p.edge_geometry(e);
        if geometry.is_empty() {
            geometry.extend(g);
        } else {
            geometry.extend(g.into_iter().skip(1));
        }
        let v_ms = e.speed_kmh as f64 / 3.6;
        time_s += e.length_m as f64 / v_ms;
        energy_wh += edge_energy_wh(e.length_m as f64, e.speed_kmh as f64);
        dist_m += e.length_m as f64;
    }
    Some(Leg { time_s, energy_wh, dist_m, geometry })
}

/// `query_leg` memoized per (from,to) node pair -- candidate dedup already removes
/// most repeats, but this also protects against duplicate work if two distinct
/// candidates ever snap to the same graph node. `ch_queries` is only incremented on
/// an actual cache miss, so it reflects real CH work done.
fn cached_leg(
    p: &Planner,
    calc: &mut PathCalculator,
    cache: &mut HashMap<(u32, u32), Option<Leg>>,
    ch_queries: &mut u32,
    from: u32,
    to: u32,
) -> Option<Leg> {
    if let Some(hit) = cache.get(&(from, to)) {
        return hit.clone();
    }
    *ch_queries += 1;
    let leg = query_leg(p, calc, from, to);
    cache.insert((from, to), leg.clone());
    leg
}

// ---------------------------------------------------------------------
// Optimiser (ADR 0006, simplified)
// ---------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
struct HeapItem {
    time_ms: i64,
    node: usize,
    bucket: i32,
}
impl Ord for HeapItem {
    fn cmp(&self, other: &Self) -> Ordering {
        other.time_ms.cmp(&self.time_ms) // min-heap
    }
}
impl PartialOrd for HeapItem {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// One precomputed edge in the small candidate graph (origin, chargers, dest).
struct SearchEdge {
    to: usize,
    leg: Leg,
}

struct CameFrom {
    prev_node: usize,
    prev_bucket: i32,
    edge_idx: usize,
    depart_soc_at_prev: f64, // SoC leaving prev_node (after any charging there)
    arrival_soc_at_prev: f64, // SoC when we arrived at prev_node (before charging)
    charge_s_at_prev: f64,
}

fn soc_bucket(soc: f64) -> i32 {
    (soc * 50.0).floor() as i32 // 2% buckets
}

fn plan_impl(p: &Planner, req: &Value, total_start: Instant) -> String {
    let origin = req["origin"].as_array();
    let dest = req["dest"].as_array();
    let (Some(o), Some(d)) = (origin, dest) else {
        return error_response("origin/dest missing");
    };
    let o_lat = o[0].as_f64().unwrap_or(0.0);
    let o_lon = o[1].as_f64().unwrap_or(0.0);
    let d_lat = d[0].as_f64().unwrap_or(0.0);
    let d_lon = d[1].as_f64().unwrap_or(0.0);

    let depart_soc = req["depart_soc"].as_f64().unwrap_or(0.9);
    let arrival_min_soc = req["arrival_min_soc"].as_f64().unwrap_or(0.1);
    let charger_arrival_min_soc = req["charger_arrival_min_soc"].as_f64().unwrap_or(0.1);
    let charger_max_soc = req["charger_max_soc"].as_f64().unwrap_or(0.8);
    let stops_bias = req["stops_bias"].as_f64().unwrap_or(1.0);

    let origin_node = p.nearest_node(o_lat, o_lon, &p.has_out);
    let dest_node = p.nearest_node(d_lat, d_lon, &p.has_in);
    let both: Vec<bool> = (0..p.has_out.len()).map(|i| p.has_out[i] && p.has_in[i]).collect();

    let route_start = Instant::now();
    let mut calc = p.calculator.lock().unwrap();
    let direct = query_leg(p, &mut calc, origin_node, dest_node);
    let route_ms = route_start.elapsed().as_secs_f64() * 1000.0;

    let Some(direct_leg) = direct else {
        return error_response("no route found");
    };
    let direct_arrival_soc = depart_soc - direct_leg.energy_wh / BATTERY_WH;

    if direct_arrival_soc >= arrival_min_soc {
        // 0 stops.
        let route_geojson = linestring_json(&direct_leg.geometry);
        let soc_curve = sample_soc_curve(&direct_leg.geometry, depart_soc, direct_leg.energy_wh);
        let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;
        return json!({
            "total_time_s": direct_leg.time_s,
            "drive_time_s": direct_leg.time_s,
            "charge_time_s": 0.0,
            "total_dist_m": direct_leg.dist_m,
            "route_geojson": route_geojson,
            "stops": [],
            "soc_curve": soc_curve,
            "timings": {"route_ms": route_ms, "optimiser_ms": 0.0, "total_ms": total_ms},
        })
        .to_string();
    }

    // --- Need stops: build candidate set along the direct-route polyline. ---
    let opt_start = Instant::now();
    let ref_geom = &direct_leg.geometry;
    let mut ref_cum: Vec<f64> = Vec::with_capacity(ref_geom.len());
    let mut acc = 0.0;
    for i in 0..ref_geom.len() {
        if i > 0 {
            acc += haversine_m(ref_geom[i - 1].0, ref_geom[i - 1].1, ref_geom[i].0, ref_geom[i].1);
        }
        ref_cum.push(acc);
    }

    const CORRIDOR_M: f64 = 10_000.0;

    // The corridor-projection loop is O(chargers * route points); at corridor scale
    // (1,500+ chargers, thousands of raw OSM shape points) that dominated optimiser_ms
    // even after the CH-query fan-out was fixed. Two cheap pre-filters, both safe to
    // over-include (the exact CORRIDOR_M check below still applies):
    //  1. a coarse lat/lon bounding-box check with no trig, and
    //  2. matching against a route polyline downsampled to ~2km spacing -- ample
    //     resolution for a 10km-wide corridor and 20km candidate spacing.
    let mut min_lat = f64::MAX;
    let mut max_lat = f64::MIN;
    let mut min_lon = f64::MAX;
    let mut max_lon = f64::MIN;
    for (lat, lon) in ref_geom.iter() {
        min_lat = min_lat.min(*lat);
        max_lat = max_lat.max(*lat);
        min_lon = min_lon.min(*lon);
        max_lon = max_lon.max(*lon);
    }
    // Conservative (i.e. generous) degrees-per-meter, valid down to ~55 degrees latitude.
    let margin_deg = CORRIDOR_M / 70_000.0;
    min_lat -= margin_deg;
    max_lat += margin_deg;
    min_lon -= margin_deg;
    max_lon += margin_deg;

    const CORRIDOR_SAMPLE_M: f64 = 2_000.0;
    let mut coarse: Vec<(f64, f64, f64)> = Vec::new(); // (lat, lon, cum_dist)
    let mut next_sample = 0.0;
    for i in 0..ref_geom.len() {
        if ref_cum[i] + 1e-9 >= next_sample || i == ref_geom.len() - 1 {
            coarse.push((ref_geom[i].0, ref_geom[i].1, ref_cum[i]));
            next_sample += CORRIDOR_SAMPLE_M;
        }
    }

    let mut candidates: Vec<(&Charger, f64 /* s */, u32 /* node */)> = Vec::new();
    for c in &p.chargers {
        if c.lat < min_lat || c.lat > max_lat || c.lon < min_lon || c.lon > max_lon {
            continue;
        }
        let mut best_d = f64::MAX;
        let mut best_s = 0.0;
        for (lat, lon, cum) in &coarse {
            let dd = haversine_m(c.lat, c.lon, *lat, *lon);
            if dd < best_d {
                best_d = dd;
                best_s = *cum;
            }
        }
        if best_d <= CORRIDOR_M {
            let node = p.nearest_node(c.lat, c.lon, &both);
            candidates.push((c, best_s, node));
        }
    }
    candidates.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());

    // Dedup candidates within ~300m of each other, keeping the highest power_kw. The
    // charger feeds carry direction-pair duplicates (opposite carriageways of the same
    // physical site); each survivor is a distinct fan-out node in the search graph, so
    // duplicates alone were driving a lot of the O(n^2) CH-query blowup.
    const DEDUP_M: f64 = 300.0;
    let mut keep_candidate = vec![true; candidates.len()];
    for i in 0..candidates.len() {
        if !keep_candidate[i] {
            continue;
        }
        for j in (i + 1)..candidates.len() {
            if !keep_candidate[j] {
                continue;
            }
            if haversine_m(candidates[i].0.lat, candidates[i].0.lon, candidates[j].0.lat, candidates[j].0.lon)
                <= DEDUP_M
            {
                if candidates[j].0.power_kw > candidates[i].0.power_kw {
                    keep_candidate[i] = false;
                    break;
                } else {
                    keep_candidate[j] = false;
                }
            }
        }
    }
    let mut di = 0;
    candidates.retain(|_| {
        let k = keep_candidate[di];
        di += 1;
        k
    });
    candidates.truncate(300); // ADR 0006 cap
    let n_candidates = candidates.len();
    const FORWARD_GAP_M: f64 = 20_000.0;
    const MAX_FANOUT: usize = 30;
    let mut leg_cache: HashMap<(u32, u32), Option<Leg>> = HashMap::new();

    // Search-graph node indices: 0 = origin, 1..=n = candidates, n+1 = dest.
    const ORIGIN_IDX: usize = 0;
    let dest_idx = n_candidates + 1;

    let mut ch_queries = 0u32;
    let mut adj: HashMap<usize, Vec<SearchEdge>> = HashMap::new();

    let straight_line_ok = |a_lat: f64, a_lon: f64, b_lat: f64, b_lon: f64, avail_wh: f64| -> bool {
        let d = haversine_m(a_lat, a_lon, b_lat, b_lon) * 1.3;
        d * AVG_WH_PER_M <= avail_wh
    };

    // origin -> candidates
    for (i, (c, _s, node)) in candidates.iter().enumerate() {
        let avail_wh = depart_soc * BATTERY_WH;
        if !straight_line_ok(o_lat, o_lon, c.lat, c.lon, avail_wh) {
            continue;
        }
        if let Some(leg) = cached_leg(p, &mut calc, &mut leg_cache, &mut ch_queries, origin_node, *node) {
            adj.entry(ORIGIN_IDX).or_default().push(SearchEdge { to: i + 1, leg });
        }
    }
    // origin -> dest (always allowed as a graph edge; reuse the already-computed leg)
    adj.entry(ORIGIN_IDX).or_default().push(SearchEdge {
        to: dest_idx,
        leg: Leg {
            time_s: direct_leg.time_s,
            energy_wh: direct_leg.energy_wh,
            dist_m: direct_leg.dist_m,
            geometry: direct_leg.geometry.clone(),
        },
    });

    // candidate_i -> candidate_j: forward-pruned (skip anything closer than
    // FORWARD_GAP_M along the route -- not worth a separate stop) and capped at the
    // nearest MAX_FANOUT reachable candidates by route position, so the fan-out stays
    // roughly O(n * MAX_FANOUT) instead of O(n^2) at corridor scale.
    for (i, (ci, si, ni)) in candidates.iter().enumerate() {
        let avail_wh = charger_max_soc * BATTERY_WH;
        let mut taken = 0usize;
        for (j, (cj, sj, nj)) in candidates.iter().enumerate() {
            if taken >= MAX_FANOUT {
                break;
            }
            if *sj <= *si + FORWARD_GAP_M {
                continue;
            }
            if !straight_line_ok(ci.lat, ci.lon, cj.lat, cj.lon, avail_wh) {
                continue;
            }
            taken += 1;
            if let Some(leg) = cached_leg(p, &mut calc, &mut leg_cache, &mut ch_queries, *ni, *nj) {
                adj.entry(i + 1).or_default().push(SearchEdge { to: j + 1, leg });
            }
        }
        // candidate -> dest
        if straight_line_ok(ci.lat, ci.lon, d_lat, d_lon, avail_wh) {
            if let Some(leg) = cached_leg(p, &mut calc, &mut leg_cache, &mut ch_queries, *ni, dest_node) {
                adj.entry(i + 1).or_default().push(SearchEdge { to: dest_idx, leg });
            }
        }
    }
    drop(calc);

    eprintln!(
        "[planner] candidates={} ch_queries={}",
        n_candidates, ch_queries
    );

    let charger_of = |idx: usize| -> Option<&Charger> {
        if idx == ORIGIN_IDX || idx == dest_idx {
            None
        } else {
            Some(candidates[idx - 1].0)
        }
    };

    // Try progressively relaxed constraints, per ADR 0006 section 4 (simplified: two relaxations).
    let attempts: [(f64, f64, &str); 3] = [
        (charger_arrival_min_soc, arrival_min_soc, ""),
        (0.0, arrival_min_soc, "ARRIVAL_SOC_BELOW_WANTED"),
        (0.0, 0.0, "ARRIVAL_SOC_BELOW_WANTED"),
    ];

    let mut result: Option<(HashMap<(usize, i32), CameFrom>, i32, String)> = None;
    for (charger_min, dest_min, flag) in attempts {
        if let Some((came_from, winning_bucket)) = run_label_search(
            &adj, ORIGIN_IDX, dest_idx, depart_soc, charger_min, dest_min,
            charger_max_soc, stops_bias, &charger_of,
        ) {
            result = Some((came_from, winning_bucket, flag.to_string()));
            break;
        }
    }

    let optimiser_ms = opt_start.elapsed().as_secs_f64() * 1000.0;

    let Some((came_from, winning_bucket, error_flag)) = result else {
        return error_response("NO_ROUTE: destination unreachable even after relaxing SoC constraints");
    };

    // Reconstruct path: walk back from (dest_idx, winning_bucket) to origin.
    let mut chain: Vec<(usize, i32)> = vec![(dest_idx, winning_bucket)];
    let mut cur = (dest_idx, winning_bucket);
    while cur.0 != ORIGIN_IDX {
        let cf = came_from.get(&cur).expect("came_from chain broken");
        cur = (cf.prev_node, cf.prev_bucket);
        chain.push(cur);
    }
    chain.reverse(); // origin ... dest

    let mut route_geometry: Vec<(f64, f64)> = Vec::new();
    let mut stops_out: Vec<Value> = Vec::new();
    let mut drive_time_s = 0.0;
    let mut charge_time_total_s = 0.0;
    let mut total_dist_m = 0.0;
    let mut soc_segments: Vec<(f64, f64, Vec<(f64, f64)>)> = Vec::new(); // (soc_start, energy_wh, geometry)

    for w in chain.windows(2) {
        let (from_key, to_key) = (w[0], w[1]);
        let cf = came_from.get(&to_key).unwrap();
        let edge = &adj[&from_key.0][cf.edge_idx];
        drive_time_s += edge.leg.time_s;
        total_dist_m += edge.leg.dist_m;

        if !route_geometry.is_empty() && !edge.leg.geometry.is_empty() {
            route_geometry.pop(); // avoid duplicate junction point between legs
        }
        route_geometry.extend(edge.leg.geometry.iter().cloned());
        soc_segments.push((cf.depart_soc_at_prev, edge.leg.energy_wh, edge.leg.geometry.clone()));

        if let Some(charger) = charger_of(from_key.0) {
            charge_time_total_s += cf.charge_s_at_prev;
            stops_out.push(json!({
                "name": charger.name,
                "lat": charger.lat,
                "lon": charger.lon,
                "power_kw": charger.power_kw,
                "arrival_soc": cf.arrival_soc_at_prev,
                "depart_soc": cf.depart_soc_at_prev,
                "charge_s": cf.charge_s_at_prev,
                "dist_from_start_m": total_dist_m - edge.leg.dist_m,
            }));
        }
    }

    let route_geojson = linestring_json(&route_geometry);
    let soc_curve = sample_soc_curve_multi(&soc_segments);
    let total_time_s = drive_time_s + charge_time_total_s;
    let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;

    let mut resp = json!({
        "total_time_s": total_time_s,
        "drive_time_s": drive_time_s,
        "charge_time_s": charge_time_total_s,
        "total_dist_m": total_dist_m,
        "route_geojson": route_geojson,
        "stops": stops_out,
        "soc_curve": soc_curve,
        "timings": {"route_ms": route_ms, "optimiser_ms": optimiser_ms, "total_ms": total_ms},
    });
    if !error_flag.is_empty() {
        resp["error"] = json!(error_flag);
    }
    resp.to_string()
}

#[allow(clippy::too_many_arguments)]
fn run_label_search<'a>(
    adj: &HashMap<usize, Vec<SearchEdge>>,
    origin_idx: usize,
    dest_idx: usize,
    depart_soc: f64,
    charger_arrival_min_soc: f64,
    arrival_min_soc: f64,
    charger_max_soc: f64,
    stops_bias: f64,
    charger_of: &dyn Fn(usize) -> Option<&'a Charger>,
) -> Option<(HashMap<(usize, i32), CameFrom>, i32)> {
    let stop_penalty_s = 300.0 * stops_bias;

    let mut best_time: HashMap<(usize, i32), f64> = HashMap::new();
    let mut came_from: HashMap<(usize, i32), CameFrom> = HashMap::new();
    let mut heap: BinaryHeap<HeapItem> = BinaryHeap::new();

    let start_bucket = soc_bucket(depart_soc);
    best_time.insert((origin_idx, start_bucket), 0.0);
    heap.push(HeapItem { time_ms: 0, node: origin_idx, bucket: start_bucket });
    // Track exact soc per (node,bucket) winner separately from the bucket key.
    let mut exact_soc: HashMap<(usize, i32), f64> = HashMap::new();
    exact_soc.insert((origin_idx, start_bucket), depart_soc);

    while let Some(item) = heap.pop() {
        let key = (item.node, item.bucket);
        let cur_time = item.time_ms as f64 / 1000.0;
        if let Some(&bt) = best_time.get(&key) {
            if cur_time > bt + 1e-6 {
                continue; // stale
            }
        }
        if item.node == dest_idx {
            return Some((came_from, item.bucket));
        }
        let cur_soc = *exact_soc.get(&key).unwrap();

        let Some(edges) = adj.get(&item.node) else { continue };
        let is_charger_node = charger_of(item.node).is_some();
        let charger_kw = charger_of(item.node).map(|c| c.power_kw as f64);

        for (edge_idx, edge) in edges.iter().enumerate() {
            let required_min = if edge.to == dest_idx { arrival_min_soc } else { charger_arrival_min_soc };

            if item.node == origin_idx {
                // No charging at origin: just travel.
                let soc_after = cur_soc - edge.leg.energy_wh / BATTERY_WH;
                if soc_after + 1e-9 < required_min || soc_after < 0.0 {
                    continue;
                }
                let new_time = cur_time + edge.leg.time_s;
                let nb = soc_bucket(soc_after);
                let nk = (edge.to, nb);
                if new_time < *best_time.get(&nk).unwrap_or(&f64::MAX) - 1e-9 {
                    best_time.insert(nk, new_time);
                    exact_soc.insert(nk, soc_after);
                    came_from.insert(nk, CameFrom {
                        prev_node: item.node,
                        prev_bucket: item.bucket,
                        edge_idx,
                        depart_soc_at_prev: cur_soc,
                        arrival_soc_at_prev: cur_soc,
                        charge_s_at_prev: 0.0,
                    });
                    heap.push(HeapItem { time_ms: (new_time * 1000.0) as i64, node: edge.to, bucket: nb });
                }
            } else {
                // Charger node: branch over discrete depart targets.
                debug_assert!(is_charger_node);
                let kw = charger_kw.unwrap();
                let need_for_edge = (edge.leg.energy_wh / BATTERY_WH) + required_min + 0.05;
                // MIN_CHARGE_SOC: a depart target this close to arrival SoC is not a real
                // charging decision (defect: it used to create free, zero-penalty "stops"
                // that just happened to sit on the winning path). Anything below this bar
                // is dropped rather than clamped, so no label -- and therefore no stop --
                // is ever created for it; a genuine bypass of this candidate is already
                // covered by the direct edges between its neighbours in `adj`.
                const MIN_CHARGE_SOC: f64 = 0.02;
                let mut targets: Vec<f64> = vec![0.6, 0.7, 0.8, charger_max_soc, need_for_edge.min(charger_max_soc)];
                targets.retain(|t| *t > cur_soc + MIN_CHARGE_SOC - 1e-9 && *t <= charger_max_soc + 1e-9);
                targets.sort_by(|a, b| a.partial_cmp(b).unwrap());
                targets.dedup_by(|a, b| (*a - *b).abs() < 1e-6);

                for target_soc in targets {
                    let charge_s = charge_time_s(cur_soc, target_soc, kw);
                    let soc_after = target_soc - edge.leg.energy_wh / BATTERY_WH;
                    if soc_after + 1e-9 < required_min || soc_after < 0.0 {
                        continue;
                    }
                    let new_time = cur_time + charge_s + stop_penalty_s + edge.leg.time_s;
                    let nb = soc_bucket(soc_after);
                    let nk = (edge.to, nb);
                    if new_time < *best_time.get(&nk).unwrap_or(&f64::MAX) - 1e-9 {
                        best_time.insert(nk, new_time);
                        exact_soc.insert(nk, soc_after);
                        came_from.insert(nk, CameFrom {
                            prev_node: item.node,
                            prev_bucket: item.bucket,
                            edge_idx,
                            depart_soc_at_prev: target_soc,
                            arrival_soc_at_prev: cur_soc,
                            charge_s_at_prev: charge_s,
                        });
                        heap.push(HeapItem { time_ms: (new_time * 1000.0) as i64, node: edge.to, bucket: nb });
                    }
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------------
// GeoJSON / soc-curve output helpers
// ---------------------------------------------------------------------

fn linestring_json(geometry: &[(f64, f64)]) -> Value {
    json!({
        "type": "LineString",
        "coordinates": geometry.iter().map(|(lat, lon)| vec![*lon, *lat]).collect::<Vec<_>>(),
    })
}

/// Samples SoC roughly every 2km assuming uniform energy density along the leg.
fn sample_soc_curve(geometry: &[(f64, f64)], start_soc: f64, total_energy_wh: f64) -> Vec<Vec<f64>> {
    sample_soc_curve_multi(&[(start_soc, total_energy_wh, geometry.to_vec())])
}

fn sample_soc_curve_multi(segments: &[(f64, f64, Vec<(f64, f64)>)]) -> Vec<Vec<f64>> {
    const STEP_M: f64 = 2000.0;
    let mut out: Vec<Vec<f64>> = Vec::new();
    let mut base_dist = 0.0;
    let mut next_sample = 0.0;

    for (start_soc, energy_wh, geom) in segments {
        if geom.len() < 2 {
            continue;
        }
        let mut cum = 0.0;
        let mut lengths = Vec::with_capacity(geom.len());
        lengths.push(0.0);
        for i in 1..geom.len() {
            cum += haversine_m(geom[i - 1].0, geom[i - 1].1, geom[i].0, geom[i].1);
            lengths.push(cum);
        }
        let seg_len = cum.max(1e-9);
        for i in 0..geom.len() {
            let d_here = base_dist + lengths[i];
            if d_here + 1e-9 >= next_sample {
                let frac = lengths[i] / seg_len;
                let soc = start_soc - (energy_wh * frac) / BATTERY_WH;
                out.push(vec![d_here, soc]);
                next_sample += STEP_M;
            }
        }
        base_dist += cum;
    }
    out
}
