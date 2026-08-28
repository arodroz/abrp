//! Round-trip test for the `.rpack` writer + reader: builds a deterministic
//! synthetic Luxembourg-sized graph, writes it, reopens it with
//! `packs::Rpack`, and checks every section round-trips, checksums verify,
//! CSR/geometry lookups agree with the model, nearest-node snapping matches
//! a brute-force scan, and malformed files are rejected without panicking.

use std::io::{Read, Seek, SeekFrom, Write};

use packs::{
    EdgeHot, GeomVertex, HeaderFixed, NodeRecord, RegionGraphModel, Rpack, RpackError,
    SectionEntry, SnapGridHeader, SnapGridModel, CH_MIDDLE_NODE_NONE, SECTION_SNAP_GRID,
};
use pipeline::{write_rpack, PackMeta};

const MIN_LAT: f32 = 49.4;
const MAX_LAT: f32 = 50.2;
const MIN_LON: f32 = 5.7;
const MAX_LON: f32 = 6.5;
const CELL_SIZE_DEG: f32 = 0.1;

const N_NODES: usize = 12_000;
const N_EDGES: usize = 45_000;

/// A minimal LCG (numerical-recipes constants) so the test graph is
/// deterministic without pulling in a `rand` dependency.
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

    /// Uniform in `[0, 1)`.
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn next_range_u32(&mut self, lo: u32, hi: u32) -> u32 {
        lo + (self.next_f64() * (hi - lo) as f64) as u32
    }

    fn next_range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next_f64() as f32 * (hi - lo)
    }
}

fn snap_grid_dims() -> (u32, u32) {
    let n_rows = ((MAX_LAT - MIN_LAT) / CELL_SIZE_DEG).ceil() as u32;
    let n_cols = ((MAX_LON - MIN_LON) / CELL_SIZE_DEG).ceil() as u32;
    (n_rows, n_cols)
}

fn grid_cell(lat: f32, lon: f32, n_rows: u32, n_cols: u32) -> (u32, u32) {
    let row = (((lat - MIN_LAT) / CELL_SIZE_DEG).floor() as i64).clamp(0, n_rows as i64 - 1) as u32;
    let col = (((lon - MIN_LON) / CELL_SIZE_DEG).floor() as i64).clamp(0, n_cols as i64 - 1) as u32;
    (row, col)
}

fn build_snap_grid(nodes: &[NodeRecord]) -> SnapGridModel {
    let (n_rows, n_cols) = snap_grid_dims();
    let n_cells = (n_rows * n_cols) as usize;
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); n_cells];
    for (i, n) in nodes.iter().enumerate() {
        let (row, col) = grid_cell(n.lat, n.lon, n_rows, n_cols);
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
        min_lat: MIN_LAT,
        min_lon: MIN_LON,
        cell_size_deg: CELL_SIZE_DEG,
        n_rows,
        n_cols,
        cell_offsets,
        node_ids,
    }
}

/// Builds a deterministic synthetic graph over a Luxembourg-sized bbox:
/// `N_NODES` scattered nodes, `N_EDGES` directed edges (some with CH
/// shortcut middle nodes) each carrying its own polyline into the geometry
/// blob, and a 0.1deg snap grid.
fn build_model(seed: u64) -> RegionGraphModel {
    let mut rng = Lcg::new(seed);

    let nodes: Vec<NodeRecord> = (0..N_NODES)
        .map(|_| NodeRecord {
            lat: rng.next_range_f32(MIN_LAT, MAX_LAT),
            lon: rng.next_range_f32(MIN_LON, MAX_LON),
        })
        .collect();

    // Generate (from, edge) pairs, then sort by `from` so edges land
    // contiguously per node -- exactly what the CSR row index requires.
    let mut geometry: Vec<GeomVertex> = Vec::new();
    let mut pairs: Vec<(u32, EdgeHot)> = Vec::with_capacity(N_EDGES);
    for _ in 0..N_EDGES {
        let from = rng.next_range_u32(0, N_NODES as u32);
        let mut to = rng.next_range_u32(0, N_NODES as u32);
        while to == from {
            to = rng.next_range_u32(0, N_NODES as u32);
        }

        let a = nodes[from as usize];
        let b = nodes[to as usize];
        let geom_count = rng.next_range_u32(2, 5);
        let geom_offset = geometry.len() as u32;
        for i in 0..geom_count {
            let t = i as f32 / (geom_count - 1) as f32;
            geometry.push(GeomVertex {
                lat: a.lat + (b.lat - a.lat) * t,
                lon: a.lon + (b.lon - a.lon) * t,
                elev_m: rng.next_range_u32(200, 800) as i16,
                _pad: 0,
            });
        }

        // ~10% of edges are CH shortcuts over some other node.
        let ch_middle_node = if rng.next_f64() < 0.1 {
            rng.next_range_u32(0, N_NODES as u32)
        } else {
            CH_MIDDLE_NODE_NONE
        };

        pairs.push((
            from,
            EdgeHot {
                target: to,
                length_m: rng.next_range_f32(10.0, 3000.0),
                speed_kmh: rng.next_range_f32(30.0, 130.0),
                ascent_m: rng.next_range_f32(0.0, 30.0),
                descent_m: rng.next_range_f32(0.0, 30.0),
                road_class: rng.next_range_u32(0, 7) as u8,
                _pad: [0; 3],
                ch_middle_node,
                geom_offset,
                geom_count,
            },
        ));
    }
    pairs.sort_by_key(|(from, _)| *from);

    let mut counts = vec![0u32; N_NODES];
    for (from, _) in &pairs {
        counts[*from as usize] += 1;
    }
    let mut csr_first_edge = vec![0u32; N_NODES + 1];
    for i in 0..N_NODES {
        csr_first_edge[i + 1] = csr_first_edge[i] + counts[i];
    }

    let edges: Vec<EdgeHot> = pairs.into_iter().map(|(_, e)| e).collect();
    let ch_order: Vec<u32> = (0..N_NODES)
        .map(|_| rng.next_range_u32(0, N_NODES as u32))
        .collect();
    let snap_grid = build_snap_grid(&nodes);

    RegionGraphModel {
        nodes,
        csr_first_edge,
        edges,
        ch_order,
        geometry,
        snap_grid,
    }
}

fn build_meta() -> PackMeta {
    PackMeta {
        osm_snapshot_epoch: 1_756_252_800,
        region_id: 42,
        region_name: "lu-dev".to_string(),
    }
}

/// Same equirectangular metric `Rpack::snap` uses internally, so a
/// brute-force scan over every node is a fair reference for "true nearest".
fn brute_force_nearest(nodes: &[NodeRecord], lat: f32, lon: f32) -> u32 {
    let cos_lat = (lat as f64).to_radians().cos();
    let mut best = (0u32, f64::MAX);
    for (i, n) in nodes.iter().enumerate() {
        let dlat = (n.lat - lat) as f64;
        let dlon = (n.lon - lon) as f64 * cos_lat;
        let d2 = dlat * dlat + dlon * dlon;
        if d2 < best.1 {
            best = (i as u32, d2);
        }
    }
    best.0
}

#[test]
fn round_trips_and_verifies() {
    let model = build_model(0xC0FFEE);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lu-dev.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    let pack = Rpack::open(&path).unwrap();

    assert_eq!(pack.format_version(), (1, 0));
    assert_eq!(pack.osm_snapshot_epoch(), meta.osm_snapshot_epoch);
    assert_eq!(pack.region_id(), meta.region_id);
    assert_eq!(pack.region_name(), meta.region_name);

    // Every section array round-trips element-for-element.
    assert_eq!(pack.nodes(), model.nodes.as_slice());
    assert_eq!(pack.csr_first_edge(), model.csr_first_edge.as_slice());
    assert_eq!(pack.edges(), model.edges.as_slice());
    assert_eq!(pack.ch_order(), model.ch_order.as_slice());
    assert_eq!(pack.geometry(), model.geometry.as_slice());

    pack.verify_checksums()
        .expect("checksums should verify on an untouched file");

    // CSR edge-range and geometry getters agree with the model.
    for &node_id in &[0u32, 1, 100, 5000, (N_NODES - 1) as u32] {
        let range = pack.edge_range(node_id).unwrap();
        let expected_range = model.csr_first_edge[node_id as usize] as usize
            ..model.csr_first_edge[node_id as usize + 1] as usize;
        assert_eq!(range, expected_range);
        assert_eq!(
            pack.edges_for(node_id).unwrap(),
            &model.edges[expected_range]
        );
    }
    for edge in pack.edges().iter().step_by(500) {
        let expected = &model.geometry
            [edge.geom_offset as usize..edge.geom_offset as usize + edge.geom_count as usize];
        assert_eq!(pack.geometry_for_edge(edge), expected);
    }

    // snap() on sampled coordinates matches a brute-force scan.
    let mut rng = Lcg::new(0xBEEF);
    for _ in 0..20 {
        let lat = rng.next_range_f32(MIN_LAT, MAX_LAT);
        let lon = rng.next_range_f32(MIN_LON, MAX_LON);
        let expected = brute_force_nearest(&model.nodes, lat, lon);
        let got = pack
            .snap(lat, lon)
            .expect("query inside coverage should always snap");
        assert_eq!(got, expected, "snap({lat}, {lon}) mismatch");
    }
}

#[test]
fn corrupting_a_section_byte_fails_checksum_verification() {
    let model = build_model(1);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("corrupt.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    // Flip a byte well past the header + section table, inside the NODES payload.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    let flip_offset = 300u64;
    file.seek(SeekFrom::Start(flip_offset)).unwrap();
    let mut byte = [0u8; 1];
    file.read_exact(&mut byte).unwrap();
    byte[0] ^= 0xFF;
    file.seek(SeekFrom::Start(flip_offset)).unwrap();
    file.write_all(&byte).unwrap();
    drop(file);

    let pack = Rpack::open(&path).unwrap();
    assert!(pack.verify_checksums().is_err());
}

#[test]
fn refuses_unsupported_format_major() {
    let model = build_model(2);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    // format_major is a little-endian u16 at byte offset 4.
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
    file.seek(SeekFrom::Start(4)).unwrap();
    file.write_all(&2u16.to_le_bytes()).unwrap();
    drop(file);

    match Rpack::open(&path) {
        Err(RpackError::UnsupportedVersion { major: 2 }) => {}
        Err(other) => panic!("expected UnsupportedVersion {{ major: 2 }}, got {other}"),
        Ok(_) => panic!("expected UnsupportedVersion {{ major: 2 }}, got Ok"),
    }
}

#[test]
fn refuses_truncated_file_without_panicking() {
    let model = build_model(3);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("truncated.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    let full_len = std::fs::metadata(&path).unwrap().len();
    let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.set_len(full_len / 2).unwrap();
    drop(file);

    assert!(Rpack::open(&path).is_err());
}

/// Finds the absolute file offset of `section_id`'s payload by parsing the
/// header + section table directly, so tests can byte-patch a specific
/// section without hard-coding the writer's internal layout.
fn find_section_offset(bytes: &[u8], section_id: u32) -> usize {
    let header_size = std::mem::size_of::<HeaderFixed>();
    let entry_size = std::mem::size_of::<SectionEntry>();
    let header: HeaderFixed = bytemuck::pod_read_unaligned(&bytes[0..header_size]);
    for i in 0..header.section_count as usize {
        let start = header_size + i * entry_size;
        let entry: SectionEntry = bytemuck::pod_read_unaligned(&bytes[start..start + entry_size]);
        if entry.section_id == section_id {
            return entry.offset as usize;
        }
    }
    panic!("section {section_id} not found in section table");
}

#[test]
fn refuses_snap_grid_with_oversized_dimensions() {
    let model = build_model(4);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("oversized_grid.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    let snap_grid_offset = find_section_offset(&bytes, SECTION_SNAP_GRID);
    // n_rows follows min_lat/min_lon/cell_size_deg (three f32s) in SnapGridHeader.
    let n_rows_offset = snap_grid_offset + 3 * std::mem::size_of::<f32>();
    bytes[n_rows_offset..n_rows_offset + 4].copy_from_slice(&1_000_000u32.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    assert!(Rpack::open(&path).is_err());
}

#[test]
fn refuses_snap_grid_with_non_monotone_cell_offsets() {
    let model = build_model(5);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("non_monotone_grid.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    let snap_grid_offset = find_section_offset(&bytes, SECTION_SNAP_GRID);
    let header_size = std::mem::size_of::<SnapGridHeader>();
    // cell_offsets[1]: any later cumulative count is far smaller than u32::MAX,
    // so this forces a monotonicity violation further into the array.
    let cell_offset_1 = snap_grid_offset + header_size + std::mem::size_of::<u32>();
    bytes[cell_offset_1..cell_offset_1 + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    assert!(Rpack::open(&path).is_err());
}
