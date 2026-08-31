//! OSM PBF importer: builds an uncontracted `RegionGraphModel` from one or
//! more raw OSM extracts (e.g. Geofabrik country dumps), ready for
//! `pipeline::ch_prepare`. Ports the throwaway
//! `prototype/vertical-slice`'s `build_pack` bin (highway-class filter,
//! junction detection, degree-2 chain collapse with per-edge geometry,
//! largest-SCC prune) with one change: multiple input files are read as one
//! graph, deduping ways by id so overlapping Geofabrik border extracts
//! (e.g. Luxembourg's edge repeated in the Belgium extract) don't double
//! count shared roads. Node coordinates dedup naturally by id in the
//! coordinate map. See wayfinder ticket #35, docs/adr/0005, ADR 0007.

use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::Command;

use osmpbf::{BlobDecode, BlobReader, Element, ElementReader};
use packs::{
    DestSign, EdgeAttr, EdgeHot, ExitRef, GeomVertex, NodeRecord, RegionGraphModel, SnapGridModel,
    CH_MIDDLE_NODE_NONE, GUIDE_CLASS_LIVING_STREET, GUIDE_CLASS_MOTORWAY, GUIDE_CLASS_NONE,
    GUIDE_CLASS_PRIMARY, GUIDE_CLASS_RESIDENTIAL, GUIDE_CLASS_SECONDARY, GUIDE_CLASS_TERTIARY,
    GUIDE_CLASS_TRUNK, GUIDE_CLASS_UNCLASSIFIED, GUIDE_FLAG_LINK, GUIDE_FLAG_ROUNDABOUT,
};

/// Snap grid cell size, matching `slice_import::SNAP_CELL_SIZE_DEG`.
pub const SNAP_CELL_SIZE_DEG: f32 = 0.1;

const EARTH_R_M: f64 = 6_371_000.0;

fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let p1 = lat1.to_radians();
    let p2 = lat2.to_radians();
    let dphi = (lat2 - lat1).to_radians();
    let dlambda = (lon2 - lon1).to_radians();
    let a = (dphi / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlambda / 2.0).sin().powi(2);
    2.0 * EARTH_R_M * a.sqrt().asin()
}

/// Highway class -> default speed (km/h). `_link` variants get 60% of the
/// parent class's speed.
fn class_default_speed(highway: &str) -> Option<f64> {
    let (base, is_link) = match highway.strip_suffix("_link") {
        Some(b) => (b, true),
        None => (highway, false),
    };
    let base_speed = match base {
        "motorway" => 120.0,
        "trunk" => 100.0,
        "primary" => 80.0,
        "secondary" => 70.0,
        "tertiary" => 60.0,
        "unclassified" => 50.0,
        "residential" => 30.0,
        "living_street" => 20.0,
        _ => return None,
    };
    Some(if is_link {
        base_speed * 0.6
    } else {
        base_speed
    })
}

/// `1` (urban) for `residential`/`living_street`, matching the energy
/// crate's `URBAN_SURCHARGE_WH_PER_KM` contract (`core/energy/src/edge.rs`:
/// any other value is treated as `0`, no surcharge).
fn road_class_for(highway: &str) -> u8 {
    if highway == "residential" || highway == "living_street" {
        1
    } else {
        0
    }
}

/// Highway class -> `GUIDE_CLASS_*` (wayfinder #65). `_link` variants are
/// stripped by the caller before this lookup; anything not in the drivable
/// table (already filtered out by `class_default_speed` before a way's
/// guidance is computed) maps to `GUIDE_CLASS_NONE`.
fn guide_class_for(base_highway: &str) -> u8 {
    match base_highway {
        "motorway" => GUIDE_CLASS_MOTORWAY,
        "trunk" => GUIDE_CLASS_TRUNK,
        "primary" => GUIDE_CLASS_PRIMARY,
        "secondary" => GUIDE_CLASS_SECONDARY,
        "tertiary" => GUIDE_CLASS_TERTIARY,
        "unclassified" => GUIDE_CLASS_UNCLASSIFIED,
        "residential" => GUIDE_CLASS_RESIDENTIAL,
        "living_street" => GUIDE_CLASS_LIVING_STREET,
        _ => GUIDE_CLASS_NONE,
    }
}

/// Turn-by-turn guidance flags (wayfinder #65) for a way: highway class in
/// bits 0-3, `_link` in bit 4, roundabout in bit 5. Deliberately independent
/// of `oneway_dir` -- `junction=circular` sets the roundabout bit here but,
/// unlike `junction=roundabout`, does not imply oneway.
fn guide_flags_for(highway: &str, tags: &HashMap<&str, &str>) -> u8 {
    let (base, is_link) = match highway.strip_suffix("_link") {
        Some(b) => (b, true),
        None => (highway, false),
    };
    let mut flags = guide_class_for(base);
    if is_link {
        flags |= GUIDE_FLAG_LINK;
    }
    if matches!(
        tags.get("junction").copied(),
        Some("roundabout" | "circular")
    ) {
        flags |= GUIDE_FLAG_ROUNDABOUT;
    }
    flags
}

fn parse_maxspeed(v: &str) -> Option<f64> {
    let lower = v.to_ascii_lowercase();
    if lower.contains("mph") {
        return None;
    }
    let digits: String = lower.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<f64>().ok()
}

/// `1` = forward only, `-1` = reverse only, `0` = both directions.
fn oneway_dir(tags: &HashMap<&str, &str>) -> i8 {
    if let Some(v) = tags.get("oneway") {
        match *v {
            "yes" | "true" | "1" => return 1,
            "-1" => return -1,
            _ => {}
        }
    }
    if tags.get("junction") == Some(&"roundabout") {
        return 1;
    }
    0
}

/// Interns strings into a contiguous table (wayfinder #65): id 0 is always
/// the empty string, matching `SECTION_STRING_OFFSETS`/`SECTION_STRING_BLOB`'s
/// on-disk contract. `finish` produces the two arrays the model/writer want
/// directly, so callers never build the offsets by hand.
struct StringInterner {
    strings: Vec<String>,
    index: HashMap<String, u32>,
}

impl StringInterner {
    fn new() -> Self {
        StringInterner {
            strings: vec![String::new()],
            index: HashMap::from([(String::new(), 0)]),
        }
    }

    fn intern(&mut self, s: &str) -> u32 {
        if let Some(&id) = self.index.get(s) {
            return id;
        }
        let id = self.strings.len() as u32;
        self.strings.push(s.to_string());
        self.index.insert(s.to_string(), id);
        id
    }

    fn finish(self) -> (Vec<u32>, Vec<u8>) {
        let mut offsets = Vec::with_capacity(self.strings.len() + 1);
        let mut blob = Vec::new();
        offsets.push(0u32);
        for s in &self.strings {
            blob.extend_from_slice(s.as_bytes());
            offsets.push(blob.len() as u32);
        }
        (offsets, blob)
    }
}

impl Default for StringInterner {
    fn default() -> Self {
        Self::new()
    }
}

/// Interns unique (name_id, ref_id) pairs (wayfinder #65): entry 0 is always
/// `{0, 0}` (unnamed), matching `SECTION_EDGE_ATTRS`'s on-disk contract.
struct AttrInterner {
    attrs: Vec<EdgeAttr>,
    index: HashMap<(u32, u32), u32>,
}

impl AttrInterner {
    fn new() -> Self {
        AttrInterner {
            attrs: vec![EdgeAttr {
                name_id: 0,
                ref_id: 0,
            }],
            index: HashMap::from([((0, 0), 0)]),
        }
    }

    fn intern(&mut self, name_id: u32, ref_id: u32) -> u32 {
        if let Some(&id) = self.index.get(&(name_id, ref_id)) {
            return id;
        }
        let id = self.attrs.len() as u32;
        self.attrs.push(EdgeAttr { name_id, ref_id });
        self.index.insert((name_id, ref_id), id);
        id
    }
}

impl Default for AttrInterner {
    fn default() -> Self {
        Self::new()
    }
}

struct WayRec {
    refs: Vec<i64>,
    oneway: i8,
    speed_kmh: f64,
    road_class: u8,
    attr_idx: u32,
    guide_flags: u8,
    dest_id: u32,
    dest_ref_id: u32,
    junction_ref_id: u32,
}

/// Destination signage describes a way's forward direction only (wayfinder
/// #65: `destination:backward` is deferred), so it's attached only to an
/// edge built in way-forward orientation -- the `oneway == 1` edge and the
/// forward edge of a two-way. The `oneway == -1` edge and the reverse edge
/// of a two-way get zeros (meaning "not present").
fn dest_ids_for_direction(way: &WayRec, forward: bool) -> (u32, u32, u32) {
    if forward {
        (way.dest_id, way.dest_ref_id, way.junction_ref_id)
    } else {
        (0, 0, 0)
    }
}

/// Accumulates drivable ways across one or more PBF files. Deduped by way
/// id: `accept_way` is a no-op (returns `false`) for a way id it has
/// already seen, which is what makes overlapping multi-file border extracts
/// safe to feed in as one input list.
#[derive(Default)]
struct WayAccumulator {
    ways: Vec<WayRec>,
    node_way_count: HashMap<i64, u32>,
    endpoints: HashSet<i64>,
    needed_nodes: HashSet<i64>,
    seen_way_ids: HashSet<i64>,
    strings: StringInterner,
    attrs: AttrInterner,
}

impl WayAccumulator {
    /// Filters to drivable highway classes with >=2 refs, then records the
    /// way and the node bookkeeping (`node_way_count`, `endpoints`,
    /// `needed_nodes`) `import_pbfs` needs for junction detection. Returns
    /// whether the way was newly accepted (`false` for a duplicate id or a
    /// way that doesn't pass the filter).
    fn accept_way(&mut self, way_id: i64, tags: &HashMap<&str, &str>, refs: Vec<i64>) -> bool {
        if !self.seen_way_ids.insert(way_id) {
            return false;
        }
        let Some(highway) = tags.get("highway") else {
            return false;
        };
        let Some(default_speed) = class_default_speed(highway) else {
            return false;
        };
        if refs.len() < 2 {
            return false;
        }
        let speed_kmh = tags
            .get("maxspeed")
            .and_then(|v| parse_maxspeed(v))
            .unwrap_or(default_speed);
        let oneway = oneway_dir(tags);
        let road_class = road_class_for(highway);
        let guide_flags = guide_flags_for(highway, tags);

        let name_id = self.strings.intern(tags.get("name").copied().unwrap_or(""));
        // `ref` may carry a semicolon-separated list (e.g. "A1;E25"); kept
        // as-is and interned as one string -- splitting is a maneuver-time
        // concern, not this ticket's.
        let ref_id = self.strings.intern(tags.get("ref").copied().unwrap_or(""));
        let attr_idx = self.attrs.intern(name_id, ref_id);

        let dest_id = tags
            .get("destination")
            .map(|v| self.strings.intern(v))
            .unwrap_or(0);
        let dest_ref_id = tags
            .get("destination:ref")
            .map(|v| self.strings.intern(v))
            .unwrap_or(0);
        let junction_ref_id = tags
            .get("junction:ref")
            .map(|v| self.strings.intern(v))
            .unwrap_or(0);

        for &n in &refs {
            *self.node_way_count.entry(n).or_insert(0) += 1;
            self.needed_nodes.insert(n);
        }
        self.endpoints.insert(refs[0]);
        self.endpoints.insert(*refs.last().unwrap());
        self.ways.push(WayRec {
            refs,
            oneway,
            speed_kmh,
            road_class,
            attr_idx,
            guide_flags,
            dest_id,
            dest_ref_id,
            junction_ref_id,
        });
        true
    }
}

/// One directed edge produced by chain collapse, before the SCC prune and
/// CSR sort renumber `from`/`to`. `attr_idx`/`guide_flags` carry over from
/// the way regardless of direction; `dest_id`/`dest_ref_id`/`junction_ref_id`
/// only describe the way's forward direction (see `import_pbfs`).
struct BuiltEdge {
    from: u32,
    to: u32,
    length_m: f32,
    speed_kmh: f32,
    road_class: u8,
    geom: Vec<GeomVertex>,
    attr_idx: u32,
    guide_flags: u8,
    dest_id: u32,
    dest_ref_id: u32,
    junction_ref_id: u32,
}

/// Counts and per-file details surfaced from an `import_pbfs` run.
pub struct OsmImportStats {
    pub ways_kept: usize,
    pub junction_nodes: usize,
    pub edges: usize,
    pub dropped_scc_nodes: usize,
    pub file_epochs: Vec<(PathBuf, Option<u64>)>,
    /// Original edges whose interned name is non-empty (wayfinder #65).
    pub named_edges: usize,
    pub dest_sign_edges: usize,
    pub exit_ref_nodes: usize,
}

/// Reads `paths` (e.g. one or more Geofabrik `.osm.pbf` extracts) into an
/// uncontracted, validated `RegionGraphModel`. Every edge's
/// `ch_middle_node` is `CH_MIDDLE_NODE_NONE`, `ch_order` is all zeros, and
/// `ascent_m`/`descent_m` are `0.0` -- elevation is filled in later (see the
/// `TODO(#35)` in `bin/build_packs.rs`).
pub fn import_pbfs(
    paths: &[PathBuf],
) -> Result<(RegionGraphModel, OsmImportStats), Box<dyn Error>> {
    // ---- Pass A (all files): read ways, filter to drivable classes, dedup by way id ----
    let mut acc = WayAccumulator::default();
    for path in paths {
        let reader = ElementReader::from_path(path)?;
        reader.for_each(|el| {
            if let Element::Way(way) = el {
                let tags: HashMap<&str, &str> = way.tags().collect();
                let refs: Vec<i64> = way.refs().collect();
                acc.accept_way(way.id(), &tags, refs);
            }
        })?;
    }
    println!("[osm_import] drivable ways: {}", acc.ways.len());

    // ---- Pass B (all files): node coordinates for nodes referenced by drivable ways, plus
    // motorway_junction exit refs (wayfinder #65) ----
    let mut coords: HashMap<i64, (f32, f32)> = HashMap::with_capacity(acc.needed_nodes.len());
    // Raw (un-interned) ref strings, keyed by osm node id -- interned lazily
    // below, only for the junction nodes that actually survive the SCC
    // prune.
    let mut exit_ref_tags: HashMap<i64, String> = HashMap::new();
    for path in paths {
        let reader = ElementReader::from_path(path)?;
        reader.for_each(|el| match el {
            Element::Node(n) if acc.needed_nodes.contains(&n.id()) => {
                coords.insert(n.id(), (n.lat() as f32, n.lon() as f32));
                let tags: HashMap<&str, &str> = n.tags().collect();
                if tags.get("highway") == Some(&"motorway_junction") {
                    if let Some(&r) = tags.get("ref") {
                        exit_ref_tags.insert(n.id(), r.to_string());
                    }
                }
            }
            Element::DenseNode(n) if acc.needed_nodes.contains(&n.id()) => {
                coords.insert(n.id(), (n.lat() as f32, n.lon() as f32));
                let tags: HashMap<&str, &str> = n.tags().collect();
                if tags.get("highway") == Some(&"motorway_junction") {
                    if let Some(&r) = tags.get("ref") {
                        exit_ref_tags.insert(n.id(), r.to_string());
                    }
                }
            }
            _ => {}
        })?;
    }

    // ---- Junction detection: way endpoints, or nodes shared by >=2 ways ----
    let mut junction_ids: Vec<i64> = acc
        .needed_nodes
        .iter()
        .copied()
        .filter(|n| acc.endpoints.contains(n) || *acc.node_way_count.get(n).unwrap_or(&0) >= 2)
        .collect();
    junction_ids.sort_unstable();
    let junction_index: HashMap<i64, u32> = junction_ids
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i as u32))
        .collect();

    let mut node_lat = Vec::with_capacity(junction_ids.len());
    let mut node_lon = Vec::with_capacity(junction_ids.len());
    for id in &junction_ids {
        let (lat, lon) = *coords.get(id).ok_or_else(|| {
            format!("node {id} is referenced by a drivable way but has no coordinates (truncated or corrupt PBF?)")
        })?;
        node_lat.push(lat);
        node_lon.push(lon);
    }
    println!("[osm_import] junction nodes: {}", junction_ids.len());

    // ---- Chain collapse: split each way into edges between consecutive junctions ----
    let to_geom = |chain: &[(f32, f32)]| -> Vec<GeomVertex> {
        chain
            .iter()
            .map(|&(lat, lon)| GeomVertex {
                lat,
                lon,
                elev_m: 0,
                _pad: 0,
            })
            .collect()
    };

    let mut built_edges: Vec<BuiltEdge> = Vec::new();
    for way in &acc.ways {
        let mut chain_start = 0usize;
        for i in 1..way.refs.len() {
            if !junction_index.contains_key(&way.refs[i]) {
                continue;
            }
            let chain_refs = &way.refs[chain_start..=i];
            chain_start = i;
            if chain_refs.len() < 2 {
                continue;
            }
            let chain_coords: Vec<(f32, f32)> = chain_refs.iter().map(|id| coords[id]).collect();
            let mut length_m = 0.0f64;
            for w in chain_coords.windows(2) {
                length_m += haversine_m(w[0].0 as f64, w[0].1 as f64, w[1].0 as f64, w[1].1 as f64);
            }
            let from = junction_index[&chain_refs[0]];
            let to = junction_index[&chain_refs[chain_refs.len() - 1]];
            if from == to || length_m <= 0.0 {
                continue;
            }
            let speed_kmh = way.speed_kmh as f32;
            let length_m = length_m as f32;
            let mut push_edge = |from: u32, to: u32, geom: Vec<GeomVertex>, forward: bool| {
                let (dest_id, dest_ref_id, junction_ref_id) = dest_ids_for_direction(way, forward);
                built_edges.push(BuiltEdge {
                    from,
                    to,
                    length_m,
                    speed_kmh,
                    road_class: way.road_class,
                    geom,
                    attr_idx: way.attr_idx,
                    guide_flags: way.guide_flags,
                    dest_id,
                    dest_ref_id,
                    junction_ref_id,
                });
            };
            match way.oneway {
                1 => push_edge(from, to, to_geom(&chain_coords), true),
                -1 => {
                    let rev: Vec<(f32, f32)> = chain_coords.iter().rev().copied().collect();
                    push_edge(to, from, to_geom(&rev), false);
                }
                _ => {
                    push_edge(from, to, to_geom(&chain_coords), true);
                    let rev: Vec<(f32, f32)> = chain_coords.iter().rev().copied().collect();
                    push_edge(to, from, to_geom(&rev), false);
                }
            }
        }
    }

    // ---- Keep only the largest strongly connected component (directed reachability) ----
    // Ported verbatim from the slice prototype: Kosaraju's algorithm, iterative
    // DFS to avoid stack overflow on large graphs. See build_pack.rs for the
    // rationale (weakly-connected fragments can still trap a CH query).
    let n = junction_ids.len();
    let mut fwd_adj: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut bwd_adj: Vec<Vec<u32>> = vec![Vec::new(); n];
    for e in &built_edges {
        fwd_adj[e.from as usize].push(e.to);
        bwd_adj[e.to as usize].push(e.from);
    }
    let mut visited = vec![false; n];
    let mut order: Vec<u32> = Vec::with_capacity(n);
    for start in 0..n {
        if visited[start] {
            continue;
        }
        let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
        visited[start] = true;
        while let Some(&mut (node, ref mut ci)) = stack.last_mut() {
            if *ci < fwd_adj[node].len() {
                let next = fwd_adj[node][*ci] as usize;
                *ci += 1;
                if !visited[next] {
                    visited[next] = true;
                    stack.push((next, 0));
                }
            } else {
                order.push(node as u32);
                stack.pop();
            }
        }
    }
    let mut comp: Vec<i32> = vec![-1; n];
    let mut comp_size: HashMap<i32, usize> = HashMap::new();
    let mut next_comp = 0i32;
    for &node in order.iter().rev() {
        let node = node as usize;
        if comp[node] != -1 {
            continue;
        }
        let mut stack = vec![node];
        comp[node] = next_comp;
        let mut size = 0usize;
        while let Some(u) = stack.pop() {
            size += 1;
            for &v in &bwd_adj[u] {
                let v = v as usize;
                if comp[v] == -1 {
                    comp[v] = next_comp;
                    stack.push(v);
                }
            }
        }
        comp_size.insert(next_comp, size);
        next_comp += 1;
    }
    let keep: Vec<bool> = if n == 0 {
        Vec::new()
    } else {
        let biggest = *comp_size.iter().max_by_key(|(_, sz)| **sz).unwrap().0;
        (0..n).map(|i| comp[i] == biggest).collect()
    };
    let dropped_scc_nodes = n - keep.iter().filter(|k| **k).count();
    if dropped_scc_nodes > 0 {
        println!("[osm_import] dropping {dropped_scc_nodes} nodes outside the largest connected component");
    }

    let mut remap: Vec<u32> = vec![u32::MAX; n];
    let mut new_lat = Vec::new();
    let mut new_lon = Vec::new();
    for i in 0..n {
        if keep[i] {
            remap[i] = new_lat.len() as u32;
            new_lat.push(node_lat[i]);
            new_lon.push(node_lon[i]);
        }
    }

    let mut kept_edges: Vec<BuiltEdge> = built_edges
        .into_iter()
        .filter(|e| keep[e.from as usize] && keep[e.to as usize])
        .map(|mut e| {
            e.from = remap[e.from as usize];
            e.to = remap[e.to as usize];
            e
        })
        .collect();

    // Sort by (from, to) so the runtime can binary-search a CSR row.
    kept_edges.sort_by_key(|e| (e.from, e.to));

    let node_count = new_lat.len();
    let mut csr_first_edge = vec![0u32; node_count + 1];
    for e in &kept_edges {
        csr_first_edge[e.from as usize + 1] += 1;
    }
    for i in 0..node_count {
        csr_first_edge[i + 1] += csr_first_edge[i];
    }

    let mut geometry: Vec<GeomVertex> = Vec::new();
    let edges: Vec<EdgeHot> = kept_edges
        .iter()
        .map(|e| {
            let geom_offset = geometry.len() as u32;
            geometry.extend_from_slice(&e.geom);
            EdgeHot {
                target: e.to,
                length_m: e.length_m,
                speed_kmh: e.speed_kmh,
                ascent_m: 0.0,
                descent_m: 0.0,
                road_class: e.road_class,
                guide_flags: e.guide_flags,
                _pad: [0; 2],
                ch_middle_node: CH_MIDDLE_NODE_NONE,
                geom_offset,
                geom_count: e.geom.len() as u32,
            }
        })
        .collect();
    let n_edges = edges.len();

    // Format 2.0 guidance (wayfinder #65). `kept_edges` is all-original at
    // this point (CH hasn't run), so `edge_guide` is just each edge's
    // `attr_idx` in edge-slot order -- never GUIDE_NONE here.
    let edge_guide: Vec<u32> = kept_edges.iter().map(|e| e.attr_idx).collect();
    let named_edges = kept_edges
        .iter()
        .filter(|e| acc.attrs.attrs[e.attr_idx as usize].name_id != 0)
        .count();
    let dest_signs: Vec<DestSign> = kept_edges
        .iter()
        .enumerate()
        .filter(|(_, e)| e.dest_id != 0 || e.dest_ref_id != 0 || e.junction_ref_id != 0)
        .map(|(edge_slot, e)| DestSign {
            edge_slot: edge_slot as u32,
            dest_id: e.dest_id,
            dest_ref_id: e.dest_ref_id,
            junction_ref_id: e.junction_ref_id,
        })
        .collect();
    let dest_sign_edges = dest_signs.len();

    // Exit refs (wayfinder #65): walk `junction_ids` in its existing
    // ascending-osm-id order, skipping SCC-dropped nodes -- `remap` assigns
    // final node ids in that same relative order, so this comes out sorted
    // by final node_id with no extra sort needed.
    let mut exit_refs: Vec<ExitRef> = Vec::new();
    for (i, osm_id) in junction_ids.iter().enumerate() {
        if !keep[i] {
            continue;
        }
        if let Some(raw_ref) = exit_ref_tags.get(osm_id) {
            let ref_id = acc.strings.intern(raw_ref);
            exit_refs.push(ExitRef {
                node_id: remap[i],
                ref_id,
            });
        }
    }
    let exit_ref_nodes = exit_refs.len();

    let edge_attrs = acc.attrs.attrs.clone();
    let (string_offsets, string_blob) = acc.strings.finish();

    let nodes: Vec<NodeRecord> = new_lat
        .into_iter()
        .zip(new_lon)
        .map(|(lat, lon)| NodeRecord { lat, lon })
        .collect();
    let snap_grid = build_snap_grid(&nodes, SNAP_CELL_SIZE_DEG);

    let model = RegionGraphModel {
        nodes,
        csr_first_edge,
        edges,
        ch_order: vec![0u32; node_count],
        geometry,
        snap_grid,
        string_offsets,
        string_blob,
        edge_attrs,
        edge_guide,
        dest_signs,
        exit_refs,
    };
    model.validate()?;

    let file_epochs = paths.iter().map(|p| (p.clone(), pbf_epoch(p))).collect();

    println!(
        "[osm_import] guidance: {named_edges} named edges, {dest_sign_edges} destination signs, {exit_ref_nodes} exit refs"
    );

    Ok((
        model,
        OsmImportStats {
            ways_kept: acc.ways.len(),
            junction_nodes: node_count,
            edges: n_edges,
            dropped_scc_nodes,
            file_epochs,
            named_edges,
            dest_sign_edges,
            exit_ref_nodes,
        },
    ))
}

/// Builds a `cell_size_deg`-wide snap grid over `nodes`'s own bounding box.
/// Same approach as `slice_import`'s private helper of the same name (kept
/// separate since that module isn't meant to be touched by this ticket).
fn build_snap_grid(nodes: &[NodeRecord], cell_size_deg: f32) -> SnapGridModel {
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

    let n_rows = (((max_lat - min_lat) / cell_size_deg).ceil() as u32).max(1);
    let n_cols = (((max_lon - min_lon) / cell_size_deg).ceil() as u32).max(1);

    let grid_cell = |lat: f32, lon: f32| -> (u32, u32) {
        let row =
            (((lat - min_lat) / cell_size_deg).floor() as i64).clamp(0, n_rows as i64 - 1) as u32;
        let col =
            (((lon - min_lon) / cell_size_deg).floor() as i64).clamp(0, n_cols as i64 - 1) as u32;
        (row, col)
    };

    let n_cells = (n_rows * n_cols) as usize;
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); n_cells];
    for (i, n) in nodes.iter().enumerate() {
        let (row, col) = grid_cell(n.lat, n.lon);
        buckets[(row * n_cols + col) as usize].push(i as u32);
    }
    let mut cell_offsets = Vec::with_capacity(n_cells + 1);
    let mut node_ids = Vec::new();
    cell_offsets.push(0u32);
    for bucket in &buckets {
        node_ids.extend_from_slice(bucket);
        cell_offsets.push(node_ids.len() as u32);
    }

    SnapGridModel {
        min_lat,
        min_lon,
        cell_size_deg,
        n_rows,
        n_cols,
        cell_offsets,
        node_ids,
    }
}

/// Reads the OSMHeader blob's `osmosis_replication_timestamp` (Unix epoch
/// seconds marking the extract's data snapshot) from `path`. Tries the
/// `osmpbf` crate's blob-level API first; falls back to shelling out to
/// `osmium fileinfo` if the crate can't decode a header (e.g. a
/// non-Osmosis PBF writer that used a different header option). `None` if
/// neither path finds a timestamp.
pub fn pbf_epoch(path: &Path) -> Option<u64> {
    pbf_epoch_via_crate(path).or_else(|| pbf_epoch_via_osmium(path))
}

fn pbf_epoch_via_crate(path: &Path) -> Option<u64> {
    let reader = BlobReader::from_path(path).ok()?;
    for blob in reader {
        let blob = blob.ok()?;
        if let Ok(BlobDecode::OsmHeader(header)) = blob.decode() {
            return header
                .osmosis_replication_timestamp()
                .and_then(|ts| u64::try_from(ts).ok());
        }
    }
    None
}

fn pbf_epoch_via_osmium(path: &Path) -> Option<u64> {
    let output = Command::new("osmium")
        .args([
            "fileinfo",
            "-g",
            "header.option.osmosis_replication_timestamp",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    parse_rfc3339_epoch(text.trim())
}

/// Minimal RFC 3339 UTC parser for osmium's `YYYY-MM-DDTHH:MM:SSZ` output
/// -- avoids pulling in a date/time crate for one fallback path.
fn parse_rfc3339_epoch(s: &str) -> Option<u64> {
    let s = s.strip_suffix('Z')?;
    let (date, time) = s.split_once('T')?;
    let mut d = date.split('-');
    let year: i64 = d.next()?.parse().ok()?;
    let month: u32 = d.next()?.parse().ok()?;
    let day: u32 = d.next()?.parse().ok()?;
    let mut t = time.split(':');
    let hour: i64 = t.next()?.parse().ok()?;
    let minute: i64 = t.next()?.parse().ok()?;
    let second: i64 = t.next()?.parse().ok()?;

    let days = days_from_civil(year, month, day);
    let secs = days * 86_400 + hour * 3600 + minute * 60 + second;
    u64::try_from(secs).ok()
}

/// Howard Hinnant's `days_from_civil`: proleptic-Gregorian day count since
/// the Unix epoch for a UTC calendar date.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(pairs: &[(&'static str, &'static str)]) -> HashMap<&'static str, &'static str> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn class_default_speed_covers_the_drivable_table_incl_living_street() {
        assert_eq!(class_default_speed("motorway"), Some(120.0));
        assert_eq!(class_default_speed("residential"), Some(30.0));
        assert_eq!(class_default_speed("living_street"), Some(20.0));
        assert_eq!(class_default_speed("motorway_link"), Some(72.0));
        assert_eq!(class_default_speed("footway"), None);
    }

    #[test]
    fn road_class_marks_residential_and_living_street_as_urban() {
        assert_eq!(road_class_for("residential"), 1);
        assert_eq!(road_class_for("living_street"), 1);
        assert_eq!(road_class_for("primary"), 0);
        assert_eq!(road_class_for("motorway"), 0);
    }

    #[test]
    fn maxspeed_parses_leading_digits_and_rejects_mph() {
        assert_eq!(parse_maxspeed("50"), Some(50.0));
        assert_eq!(parse_maxspeed("50 km/h"), Some(50.0));
        assert_eq!(parse_maxspeed("30 mph"), None);
        assert_eq!(parse_maxspeed("walk"), None);
    }

    #[test]
    fn oneway_detects_explicit_tag_and_roundabout_junction() {
        assert_eq!(oneway_dir(&tags(&[("oneway", "yes")])), 1);
        assert_eq!(oneway_dir(&tags(&[("oneway", "-1")])), -1);
        assert_eq!(oneway_dir(&tags(&[("junction", "roundabout")])), 1);
        assert_eq!(oneway_dir(&tags(&[])), 0);
    }

    #[test]
    fn way_accumulator_dedups_by_way_id() {
        let mut acc = WayAccumulator::default();
        let t = tags(&[("highway", "residential")]);
        assert!(acc.accept_way(1, &t, vec![10, 11]));
        assert!(!acc.accept_way(1, &t, vec![10, 11]));
        assert_eq!(acc.ways.len(), 1);
    }

    #[test]
    fn way_accumulator_rejects_non_drivable_and_too_short_ways() {
        let mut acc = WayAccumulator::default();
        assert!(!acc.accept_way(1, &tags(&[("highway", "footway")]), vec![10, 11]));
        assert!(!acc.accept_way(2, &tags(&[("highway", "residential")]), vec![10]));
        assert_eq!(acc.ways.len(), 0);
    }

    #[test]
    fn way_accumulator_tracks_endpoints_and_node_way_counts() {
        let mut acc = WayAccumulator::default();
        let t = tags(&[("highway", "residential")]);
        acc.accept_way(1, &t, vec![10, 11, 12]);
        acc.accept_way(2, &t, vec![12, 13]);
        assert!(acc.endpoints.contains(&10));
        assert!(acc.endpoints.contains(&12));
        assert!(acc.endpoints.contains(&13));
        assert_eq!(acc.node_way_count[&12], 2);
    }

    #[test]
    fn rfc3339_epoch_parses_osmium_fileinfo_output() {
        // 2026-08-26T20:22:15Z, cross-checked against `date -u`.
        assert_eq!(
            parse_rfc3339_epoch("2026-08-26T20:22:15Z"),
            Some(1_787_775_735)
        );
    }

    // --- Format 2.0 guidance (wayfinder #65) --------------------------

    #[test]
    fn guide_flags_strips_link_suffix_and_sets_the_link_bit() {
        let flags = guide_flags_for("motorway_link", &tags(&[("highway", "motorway_link")]));
        assert_eq!(flags & 0x0F, packs::GUIDE_CLASS_MOTORWAY);
        assert_ne!(flags & packs::GUIDE_FLAG_LINK, 0);
        assert_eq!(flags & packs::GUIDE_FLAG_ROUNDABOUT, 0);
    }

    #[test]
    fn guide_flags_maps_residential_with_no_link_bit() {
        let flags = guide_flags_for("residential", &tags(&[("highway", "residential")]));
        assert_eq!(flags & 0x0F, packs::GUIDE_CLASS_RESIDENTIAL);
        assert_eq!(flags & packs::GUIDE_FLAG_LINK, 0);
    }

    #[test]
    fn guide_flags_roundabout_and_circular_both_set_the_roundabout_bit() {
        let roundabout = guide_flags_for(
            "primary",
            &tags(&[("highway", "primary"), ("junction", "roundabout")]),
        );
        let circular = guide_flags_for(
            "primary",
            &tags(&[("highway", "primary"), ("junction", "circular")]),
        );
        assert_ne!(roundabout & packs::GUIDE_FLAG_ROUNDABOUT, 0);
        assert_ne!(circular & packs::GUIDE_FLAG_ROUNDABOUT, 0);

        // Only `junction=roundabout` implies oneway; `circular` does not
        // (behavior unchanged from before this ticket).
        assert_eq!(
            oneway_dir(&tags(&[("highway", "primary"), ("junction", "roundabout")])),
            1
        );
        assert_eq!(
            oneway_dir(&tags(&[("highway", "primary"), ("junction", "circular")])),
            0
        );
    }

    #[test]
    fn accept_way_interns_name_ref_and_dedups_attr_pairs_across_ways() {
        let mut acc = WayAccumulator::default();
        let t1 = tags(&[
            ("highway", "residential"),
            ("name", "Avenue A"),
            ("ref", "A6"),
        ]);
        acc.accept_way(1, &t1, vec![10, 11]);
        // A second, distinct way with the identical (name, ref) pair should
        // reuse the same interned attr, not create a duplicate.
        let t2 = tags(&[
            ("highway", "residential"),
            ("name", "Avenue A"),
            ("ref", "A6"),
        ]);
        acc.accept_way(2, &t2, vec![12, 13]);
        assert_eq!(acc.ways[0].attr_idx, acc.ways[1].attr_idx);
        // attrs: {0,0} (unnamed) plus the one interned pair.
        assert_eq!(acc.attrs.attrs.len(), 2);

        // A way with no name/ref at all gets the unnamed attr (id 0).
        let t3 = tags(&[("highway", "residential")]);
        acc.accept_way(3, &t3, vec![14, 15]);
        assert_eq!(acc.ways[2].attr_idx, 0);
    }

    #[test]
    fn destination_signage_only_attaches_to_the_forward_direction() {
        let way = WayRec {
            refs: vec![10, 11],
            oneway: 0,
            speed_kmh: 50.0,
            road_class: 0,
            attr_idx: 0,
            guide_flags: 0,
            dest_id: 7,
            dest_ref_id: 8,
            junction_ref_id: 9,
        };
        assert_eq!(dest_ids_for_direction(&way, true), (7, 8, 9));
        assert_eq!(dest_ids_for_direction(&way, false), (0, 0, 0));
    }
}
