//! Round-trip test for the `.rpack` writer + reader: builds a deterministic
//! synthetic Luxembourg-sized graph, writes it, reopens it with
//! `packs::Rpack`, and checks every section round-trips, checksums verify,
//! CSR/geometry lookups agree with the model, nearest-node snapping matches
//! a brute-force scan, and malformed files are rejected without panicking.

use std::io::{Read, Seek, SeekFrom, Write};

use packs::{
    EdgeHot, GeomVertex, HeaderFixed, NodeRecord, RegionGraphModel, Rpack, RpackError,
    SectionEntry, SnapGridHeader, SnapGridModel, CH_MIDDLE_NODE_NONE, SECTION_CH_ORDER,
    SECTION_CSR, SECTION_EDGES_HOT, SECTION_NODES, SECTION_REVERSE_EDGES, SECTION_SNAP_GRID,
};
use pipeline::{write_rpack, PackMeta};

mod common;
use common::Lcg;

const MIN_LAT: f32 = 49.4;
const MAX_LAT: f32 = 50.2;
const MIN_LON: f32 = 5.7;
const MAX_LON: f32 = 6.5;
const CELL_SIZE_DEG: f32 = 0.1;

const N_NODES: usize = 12_000;
const N_EDGES: usize = 45_000;

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

    assert_eq!(pack.format_version(), (1, 1));
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

    // Baked reverse adjacency (format 1.1): every edge appears exactly once,
    // grouped under its target node.
    let reverse_csr = pack.reverse_csr();
    assert_eq!(reverse_csr.len(), N_NODES + 1);
    let mut seen = vec![false; model.edges.len()];
    for node_id in 0..N_NODES as u32 {
        for &edge_id in pack.reverse_edge_ids_for(node_id).unwrap() {
            assert!(
                !seen[edge_id as usize],
                "edge {edge_id} appears in more than one reverse bucket"
            );
            seen[edge_id as usize] = true;
            assert_eq!(
                pack.edges()[edge_id as usize].target,
                node_id,
                "edge {edge_id} grouped under node {node_id} but targets a different node"
            );
        }
    }
    assert!(
        seen.iter().all(|&s| s),
        "every edge should appear in the reverse index"
    );

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

    // Flip a byte inside the NODES payload (offset computed from the section
    // table rather than hard-coded, since the table's size shifts as
    // sections are added).
    let bytes = std::fs::read(&path).unwrap();
    let flip_offset = (find_section_offset(&bytes, SECTION_NODES) + 4) as u64;
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .unwrap();
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

/// Finds the absolute file offset of `section_id`'s table *entry* (as
/// opposed to `find_section_offset`, which returns its payload offset), so
/// tests can byte-patch `section_id`/`offset`/`len_bytes` directly.
fn find_table_entry_offset(bytes: &[u8], section_id: u32) -> usize {
    let header_size = std::mem::size_of::<HeaderFixed>();
    let entry_size = std::mem::size_of::<SectionEntry>();
    let header: HeaderFixed = bytemuck::pod_read_unaligned(&bytes[0..header_size]);
    for i in 0..header.section_count as usize {
        let start = header_size + i * entry_size;
        let entry: SectionEntry = bytemuck::pod_read_unaligned(&bytes[start..start + entry_size]);
        if entry.section_id == section_id {
            return start;
        }
    }
    panic!("section {section_id} not found in section table");
}

// --- M-04: cheap structural checks at `open` ---------------------------

#[test]
fn refuses_duplicate_section_ids() {
    let model = build_model(6);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("duplicate_section.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    // section_id is the entry's first field (u32 at offset 0).
    let ch_order_entry = find_table_entry_offset(&bytes, SECTION_CH_ORDER);
    bytes[ch_order_entry..ch_order_entry + 4].copy_from_slice(&SECTION_NODES.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    match Rpack::open(&path) {
        Err(RpackError::DuplicateSection { section_id }) => assert_eq!(section_id, SECTION_NODES),
        Err(other) => panic!("expected DuplicateSection, got {other}"),
        Ok(_) => panic!("expected DuplicateSection, got Ok"),
    }
}

#[test]
fn refuses_sections_overlapping_each_other() {
    let model = build_model(7);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("overlapping_sections.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    let nodes_entry = find_table_entry_offset(&bytes, SECTION_NODES);
    // offset is the entry's third field (u64 at byte 8, after section_id
    // and its padding).
    let nodes_offset =
        u64::from_le_bytes(bytes[nodes_entry + 8..nodes_entry + 16].try_into().unwrap());

    let geometry_entry = find_table_entry_offset(&bytes, packs::SECTION_GEOMETRY);
    // Moves GEOMETRY's offset to land inside NODES' range -- still 8-byte
    // aligned (nodes_offset already is), still within the file (moved
    // earlier, so its end can only shrink), but now overlapping.
    let new_offset = nodes_offset + 8;
    bytes[geometry_entry + 8..geometry_entry + 16].copy_from_slice(&new_offset.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    match Rpack::open(&path) {
        Err(RpackError::OverlappingSections { .. }) => {}
        Err(other) => panic!("expected OverlappingSections, got {other}"),
        Ok(_) => panic!("expected OverlappingSections, got Ok"),
    }
}

#[test]
fn refuses_section_overlapping_the_header_or_table() {
    let model = build_model(8);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("section_overlaps_header.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    let snap_grid_entry = find_table_entry_offset(&bytes, SECTION_SNAP_GRID);
    // Rewrites SNAP_GRID's [offset, len_bytes) to [0, 8) -- entirely inside
    // the fixed header (56 bytes), so it doesn't collide with any other
    // section, only the header/table region itself.
    bytes[snap_grid_entry + 8..snap_grid_entry + 16].copy_from_slice(&0u64.to_le_bytes());
    bytes[snap_grid_entry + 16..snap_grid_entry + 24].copy_from_slice(&8u64.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    match Rpack::open(&path) {
        Err(RpackError::SectionOverlapsHeader { .. }) => {}
        Err(other) => panic!("expected SectionOverlapsHeader, got {other}"),
        Ok(_) => panic!("expected SectionOverlapsHeader, got Ok"),
    }
}

// --- M-04: verify_structure (opt-in O(n) index validation) --------------

/// `EdgeHot`'s field byte offsets, mirroring `core/packs/src/format.rs`'s
/// `#[repr(C)]` layout (36 bytes total): target(4)@0, length_m(4)@4,
/// speed_kmh(4)@8, ascent_m(4)@12, descent_m(4)@16, road_class(1)+pad(3)@20,
/// ch_middle_node(4)@24, geom_offset(4)@28, geom_count(4)@32.
const EDGE_TARGET_OFFSET: usize = 0;
const EDGE_GEOM_COUNT_OFFSET: usize = 32;

#[test]
fn verify_structure_accepts_an_untouched_pack() {
    let model = build_model(9);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("valid.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    let pack = Rpack::open(&path).unwrap();
    pack.verify_structure()
        .expect("a pack built from a validated model should pass verify_structure");
}

#[test]
fn verify_structure_rejects_non_monotone_csr() {
    let model = build_model(10);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("non_monotone_csr.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    let csr_offset = find_section_offset(&bytes, SECTION_CSR);
    // csr_first_edge[1]: forced far above any later cumulative count.
    let idx1 = csr_offset + std::mem::size_of::<u32>();
    bytes[idx1..idx1 + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    let pack = Rpack::open(&path).expect("this invariant is O(n), not checked by open");
    assert!(pack.verify_structure().is_err());
}

#[test]
fn verify_structure_rejects_csr_not_ending_at_edge_count() {
    let model = build_model(11);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("csr_wrong_last_entry.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    let csr_offset = find_section_offset(&bytes, SECTION_CSR);
    let last_idx = csr_offset + N_NODES * std::mem::size_of::<u32>();
    bytes[last_idx..last_idx + 4].copy_from_slice(&0u32.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    let pack = Rpack::open(&path).expect("this invariant is O(n), not checked by open");
    assert!(pack.verify_structure().is_err());
}

#[test]
fn verify_structure_rejects_out_of_range_edge_target() {
    let model = build_model(12);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad_edge_target.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    let edges_offset = find_section_offset(&bytes, SECTION_EDGES_HOT);
    let target_idx = edges_offset + EDGE_TARGET_OFFSET;
    bytes[target_idx..target_idx + 4].copy_from_slice(&(N_NODES as u32).to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    let pack = Rpack::open(&path).expect("this invariant is O(n), not checked by open");
    assert!(pack.verify_structure().is_err());
}

#[test]
fn verify_structure_rejects_out_of_range_reverse_edge_index() {
    let model = build_model(13);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad_reverse_edge.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    let reverse_edges_offset = find_section_offset(&bytes, SECTION_REVERSE_EDGES);
    bytes[reverse_edges_offset..reverse_edges_offset + 4]
        .copy_from_slice(&(N_EDGES as u32).to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    let pack = Rpack::open(&path).expect("this invariant is O(n), not checked by open");
    assert!(pack.verify_structure().is_err());
}

#[test]
fn verify_structure_rejects_out_of_range_ch_order() {
    let model = build_model(14);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad_ch_order.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    let ch_order_offset = find_section_offset(&bytes, SECTION_CH_ORDER);
    bytes[ch_order_offset..ch_order_offset + 4].copy_from_slice(&(N_NODES as u32).to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    let pack = Rpack::open(&path).expect("this invariant is O(n), not checked by open");
    assert!(pack.verify_structure().is_err());
}

#[test]
fn verify_structure_rejects_geometry_range_out_of_bounds() {
    let model = build_model(15);
    let meta = build_meta();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad_geometry_range.rpack");
    write_rpack(&model, &meta, &path).unwrap();

    let mut bytes = std::fs::read(&path).unwrap();
    let edges_offset = find_section_offset(&bytes, SECTION_EDGES_HOT);
    let geom_count_idx = edges_offset + EDGE_GEOM_COUNT_OFFSET;
    bytes[geom_count_idx..geom_count_idx + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    std::fs::write(&path, &bytes).unwrap();

    let pack = Rpack::open(&path).expect("this invariant is O(n), not checked by open");
    assert!(pack.verify_structure().is_err());
}
