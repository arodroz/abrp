//! Corridor assembly (wayfinder #33): turns a route request into the
//! `CandidateGraph` `search::solve` consumes, per ADR 0006 point 1 ("Candidate
//! corridor") and ADR 0010 point 4 (global waypoint segments). Candidate
//! selection (bbox pre-filter, downsampled-polyline projection, dedup,
//! forward-fanout pruning) is ported from the throwaway vertical-slice
//! prototype (`prototype/slice/src/lib.rs`), which validated the approach at
//! corridor scale (#15).

use std::collections::HashMap;
use std::time::Instant;

use energy::{edge_energy_wh, Calibration, Conditions, EdgeInput, VehicleModel};
use packs::{EdgeHot, Rpack};
use routing::{Route, Router};
use serde::Deserialize;

use crate::types::{
    CandidateGraph, CandidateLeg, CandidateNode, ChargerSite, LegEval, NodeKind, WaypointSpec,
    SPEED_CAPS_KMH,
};

/// A Charging Stop optimiser request: journey endpoints, waypoints in visit
/// order, and the ambient conditions every Leg is evaluated under.
pub struct CorridorRequest {
    pub origin: (f64, f64),
    pub waypoints: Vec<WaypointSpec>,
    pub dest: (f64, f64),
    pub temp_c: f64,
    pub headwind_ms: f64,
}

/// Diagnostics from one `assemble` run.
#[derive(Debug, Clone, Copy, Default)]
pub struct AssemblyStats {
    /// Sites that passed the cheap bbox pre-filter and got the full
    /// polyline-distance check, summed over all segments.
    pub candidates_considered: usize,
    /// Sites that survived corridor membership, dedup, and the ~300 cap.
    pub candidates_kept: usize,
    /// Candidate-graph legs actually materialized (edges added to `out`).
    pub legs_evaluated: usize,
    pub p2p_queries: u32,
    pub corridor_m: f64,
    pub assemble_ms: f64,
    pub p2p_ms: f64,
}

/// Why `assemble` could not build a `CandidateGraph`. Per ADR 0006 point 4
/// this is only ever a road-graph connectivity failure between two adjacent
/// journey points -- SoC infeasibility is never an error, it surfaces as a
/// `Plan`/`Leg` flag from `search::solve`.
#[derive(Debug, Clone, PartialEq)]
pub enum AssembleError {
    SnapFailed {
        lat: f64,
        lon: f64,
    },
    NoRoute {
        from: (f64, f64),
        to: (f64, f64),
    },
    /// The caller's cancel flag was observed set (ADR 0004 point 4).
    Cancelled,
}

impl std::fmt::Display for AssembleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssembleError::SnapFailed { lat, lon } => {
                write!(f, "no pack node near ({lat}, {lon})")
            }
            AssembleError::NoRoute { from, to } => write!(f, "no route from {from:?} to {to:?}"),
            AssembleError::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for AssembleError {}

// ---------------------------------------------------------------------
// Charger Pack parsing
// ---------------------------------------------------------------------

const CPACK_FORMAT: &str = "cpack-1";

#[derive(Debug, Deserialize)]
struct CpackFile {
    format: String,
    chargers: Vec<CpackCharger>,
}

#[derive(Debug, Deserialize)]
struct CpackCharger {
    id: String,
    name: String,
    lat: f64,
    lon: f64,
    max_power_kw: f64,
}

#[derive(Debug)]
pub enum CpackError {
    Json(serde_json::Error),
    UnsupportedFormat(String),
}

impl std::fmt::Display for CpackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CpackError::Json(e) => write!(f, "cpack json error: {e}"),
            CpackError::UnsupportedFormat(got) => write!(f, "unsupported cpack format: {got}"),
        }
    }
}

impl std::error::Error for CpackError {}

impl From<serde_json::Error> for CpackError {
    fn from(e: serde_json::Error) -> Self {
        CpackError::Json(e)
    }
}

/// Parses a Charger Pack (`corridor-chargers.json`-shaped) blob into the
/// sites the corridor layer selects candidates from.
pub fn parse_cpack(json_bytes: &[u8]) -> Result<Vec<ChargerSite>, CpackError> {
    let file: CpackFile = serde_json::from_slice(json_bytes)?;
    if file.format != CPACK_FORMAT {
        return Err(CpackError::UnsupportedFormat(file.format));
    }
    Ok(file
        .chargers
        .into_iter()
        .map(|c| ChargerSite {
            id: c.id,
            name: c.name,
            lat: c.lat,
            lon: c.lon,
            power_kw: c.max_power_kw,
        })
        .collect())
}

// ---------------------------------------------------------------------
// Tunables (ported from the vertical-slice prototype)
// ---------------------------------------------------------------------

/// Reference-polyline downsampling spacing for the corridor pre-filter.
const CORRIDOR_SAMPLE_M: f64 = 2_000.0;
/// Two candidates within this distance of each other are the same physical
/// site (opposite-carriageway duplicates); keep the higher-power one.
const DEDUP_M: f64 = 300.0;
/// Skip a same-segment candidate pair closer than this along the route --
/// not worth a separate stop.
const FORWARD_GAP_M: f64 = 20_000.0;
/// Cap same-segment fan-out per node so assembly stays roughly
/// O(candidates * MAX_FANOUT) rather than O(candidates^2).
const MAX_FANOUT: usize = 30;
/// ADR 0006 point 1: candidates are capped at ~300 across the whole journey.
const MAX_CANDIDATES: usize = 300;
/// A kept candidate this close to a waypoint becomes that waypoint's own
/// Charger rather than a separate node (ADR 0010 point 4).
const WAYPOINT_CHARGER_M: f64 = 150.0;
/// Conservative average consumption for the straight-line feasibility
/// pre-filter (Wh/m).
const AVG_WH_PER_M: f64 = 0.18;
/// Slack multiplier on the straight-line distance in the feasibility
/// pre-filter, to account for roads not being straight lines.
const STRAIGHT_LINE_SLACK: f64 = 1.3;

fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const R_M: f64 = 6_371_000.0;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dphi = (lat2 - lat1).to_radians();
    let dlambda = (lon2 - lon1).to_radians();
    let a = (dphi / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlambda / 2.0).sin().powi(2);
    2.0 * R_M * a.sqrt().asin()
}

/// Conservative-full-battery straight-line feasibility pre-filter (the
/// slice's `straight_line_ok`): a leg is worth an expensive `p2p` query only
/// if even a generous straight-line estimate fits under `avail_wh`. Uses a
/// full battery regardless of caller SoC (ADR: "charger_max 1.0
/// conservatively since caps change consumption") -- real SoC feasibility is
/// the search's job, not assembly's.
fn straight_line_ok(a: (f64, f64), b: (f64, f64), avail_wh: f64) -> bool {
    haversine_m(a.0, a.1, b.0, b.1) * STRAIGHT_LINE_SLACK * AVG_WH_PER_M <= avail_wh
}

// ---------------------------------------------------------------------
// Reference polyline
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct PolyPoint {
    lat: f64,
    lon: f64,
    cum_m: f64,
}

/// Builds one segment's full-resolution reference polyline from its route's
/// edge geometry, deduping the junction vertex shared by consecutive edges.
/// A zero-edge route (source snaps to the same node as target) yields a
/// single point.
fn build_segment_polyline(pack: &Rpack, route: &Route) -> Vec<PolyPoint> {
    if route.edges.is_empty() {
        let n = &pack.nodes()[route.nodes[0] as usize];
        return vec![PolyPoint {
            lat: n.lat as f64,
            lon: n.lon as f64,
            cum_m: 0.0,
        }];
    }
    let mut poly: Vec<PolyPoint> = Vec::new();
    let mut cum = 0.0;
    for (i, &edge_idx) in route.edges.iter().enumerate() {
        let edge = &pack.edges()[edge_idx as usize];
        let verts = pack.geometry_for_edge(edge);
        let skip = if i == 0 { 0 } else { 1 };
        for v in verts.iter().skip(skip) {
            if let Some(prev) = poly.last() {
                cum += haversine_m(prev.lat, prev.lon, v.lat as f64, v.lon as f64);
            }
            poly.push(PolyPoint {
                lat: v.lat as f64,
                lon: v.lon as f64,
                cum_m: cum,
            });
        }
    }
    poly
}

/// Downsamples a polyline to ~`sample_m` spacing for the corridor
/// pre-filter (cheap over-inclusion; the exact `corridor_m` check still
/// applies against these coarse points).
fn downsample(poly: &[PolyPoint], sample_m: f64) -> Vec<PolyPoint> {
    let mut coarse = Vec::new();
    let mut next_sample = 0.0;
    for (i, p) in poly.iter().enumerate() {
        if p.cum_m + 1e-9 >= next_sample || i == poly.len() - 1 {
            coarse.push(*p);
            next_sample += sample_m;
        }
    }
    coarse
}

// ---------------------------------------------------------------------
// Candidate selection
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct SegCandidate {
    /// Index into the caller's `sites` slice.
    site_idx: usize,
    /// Projected position along the segment's reference polyline.
    pos_m: f64,
}

/// Bbox pre-filter, then the ~2 km downsampled-polyline distance check
/// (both from the slice). Returns candidates within `corridor_m`, sorted by
/// route position, plus the count that passed the bbox filter (for stats).
fn project_candidates(
    sites: &[ChargerSite],
    poly: &[PolyPoint],
    coarse: &[PolyPoint],
    corridor_m: f64,
) -> (Vec<SegCandidate>, usize) {
    let mut min_lat = f64::MAX;
    let mut max_lat = f64::MIN;
    let mut min_lon = f64::MAX;
    let mut max_lon = f64::MIN;
    for p in poly {
        min_lat = min_lat.min(p.lat);
        max_lat = max_lat.max(p.lat);
        min_lon = min_lon.min(p.lon);
        max_lon = max_lon.max(p.lon);
    }
    // Conservative (i.e. generous) degrees-per-meter, valid down to ~55
    // degrees latitude -- matches the slice.
    let margin_deg = corridor_m / 70_000.0;
    min_lat -= margin_deg;
    max_lat += margin_deg;
    min_lon -= margin_deg;
    max_lon += margin_deg;

    let mut considered = 0usize;
    let mut out = Vec::new();
    for (i, site) in sites.iter().enumerate() {
        if site.lat < min_lat || site.lat > max_lat || site.lon < min_lon || site.lon > max_lon {
            continue;
        }
        considered += 1;
        let mut best_d = f64::MAX;
        let mut best_pos = 0.0;
        for c in coarse {
            let d = haversine_m(site.lat, site.lon, c.lat, c.lon);
            if d < best_d {
                best_d = d;
                best_pos = c.cum_m;
            }
        }
        if best_d <= corridor_m {
            out.push(SegCandidate {
                site_idx: i,
                pos_m: best_pos,
            });
        }
    }
    out.sort_by(|a, b| a.pos_m.partial_cmp(&b.pos_m).unwrap());
    (out, considered)
}

/// Dedups candidates within `DEDUP_M` of each other, keeping the
/// highest-power one (opposite-carriageway duplicates at the same physical
/// site). Preserves route-position order.
fn dedup_candidates(cands: Vec<SegCandidate>, sites: &[ChargerSite]) -> Vec<SegCandidate> {
    let mut keep = vec![true; cands.len()];
    for i in 0..cands.len() {
        if !keep[i] {
            continue;
        }
        for j in (i + 1)..cands.len() {
            if !keep[j] {
                continue;
            }
            let a = &sites[cands[i].site_idx];
            let b = &sites[cands[j].site_idx];
            if haversine_m(a.lat, a.lon, b.lat, b.lon) <= DEDUP_M {
                if b.power_kw > a.power_kw {
                    keep[i] = false;
                    break;
                } else {
                    keep[j] = false;
                }
            }
        }
    }
    let mut i = 0;
    let mut out = cands;
    out.retain(|_| {
        let k = keep[i];
        i += 1;
        k
    });
    out
}

/// ADR 0006 point 1: caps the whole journey's candidates at `MAX_CANDIDATES`,
/// dropping the lowest-power ones first across all segments.
fn cap_total_candidates(
    mut per_segment: Vec<Vec<SegCandidate>>,
    sites: &[ChargerSite],
) -> Vec<Vec<SegCandidate>> {
    let total: usize = per_segment.iter().map(|v| v.len()).sum();
    if total <= MAX_CANDIDATES {
        return per_segment;
    }
    let mut all: Vec<(usize, usize, f64)> = Vec::new();
    for (s, seg) in per_segment.iter().enumerate() {
        for (c, cand) in seg.iter().enumerate() {
            all.push((s, c, sites[cand.site_idx].power_kw));
        }
    }
    all.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap());
    let n_drop = total - MAX_CANDIDATES;
    let mut drop_flags: Vec<Vec<bool>> = per_segment.iter().map(|v| vec![false; v.len()]).collect();
    for &(s, c, _) in all.iter().take(n_drop) {
        drop_flags[s][c] = true;
    }
    for (s, seg) in per_segment.iter_mut().enumerate() {
        let mut i = 0;
        seg.retain(|_| {
            let d = !drop_flags[s][i];
            i += 1;
            d
        });
    }
    per_segment
}

/// Same-segment forward targets for the node at `positions[from]`: skips
/// anything not more than `FORWARD_GAP_M` ahead, then takes up to
/// `MAX_FANOUT` of the remainder in position order. `positions` is a
/// segment's interior nodes (entry + chargers), ascending by route position.
/// The segment's *terminal* (next waypoint or dest) is never in `positions`
/// -- it is always reachable regardless of gap, handled separately by the
/// caller (ADR 0010 point 4 / the slice's unconditional origin->dest edge).
///
/// The segment's ENTRY node (`from == 0`: the origin or a waypoint) is
/// exempt from both rules: the gap rationale ("you just charged, a stop
/// minutes later is not worth it") doesn't hold there -- a low-SoC
/// departure legitimately needs the charger 2 km away -- and a positional
/// prefix cap from the entry would crowd out the far chargers a high-SoC
/// departure wants. It is one node per segment, so the extra leg
/// evaluations are bounded by the segment's candidate count.
fn forward_targets(positions: &[f64], from: usize) -> Vec<usize> {
    let entry = from == 0;
    let from_pos = positions[from];
    let mut out = Vec::new();
    for (j, &pos) in positions.iter().enumerate().skip(from + 1) {
        if !entry && pos <= from_pos + FORWARD_GAP_M {
            continue;
        }
        if !entry && out.len() >= MAX_FANOUT {
            break;
        }
        out.push(j);
    }
    out
}

// ---------------------------------------------------------------------
// Leg evaluation
// ---------------------------------------------------------------------

/// Evaluates one candidate leg's unpacked road-graph edges once per
/// [`SPEED_CAPS_KMH`] entry (ADR 0010 point 1): per edge, effective speed is
/// `speed_kmh.min(cap)` (untouched when uncapped or already slower), time
/// accumulates `length / v`, energy accumulates `energy::edge_energy_wh`
/// with `delta_v` against the *previous edge's effective speed* under this
/// same cap. The first edge's `delta_v` is its own effective speed -- every
/// leg starts at a stop or the origin (ADR 0006/0010). Pure over
/// `&[EdgeHot]` so it is testable without an open pack.
fn eval_leg(
    edges: &[EdgeHot],
    vehicle: &VehicleModel,
    calib: &Calibration,
    cond: &Conditions,
) -> [LegEval; 4] {
    let mut out = [LegEval {
        time_s: 0.0,
        energy_wh: 0.0,
    }; 4];
    for (k, cap) in SPEED_CAPS_KMH.iter().enumerate() {
        let mut time_s = 0.0;
        let mut energy_wh = 0.0;
        let mut prev_eff = 0.0;
        for (i, edge) in edges.iter().enumerate() {
            let raw = edge.speed_kmh as f64;
            let eff = match cap {
                Some(c) => raw.min(*c),
                None => raw,
            };
            let delta_v = if i == 0 { eff } else { eff - prev_eff };
            if eff > 0.0 {
                time_s += edge.length_m as f64 / (eff / 3.6);
            }
            energy_wh += edge_energy_wh(
                vehicle,
                calib,
                cond,
                &EdgeInput {
                    distance_m: edge.length_m as f64,
                    speed_kmh: eff,
                    delta_v_kmh: delta_v,
                    ascent_m: edge.ascent_m as f64,
                    descent_m: edge.descent_m as f64,
                    road_class: edge.road_class,
                },
            );
            prev_eff = eff;
        }
        out[k] = LegEval { time_s, energy_wh };
    }
    out
}

/// A leg evaluation result: `(length_m, per-cap evals, route edge ids)`.
type CachedRoute = (f64, [LegEval; 4], Vec<u32>);

// ---------------------------------------------------------------------
// Assembly
// ---------------------------------------------------------------------

/// Assembles the candidate graph for one journey (ADR 0006 point 1 as
/// amended by ADR 0010 point 4): snaps and routes each origin/waypoint/dest
/// segment, projects Charger Pack sites onto each segment's corridor,
/// builds the graph nodes (segment-tagged per `types.rs`), and evaluates
/// every candidate Leg once per Speed Cap.
#[allow(clippy::too_many_arguments)]
pub fn assemble(
    pack: &Rpack,
    router: &Router,
    sites: &[ChargerSite],
    veh: &VehicleModel,
    calib: &Calibration,
    req: &CorridorRequest,
    corridor_m: f64,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> Result<(CandidateGraph, AssemblyStats), AssembleError> {
    let t0 = Instant::now();
    let mut p2p_queries: u32 = 0;
    let mut p2p_ms: f64 = 0.0;
    let cond = Conditions {
        temp_c: req.temp_c,
        headwind_ms: req.headwind_ms,
        altitude_m: 0.0,
    };

    // ---- Step 1: snap + route each origin/waypoint/dest segment ----
    let mut points: Vec<(f64, f64)> = Vec::with_capacity(req.waypoints.len() + 2);
    points.push(req.origin);
    points.extend(req.waypoints.iter().map(|w| (w.lat, w.lon)));
    points.push(req.dest);

    let snapped: Vec<u32> = points
        .iter()
        .map(|&(lat, lon)| {
            pack.snap(lat as f32, lon as f32)
                .ok_or(AssembleError::SnapFailed { lat, lon })
        })
        .collect::<Result<_, _>>()?;

    let n_segments = points.len() - 1;
    let mut seg_routes: Vec<Route> = Vec::with_capacity(n_segments);
    for s in 0..n_segments {
        let t = Instant::now();
        let route = router
            .p2p(snapped[s], snapped[s + 1])
            .ok_or(AssembleError::NoRoute {
                from: points[s],
                to: points[s + 1],
            })?;
        p2p_ms += t.elapsed().as_secs_f64() * 1000.0;
        p2p_queries += 1;
        seg_routes.push(route);
    }

    // ---- Step 2: reference polyline per segment ----
    let seg_poly: Vec<Vec<PolyPoint>> = seg_routes
        .iter()
        .map(|r| build_segment_polyline(pack, r))
        .collect();
    let seg_coarse: Vec<Vec<PolyPoint>> = seg_poly
        .iter()
        .map(|p| downsample(p, CORRIDOR_SAMPLE_M))
        .collect();
    let seg_entry_terminal: Vec<([LegEval; 4], Vec<u32>)> = seg_routes
        .iter()
        .map(|r| {
            let edges: Vec<EdgeHot> = r.edges.iter().map(|&i| pack.edges()[i as usize]).collect();
            (eval_leg(&edges, veh, calib, &cond), r.edges.clone())
        })
        .collect();

    // ---- Step 3: candidate selection per segment ----
    let mut considered_total = 0usize;
    let mut per_segment: Vec<Vec<SegCandidate>> = Vec::with_capacity(n_segments);
    for s in 0..n_segments {
        let (cands, considered) =
            project_candidates(sites, &seg_poly[s], &seg_coarse[s], corridor_m);
        considered_total += considered;
        per_segment.push(dedup_candidates(cands, sites));
    }
    per_segment = cap_total_candidates(per_segment, sites);

    // Snap survivors; a candidate the pack can't snap (coverage gap) is
    // dropped rather than failing the whole assembly.
    struct Snapped {
        site_idx: usize,
        pos_m: f64,
        node: u32,
    }
    let mut per_segment_snapped: Vec<Vec<Snapped>> = per_segment
        .into_iter()
        .map(|seg| {
            seg.into_iter()
                .filter_map(|c| {
                    let site = &sites[c.site_idx];
                    pack.snap(site.lat as f32, site.lon as f32)
                        .map(|node| Snapped {
                            site_idx: c.site_idx,
                            pos_m: c.pos_m,
                            node,
                        })
                })
                .collect()
        })
        .collect();

    // ---- Waypoint-charger merge (ADR 0010 point 4) ----
    let n_waypoints = req.waypoints.len();
    let mut waypoint_charger: Vec<Option<usize>> = vec![None; n_waypoints];
    for (w, wp) in req.waypoints.iter().enumerate() {
        let mut found: Option<(usize, usize, f64)> = None; // (segment, idx-in-segment, power)
        for seg in [w, w + 1] {
            for (idx, c) in per_segment_snapped[seg].iter().enumerate() {
                let site = &sites[c.site_idx];
                let d = haversine_m(site.lat, site.lon, wp.lat, wp.lon);
                if d <= WAYPOINT_CHARGER_M && found.is_none_or(|(_, _, p)| site.power_kw > p) {
                    found = Some((seg, idx, site.power_kw));
                }
            }
        }
        if let Some((seg, idx, _)) = found {
            waypoint_charger[w] = Some(per_segment_snapped[seg][idx].site_idx);
            for seg2 in [w, w + 1] {
                per_segment_snapped[seg2].retain(|c| {
                    haversine_m(sites[c.site_idx].lat, sites[c.site_idx].lon, wp.lat, wp.lon)
                        > WAYPOINT_CHARGER_M
                });
            }
        }
    }
    let candidates_kept: usize = per_segment_snapped.iter().map(|v| v.len()).sum();

    // ---- Step 4: graph nodes ----
    let mut graph_sites: Vec<ChargerSite> = Vec::new();
    let mut nodes: Vec<CandidateNode> = Vec::new();
    let mut coord_of: Vec<(f64, f64)> = Vec::new();
    let mut pack_node_of: Vec<u32> = Vec::new();
    // Per-segment interior nodes (entry + chargers), (node_idx, pos_m),
    // ascending by route position -- entry is always first, at pos 0.
    let mut interior_of_segment: Vec<Vec<(u32, f64)>> = vec![Vec::new(); n_segments];
    let mut waypoint_node_idx: Vec<u32> = Vec::with_capacity(n_waypoints);

    nodes.push(CandidateNode {
        kind: NodeKind::Origin,
        segment: 0,
        out: Vec::new(),
    });
    coord_of.push(points[0]);
    pack_node_of.push(snapped[0]);
    interior_of_segment[0].push((0, 0.0));

    for s in 0..n_segments {
        if s > 0 {
            let w = s - 1;
            let charger = waypoint_charger[w].map(|site_idx| {
                graph_sites.push(sites[site_idx].clone());
                (graph_sites.len() - 1) as u32
            });
            let idx = nodes.len() as u32;
            nodes.push(CandidateNode {
                kind: NodeKind::Waypoint {
                    wp: w as u32,
                    charger,
                },
                segment: s as u32,
                out: Vec::new(),
            });
            coord_of.push(points[s]);
            pack_node_of.push(snapped[s]);
            waypoint_node_idx.push(idx);
            interior_of_segment[s].push((idx, 0.0));
        }

        for c in &per_segment_snapped[s] {
            let site = sites[c.site_idx].clone();
            graph_sites.push(site.clone());
            let site_graph_idx = (graph_sites.len() - 1) as u32;
            let idx = nodes.len() as u32;
            nodes.push(CandidateNode {
                kind: NodeKind::Charger {
                    site: site_graph_idx,
                },
                segment: s as u32,
                out: Vec::new(),
            });
            coord_of.push((site.lat, site.lon));
            pack_node_of.push(c.node);
            interior_of_segment[s].push((idx, c.pos_m));
        }
    }

    let dest_idx = nodes.len() as u32;
    nodes.push(CandidateNode {
        kind: NodeKind::Dest,
        segment: (n_segments - 1) as u32,
        out: Vec::new(),
    });
    coord_of.push(points[n_segments]);
    pack_node_of.push(snapped[n_segments]);

    let terminal_of_segment: Vec<u32> = (0..n_segments)
        .map(|s| {
            if s + 1 < n_segments {
                waypoint_node_idx[s]
            } else {
                dest_idx
            }
        })
        .collect();

    // ---- Step 5: legs ----
    // Two phases: decide WHICH legs are wanted (gap/fan-out/feasibility
    // rules, cheap arithmetic), then evaluate the unique pack-node pairs in
    // parallel -- each evaluation is an independent read-only `p2p` +
    // energy pass (`Router` and `Rpack` are `&self` throughout), and this
    // phase dominates assembly wall time.
    let avail_wh_full = veh.usable_capacity_kwh * 1000.0;

    let mut wanted: Vec<(u32, u32)> = Vec::new(); // (from_node_idx, to_node_idx)
    for s in 0..n_segments {
        let interior = &interior_of_segment[s];
        let positions: Vec<f64> = interior.iter().map(|&(_, pos)| pos).collect();
        let terminal_idx = terminal_of_segment[s];

        for (i, &(from_idx, _)) in interior.iter().enumerate() {
            let from_coord = coord_of[from_idx as usize];
            for j in forward_targets(&positions, i) {
                let (to_idx, _) = interior[j];
                if straight_line_ok(from_coord, coord_of[to_idx as usize], avail_wh_full) {
                    wanted.push((from_idx, to_idx));
                }
            }
            // The segment's terminal (next waypoint or dest) is always
            // reachable regardless of forward-gap, so a stop-free segment
            // is representable (ADR 0010 point 4). The entry node's
            // terminal leg is the segment's step-1 reference route -- added
            // below without a query.
            if i != 0
                && straight_line_ok(from_coord, coord_of[terminal_idx as usize], avail_wh_full)
            {
                wanted.push((from_idx, terminal_idx));
            }
        }
    }

    let mut unique_pairs: Vec<(u32, u32)> = wanted
        .iter()
        .map(|&(f, t)| (pack_node_of[f as usize], pack_node_of[t as usize]))
        .collect();
    unique_pairs.sort_unstable();
    unique_pairs.dedup();

    let t_parallel = Instant::now();
    let n_threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(unique_pairs.len().max(1));
    let evaluated: HashMap<(u32, u32), CachedRoute> = std::thread::scope(|scope| {
        let chunk = unique_pairs.len().div_ceil(n_threads).max(1);
        let cond = &cond;
        let handles: Vec<_> = unique_pairs
            .chunks(chunk)
            .map(|pairs| {
                scope.spawn(move || {
                    let mut out = Vec::with_capacity(pairs.len());
                    for &(f, t) in pairs {
                        // Checked per pair (ADR 0004 point 4): each worker
                        // stops early once cancellation is observed, rather
                        // than finishing its whole chunk.
                        if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
                            break;
                        }
                        if let Some(route) = router.p2p(f, t) {
                            let edges: Vec<EdgeHot> = route
                                .edges
                                .iter()
                                .map(|&i| pack.edges()[i as usize])
                                .collect();
                            let evals = eval_leg(&edges, veh, calib, cond);
                            out.push(((f, t), (route.length_m, evals, route.edges)));
                        }
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("leg evaluation thread panicked"))
            .collect()
    });
    p2p_ms += t_parallel.elapsed().as_secs_f64() * 1000.0;
    p2p_queries += unique_pairs.len() as u32;

    if cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed)) {
        return Err(AssembleError::Cancelled);
    }

    let mut legs_evaluated = 0usize;
    for &(from_idx, to_idx) in &wanted {
        let key = (
            pack_node_of[from_idx as usize],
            pack_node_of[to_idx as usize],
        );
        if let Some((dist_m, evals, route_edges)) = evaluated.get(&key) {
            nodes[from_idx as usize].out.push(CandidateLeg {
                to: to_idx,
                dist_m: *dist_m,
                evals: *evals,
                route_edges: route_edges.clone(),
            });
            legs_evaluated += 1;
        }
    }
    for s in 0..n_segments {
        let entry_idx = interior_of_segment[s][0].0;
        let (evals, route_edges) = seg_entry_terminal[s].clone();
        nodes[entry_idx as usize].out.push(CandidateLeg {
            to: terminal_of_segment[s],
            dist_m: seg_routes[s].length_m,
            evals,
            route_edges,
        });
        legs_evaluated += 1;
    }

    let graph = CandidateGraph {
        nodes,
        sites: graph_sites,
        waypoints: req.waypoints.clone(),
        origin: 0,
        dest: dest_idx,
    };

    let stats = AssemblyStats {
        candidates_considered: considered_total,
        candidates_kept,
        legs_evaluated,
        p2p_queries,
        corridor_m,
        assemble_ms: t0.elapsed().as_secs_f64() * 1000.0,
        p2p_ms,
    };

    Ok((graph, stats))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(speed_kmh: f32, length_m: f32, ascent_m: f32, descent_m: f32) -> EdgeHot {
        EdgeHot {
            target: 0,
            length_m,
            speed_kmh,
            ascent_m,
            descent_m,
            road_class: 0,
            guide_flags: 0,
            _pad: [0; 2],
            ch_middle_node: packs::CH_MIDDLE_NODE_NONE,
            geom_offset: 0,
            geom_count: 0,
        }
    }

    fn site(id: &str, lat: f64, lon: f64, power_kw: f64) -> ChargerSite {
        ChargerSite {
            id: id.to_string(),
            name: id.to_string(),
            lat,
            lon,
            power_kw,
        }
    }

    #[test]
    fn parse_cpack_reads_id_name_coords_and_max_power() {
        let json = br#"{
            "format": "cpack-1",
            "region_id": "corridor",
            "built_at_epoch": 0,
            "charger_count": 1,
            "chargers": [
                {
                    "id": "ndw:1", "name": "Fastned Test", "lat": 52.1, "lon": 5.2,
                    "operator": "Fastned", "access": null, "country": "NL",
                    "max_power_kw": 400.0, "connectors": [], "source": "ndw"
                }
            ]
        }"#;
        let sites = parse_cpack(json).unwrap();
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].id, "ndw:1");
        assert_eq!(sites[0].name, "Fastned Test");
        assert_eq!(sites[0].lat, 52.1);
        assert_eq!(sites[0].lon, 5.2);
        assert_eq!(sites[0].power_kw, 400.0);
    }

    #[test]
    fn parse_cpack_rejects_unknown_format() {
        let json = br#"{"format":"cpack-2","region_id":"x","built_at_epoch":0,"charger_count":0,"chargers":[]}"#;
        assert!(matches!(
            parse_cpack(json),
            Err(CpackError::UnsupportedFormat(_))
        ));
    }

    #[test]
    fn corridor_projection_keeps_sites_within_width_and_drops_others() {
        // A straight 10km-north polyline sampled every 2km, at lon 5.0.
        let poly: Vec<PolyPoint> = (0..=10)
            .map(|i| PolyPoint {
                lat: 50.0 + i as f64 * 0.009,
                lon: 5.0,
                cum_m: i as f64 * 1_000.0,
            })
            .collect();
        let coarse = downsample(&poly, CORRIDOR_SAMPLE_M);
        let sites = vec![
            site("near", 50.045, 5.001, 50.0), // ~70m off the line: in
            site("far", 50.045, 5.2, 50.0),    // ~14km off the line: out
        ];
        let (cands, considered) = project_candidates(&sites, &poly, &coarse, 3_000.0);
        assert_eq!(
            considered, 1,
            "the far site should fail the bbox pre-filter"
        );
        assert_eq!(cands.len(), 1);
        assert_eq!(sites[cands[0].site_idx].id, "near");
    }

    #[test]
    fn dedup_keeps_highest_power_within_dedup_radius() {
        let sites = vec![
            site("weak", 50.0, 5.0, 50.0),
            site("strong", 50.0005, 5.0, 150.0), // ~56m away: within DEDUP_M
        ];
        let cands = vec![
            SegCandidate {
                site_idx: 0,
                pos_m: 0.0,
            },
            SegCandidate {
                site_idx: 1,
                pos_m: 10.0,
            },
        ];
        let kept = dedup_candidates(cands, &sites);
        assert_eq!(kept.len(), 1);
        assert_eq!(sites[kept[0].site_idx].id, "strong");
    }

    #[test]
    fn forward_targets_skips_the_gap_and_caps_fanout() {
        // From a charger (from=1): one target inside the 20km gap
        // (skipped), two beyond it (kept).
        let positions = vec![0.0, 1_000.0, 15_000.0, 25_000.0, 50_000.0];
        assert_eq!(forward_targets(&positions, 1), vec![3, 4]);

        // 40 targets all beyond the gap: capped at MAX_FANOUT (30).
        let mut many = vec![0.0, 1.0];
        for i in 1..=40 {
            many.push(FORWARD_GAP_M + 2.0 + i as f64 * FORWARD_GAP_M);
        }
        let targets = forward_targets(&many, 1);
        assert_eq!(targets.len(), MAX_FANOUT);
        assert_eq!(targets, (2..MAX_FANOUT + 2).collect::<Vec<_>>());
    }

    #[test]
    fn forward_targets_entry_node_is_exempt_from_gap_and_cap() {
        // From the segment entry (from=0): the 5km-away charger is kept (a
        // low-SoC departure needs it), and no fan-out cap applies.
        let mut positions = vec![0.0, 5_000.0];
        for i in 1..=40 {
            positions.push(FORWARD_GAP_M + 1.0 + i as f64 * FORWARD_GAP_M);
        }
        let targets = forward_targets(&positions, 0);
        assert_eq!(targets.len(), 41);
        assert_eq!(targets[0], 1);
    }

    #[test]
    fn eval_leg_clamps_speed_to_cap_and_leaves_slower_edges_untouched() {
        let vehicle = VehicleModel::ioniq5_lr_2wd();
        let calib = Calibration::default();
        let cond = Conditions {
            temp_c: 20.0,
            headwind_ms: 0.0,
            altitude_m: 0.0,
        };
        let edges = vec![
            edge(130.0, 10_000.0, 0.0, 0.0),
            edge(60.0, 5_000.0, 0.0, 0.0),
        ];
        let evals = eval_leg(&edges, &vehicle, &calib, &cond);

        // Uncapped (index 0): edge speeds used as-is.
        let uncapped_time = 10_000.0 / (130.0 / 3.6) + 5_000.0 / (60.0 / 3.6);
        assert!((evals[0].time_s - uncapped_time).abs() < 1e-6);

        // Cap 100 (index 2, per SPEED_CAPS_KMH): the 130 edge is clamped to
        // 100, the 60 edge is untouched.
        let capped_time = 10_000.0 / (100.0 / 3.6) + 5_000.0 / (60.0 / 3.6);
        assert!((evals[2].time_s - capped_time).abs() < 1e-6);
        assert!(evals[2].time_s > evals[0].time_s);
    }

    #[test]
    fn eval_leg_first_edge_delta_v_is_standstill_pullaway() {
        let vehicle = VehicleModel::ioniq5_lr_2wd();
        let calib = Calibration::default();
        let cond = Conditions {
            temp_c: 20.0,
            headwind_ms: 0.0,
            altitude_m: 0.0,
        };
        // One edge alone vs. the same edge preceded by an identical edge:
        // the first edge's kinetic term should reflect accelerating from a
        // standstill (delta_v == its own speed), not delta_v == 0.
        let one = eval_leg(&[edge(100.0, 1_000.0, 0.0, 0.0)], &vehicle, &calib, &cond);
        let two = eval_leg(
            &[
                edge(100.0, 1_000.0, 0.0, 0.0),
                edge(100.0, 1_000.0, 0.0, 0.0),
            ],
            &vehicle,
            &calib,
            &cond,
        );
        // The second edge of `two` is at constant speed (delta_v == 0), so
        // it should use less energy per meter than the first (which pays
        // the standstill pull-away kinetic cost).
        let first_edge_wh = one[0].energy_wh;
        let second_edge_wh = two[0].energy_wh - first_edge_wh;
        assert!(second_edge_wh < first_edge_wh);
    }
}
