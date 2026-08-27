// Throwaway performance prototype: OSM PBF + charger feeds -> pack directory.
// See ../../README.md. Two full passes over the PBF (ways, then nodes) -- simple,
// not memory-efficient; fine for a Luxembourg-sized extract, not for BE/NL.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::time::Instant;

use flate2::read::GzDecoder;
use osmpbf::{Element, ElementReader};
use serde::Deserialize;
use serde_json::json;

use planner::{Charger, EdgeMeta, EdgesMetaFile, NodesFile};

struct Args {
    pbf: String,
    out: String,
    ndw: Option<String>,
    road: Option<String>,
    kml: Option<String>,
}

fn parse_args() -> Args {
    let mut pbf = None;
    let mut out = None;
    let mut ndw = None;
    let mut road = None;
    let mut kml = None;
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--pbf" => pbf = it.next(),
            "--out" => out = it.next(),
            "--ndw" => ndw = it.next(),
            "--road" => road = it.next(),
            "--kml" => kml = it.next(),
            _ => {}
        }
    }
    Args {
        pbf: pbf.expect("--pbf <path> required"),
        out: out.expect("--out <dir> required"),
        ndw,
        road,
        kml,
    }
}

/// Highway class -> default speed (km/h). `_link` variants get 60% of the parent class.
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
        _ => return None,
    };
    Some(if is_link { base_speed * 0.6 } else { base_speed })
}

fn parse_maxspeed(v: &str) -> Option<f64> {
    let lower = v.to_ascii_lowercase();
    if lower.contains("mph") {
        return None;
    }
    let digits: String = lower.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<f64>().ok()
}

/// 1 = forward only, -1 = reverse only, 0 = both directions.
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

fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    planner::haversine_m(lat1, lon1, lat2, lon2)
}

struct WayRec {
    refs: Vec<i64>,
    oneway: i8,
    speed_kmh: f64,
}

fn main() {
    let args = parse_args();
    let out_dir = PathBuf::from(&args.out);
    fs::create_dir_all(&out_dir).expect("create out dir");

    // ---- Pass A: read ways, filter to drivable classes ----
    let mut ways: Vec<WayRec> = Vec::new();
    let mut node_way_count: HashMap<i64, u32> = HashMap::new();
    let mut endpoints: HashSet<i64> = HashSet::new();
    let mut needed_nodes: HashSet<i64> = HashSet::new();

    let reader = ElementReader::from_path(&args.pbf).expect("open pbf");
    reader
        .for_each(|el| {
            if let Element::Way(way) = el {
                let tags: HashMap<&str, &str> = way.tags().collect();
                let Some(highway) = tags.get("highway") else { return };
                let Some(default_speed) = class_default_speed(highway) else { return };
                let speed_kmh = tags
                    .get("maxspeed")
                    .and_then(|v| parse_maxspeed(v))
                    .unwrap_or(default_speed);
                let oneway = oneway_dir(&tags);
                let refs: Vec<i64> = way.refs().collect();
                if refs.len() < 2 {
                    return;
                }
                for &n in &refs {
                    *node_way_count.entry(n).or_insert(0) += 1;
                    needed_nodes.insert(n);
                }
                endpoints.insert(refs[0]);
                endpoints.insert(*refs.last().unwrap());
                ways.push(WayRec { refs, oneway, speed_kmh });
            }
        })
        .expect("read pbf ways");

    println!("[build_pack] drivable ways: {}", ways.len());

    // ---- Pass B: read node coordinates for nodes referenced by drivable ways ----
    let mut coords: HashMap<i64, (f32, f32)> = HashMap::with_capacity(needed_nodes.len());
    let reader = ElementReader::from_path(&args.pbf).expect("open pbf (pass 2)");
    reader
        .for_each(|el| match el {
            Element::Node(n) => {
                if needed_nodes.contains(&n.id()) {
                    coords.insert(n.id(), (n.lat() as f32, n.lon() as f32));
                }
            }
            Element::DenseNode(n) => {
                if needed_nodes.contains(&n.id()) {
                    coords.insert(n.id(), (n.lat() as f32, n.lon() as f32));
                }
            }
            _ => {}
        })
        .expect("read pbf nodes");

    // ---- Junction detection: way endpoints, or nodes shared by >=2 ways ----
    let mut junction_ids: Vec<i64> = needed_nodes
        .iter()
        .copied()
        .filter(|n| endpoints.contains(n) || *node_way_count.get(n).unwrap_or(&0) >= 2)
        .collect();
    junction_ids.sort_unstable();
    let junction_index: HashMap<i64, u32> = junction_ids
        .iter()
        .enumerate()
        .map(|(i, &id)| (id, i as u32))
        .collect();

    let node_lat: Vec<f32> = junction_ids.iter().map(|id| coords[id].0).collect();
    let node_lon: Vec<f32> = junction_ids.iter().map(|id| coords[id].1).collect();

    println!("[build_pack] junction nodes: {}", junction_ids.len());

    // ---- Build compressed edges (degree-2 chain collapse) + fast_paths input graph ----
    // Geometry is a raw little-endian f32 byte blob (not bincode-wrapped) so the
    // runtime can mmap geometry.bin directly instead of deserializing it all into RAM;
    // geom_offset/geom_len in EdgeMeta count *points* (8 bytes each: lat then lon).
    let mut input_graph = fast_paths::InputGraph::new();
    let mut edges: Vec<EdgeMeta> = Vec::new();
    let mut geometry_bytes: Vec<u8> = Vec::new();

    let mut add_edge = |from: u32, to: u32, length_m: f64, speed_kmh: f64, chain_geom: &[(f32, f32)]| {
        let v_ms = speed_kmh / 3.6;
        let weight_ms = ((length_m / v_ms) * 1000.0).round().max(1.0) as usize;
        input_graph.add_edge(from as usize, to as usize, weight_ms);
        let offset = (geometry_bytes.len() / 8) as u32;
        for (lat, lon) in chain_geom {
            geometry_bytes.extend_from_slice(&lat.to_le_bytes());
            geometry_bytes.extend_from_slice(&lon.to_le_bytes());
        }
        edges.push(EdgeMeta {
            from,
            to,
            length_m: length_m as f32,
            speed_kmh: speed_kmh as f32,
            geom_offset: offset,
            geom_len: chain_geom.len() as u32,
        });
    };

    for way in &ways {
        // Split the way's node list into chains between consecutive junction nodes.
        let mut chain_start = 0usize;
        for i in 1..way.refs.len() {
            if junction_index.contains_key(&way.refs[i]) {
                let chain_refs = &way.refs[chain_start..=i];
                if chain_refs.len() >= 2 {
                    let chain_geom: Vec<(f32, f32)> =
                        chain_refs.iter().map(|id| coords[id]).collect();
                    let mut length_m = 0.0;
                    for w in chain_geom.windows(2) {
                        length_m += haversine_m(w[0].0 as f64, w[0].1 as f64, w[1].0 as f64, w[1].1 as f64);
                    }
                    let from = junction_index[&chain_refs[0]];
                    let to = junction_index[&chain_refs[chain_refs.len() - 1]];
                    if from != to && length_m > 0.0 {
                        match way.oneway {
                            1 => add_edge(from, to, length_m, way.speed_kmh, &chain_geom),
                            -1 => {
                                let rev: Vec<(f32, f32)> = chain_geom.iter().rev().copied().collect();
                                add_edge(to, from, length_m, way.speed_kmh, &rev)
                            }
                            _ => {
                                add_edge(from, to, length_m, way.speed_kmh, &chain_geom);
                                let rev: Vec<(f32, f32)> = chain_geom.iter().rev().copied().collect();
                                add_edge(to, from, length_m, way.speed_kmh, &rev);
                            }
                        }
                    }
                }
                chain_start = i;
            }
        }
    }

    // ---- Keep only the largest strongly connected component (directed reachability). ----
    // Real OSM extracts have small disconnected/one-way-trapped fragments (private
    // driveways, tile-boundary clipping, tagging quirks); routing across them is
    // meaningless, and a merely *weakly* connected component can still contain
    // directed dead-end pockets a CH query can never escape. Kosaraju's algorithm,
    // iterative DFS to avoid stack overflow on ~35k nodes.
    let n = junction_ids.len();
    let mut fwd_adj: Vec<Vec<u32>> = vec![Vec::new(); n];
    let mut bwd_adj: Vec<Vec<u32>> = vec![Vec::new(); n];
    for e in &edges {
        fwd_adj[e.from as usize].push(e.to);
        bwd_adj[e.to as usize].push(e.from);
    }
    let mut visited = vec![false; n];
    let mut order: Vec<u32> = Vec::with_capacity(n);
    for start in 0..n {
        if visited[start] {
            continue;
        }
        // (node, next child index to visit)
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
    let biggest = *comp_size.iter().max_by_key(|(_, sz)| **sz).unwrap().0;
    let keep: Vec<bool> = (0..n).map(|i| comp[i] == biggest).collect();
    let dropped = n - keep.iter().filter(|k| **k).count();
    if dropped > 0 {
        println!(
            "[build_pack] dropping {dropped} nodes outside the largest connected component"
        );
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
    let node_lat = new_lat;
    let node_lon = new_lon;
    let mut edges: Vec<EdgeMeta> = edges
        .into_iter()
        .filter(|e| keep[e.from as usize] && keep[e.to as usize])
        .map(|mut e| {
            e.from = remap[e.from as usize];
            e.to = remap[e.to as usize];
            e
        })
        .collect();

    // Sort by (from, to) so the runtime can binary-search a CSR row instead of
    // building a `(from,to) -> edge` HashMap (which gets large at corridor scale).
    edges.sort_unstable_by_key(|e| (e.from, e.to));
    let node_count = node_lat.len();
    let mut from_start: Vec<u32> = vec![0; node_count + 1];
    for e in &edges {
        from_start[e.from as usize + 1] += 1;
    }
    for i in 0..node_count {
        from_start[i + 1] += from_start[i];
    }

    let mut input_graph = fast_paths::InputGraph::new();
    for e in &edges {
        let v_ms = e.speed_kmh as f64 / 3.6;
        let weight_ms = ((e.length_m as f64 / v_ms) * 1000.0).round().max(1.0) as usize;
        input_graph.add_edge(e.from as usize, e.to as usize, weight_ms);
    }

    println!(
        "[build_pack] input graph: {} nodes, {} directed edges",
        node_lat.len(),
        edges.len()
    );

    input_graph.freeze();
    let prep_start = Instant::now();
    let fast_graph = fast_paths::prepare(&input_graph);
    let prep_s = prep_start.elapsed().as_secs_f64();
    println!("[build_pack] fast_paths::prepare() took {:.3}s", prep_s);

    fs::write(out_dir.join("graph.bin"), bincode::serialize(&fast_graph).unwrap()).unwrap();
    fs::write(
        out_dir.join("nodes.bin"),
        bincode::serialize(&NodesFile { lat: node_lat, lon: node_lon }).unwrap(),
    )
    .unwrap();
    fs::write(
        out_dir.join("edges_meta.bin"),
        bincode::serialize(&EdgesMetaFile { edges, from_start }).unwrap(),
    )
    .unwrap();
    fs::write(out_dir.join("geometry.bin"), &geometry_bytes).unwrap();

    // ---- Chargers ----
    let mut chargers: Vec<Charger> = Vec::new();
    if let Some(path) = &args.ndw {
        chargers.extend(parse_ocpi_gz(path, "NL"));
    }
    if let Some(path) = &args.road {
        chargers.extend(parse_ocpi_plain(path, "BE"));
    }
    if let Some(path) = &args.kml {
        chargers.extend(parse_chargy_kml(path));
    }
    println!("[build_pack] chargers (>=50kW CCS): {}", chargers.len());

    fs::write(out_dir.join("chargers.bin"), bincode::serialize(&chargers).unwrap()).unwrap();

    let geojson = json!({
        "type": "FeatureCollection",
        "features": chargers.iter().map(|c| json!({
            "type": "Feature",
            "geometry": {"type": "Point", "coordinates": [c.lon, c.lat]},
            "properties": {"name": c.name, "power_kw": c.power_kw},
        })).collect::<Vec<_>>(),
    });
    fs::write(
        out_dir.join("chargers.geojson"),
        serde_json::to_string(&geojson).unwrap(),
    )
    .unwrap();

    println!("[build_pack] pack written to {}", out_dir.display());
}

// ---------------------------------------------------------------------
// Charger parsing (OCPI 2.2.1 Locations for NL/BE, KML for LU)
// ---------------------------------------------------------------------

#[derive(Deserialize)]
struct OcpiLocation {
    name: Option<String>,
    country_code: Option<String>,
    coordinates: Option<OcpiCoords>,
    #[serde(default)]
    evses: Vec<OcpiEvse>,
}
#[derive(Deserialize)]
struct OcpiCoords {
    latitude: String,
    longitude: String,
}
#[derive(Deserialize)]
struct OcpiEvse {
    #[serde(default)]
    connectors: Vec<OcpiConnector>,
}
#[derive(Deserialize)]
struct OcpiConnector {
    standard: Option<String>,
    max_electric_power: Option<f64>,
    max_voltage: Option<f64>,
    max_amperage: Option<f64>,
}

const MIN_DC_POWER_W: f64 = 50_000.0;

fn ocpi_location_to_charger(loc: &OcpiLocation, default_country: &str) -> Option<Charger> {
    let coords = loc.coordinates.as_ref()?;
    let lat: f64 = coords.latitude.parse().ok()?;
    let lon: f64 = coords.longitude.parse().ok()?;
    let mut best_power_w = 0.0f64;
    for evse in &loc.evses {
        for c in &evse.connectors {
            let is_ccs = c.standard.as_deref().unwrap_or("").to_uppercase().contains("COMBO");
            if !is_ccs {
                continue;
            }
            let power_w = c
                .max_electric_power
                .filter(|p| *p > 0.0)
                .unwrap_or_else(|| c.max_voltage.unwrap_or(0.0) * c.max_amperage.unwrap_or(0.0));
            if power_w >= MIN_DC_POWER_W {
                best_power_w = best_power_w.max(power_w);
            }
        }
    }
    if best_power_w <= 0.0 {
        return None;
    }
    Some(Charger {
        name: loc.name.clone().unwrap_or_default(),
        lat,
        lon,
        power_kw: (best_power_w / 1000.0) as f32,
        country: loc.country_code.clone().unwrap_or_else(|| default_country.to_string()),
    })
}

fn parse_ocpi_gz(path: &str, default_country: &str) -> Vec<Charger> {
    let file = fs::File::open(path).expect("open ndw file");
    let mut s = String::new();
    GzDecoder::new(file).read_to_string(&mut s).expect("gunzip ndw");
    parse_ocpi_json_str(&s, default_country)
}

fn parse_ocpi_plain(path: &str, default_country: &str) -> Vec<Charger> {
    let s = fs::read_to_string(path).expect("read road_chargers.json");
    parse_ocpi_json_str(&s, default_country)
}

fn parse_ocpi_json_str(s: &str, default_country: &str) -> Vec<Charger> {
    let value: serde_json::Value = serde_json::from_str(s).expect("parse ocpi json");
    let locations: Vec<OcpiLocation> = if let Some(arr) = value.as_array() {
        arr.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect()
    } else if let Some(arr) = value.get("data").and_then(|d| d.as_array()) {
        arr.iter().filter_map(|v| serde_json::from_value(v.clone()).ok()).collect()
    } else {
        Vec::new()
    };
    locations
        .iter()
        .filter_map(|l| ocpi_location_to_charger(l, default_country))
        .collect()
}

/// Chargy KML (Luxembourg): only "SuperChargy" placemarks are DC (>=50kW); regular
/// Chargy (22kW AC) placemarks are dropped here rather than filtered later.
fn parse_chargy_kml(path: &str) -> Vec<Charger> {
    let s = fs::read_to_string(path).expect("read chargy.kml");
    let mut out = Vec::new();
    for chunk in s.split("<Placemark>").skip(1) {
        let end = chunk.find("</Placemark>").unwrap_or(chunk.len());
        let chunk = &chunk[..end];
        let Some(name) = extract_tag(chunk, "name") else { continue };
        if !name.contains("SuperChargy") {
            continue;
        }
        let Some(coord_str) = extract_tag(chunk, "coordinates") else { continue };
        let parts: Vec<&str> = coord_str.trim().split(',').collect();
        if parts.len() < 2 {
            continue;
        }
        let (Ok(lon), Ok(lat)) = (parts[0].parse::<f64>(), parts[1].parse::<f64>()) else { continue };
        out.push(Charger { name, lat, lon, power_kw: 160.0, country: "LU".to_string() });
    }
    out
}

fn extract_tag(chunk: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = chunk.find(&open)? + open.len();
    let end = chunk[start..].find(&close)? + start;
    Some(chunk[start..end].to_string())
}
