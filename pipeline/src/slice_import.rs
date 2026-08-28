//! Shared loader for the throwaway `prototype/vertical-slice` graph dumps
//! (`nodes.bin` / `edges_meta.bin` / `geometry.bin`), used by both the
//! `import_slice` and `bench_routing` dev binaries. Not a production code
//! path -- see `bin/import_slice.rs` for context.

use std::error::Error;
use std::fs;
use std::path::Path;

use packs::{
    EdgeHot, GeomVertex, NodeRecord, RegionGraphModel, SnapGridModel, CH_MIDDLE_NODE_NONE,
};
use serde::Deserialize;

/// `nodes.bin`: parallel lat/lon arrays, one entry per node.
#[derive(Deserialize)]
struct NodesFile {
    lat: Vec<f32>,
    lon: Vec<f32>,
}

/// One directed edge as the prototype recorded it.
#[derive(Deserialize)]
struct EdgeMeta {
    from: u32,
    to: u32,
    length_m: f32,
    speed_kmh: f32,
    geom_offset: u32,
    geom_len: u32,
}

/// `edges_meta.bin`: all edges (sorted by `(from, to)`) plus the CSR row
/// index into them, keyed by `from`.
#[derive(Deserialize)]
struct EdgesMetaFile {
    edges: Vec<EdgeMeta>,
    from_start: Vec<u32>,
}

/// Minimum speed a clamped edge is given; edges with `speed_kmh <= 0` in the
/// source data are clamped here rather than propagated into `edge_cost`
/// (which would divide by zero or go negative).
const MIN_SPEED_KMH: f32 = 5.0;

/// Snap grid cell size, matching the writer-side test helper.
pub const SNAP_CELL_SIZE_DEG: f32 = 0.1;

/// Counts and sizes surfaced from a `build_base_model` run.
pub struct ImportStats {
    pub n_nodes: usize,
    pub n_edges: usize,
    pub n_geometry_points: usize,
    pub speed_clamped: usize,
}

/// Reads a slice directory into an uncontracted `RegionGraphModel`, ready
/// for `pipeline::ch_prepare`. Every edge's `ch_middle_node` is
/// `CH_MIDDLE_NODE_NONE` and `ch_order` is all zeros (both ignored by
/// `ch_prepare` on its input).
pub fn build_base_model(
    slice_dir: &Path,
) -> Result<(RegionGraphModel, ImportStats), Box<dyn Error>> {
    let nodes_file: NodesFile = bincode::deserialize(&fs::read(slice_dir.join("nodes.bin"))?)?;
    let edges_meta: EdgesMetaFile =
        bincode::deserialize(&fs::read(slice_dir.join("edges_meta.bin"))?)?;
    let geometry_bytes = fs::read(slice_dir.join("geometry.bin"))?;

    let n_nodes = nodes_file.lat.len();
    if nodes_file.lon.len() != n_nodes {
        return Err(format!(
            "nodes.bin: lat has {n_nodes} entries but lon has {}",
            nodes_file.lon.len()
        )
        .into());
    }
    let nodes: Vec<NodeRecord> = nodes_file
        .lat
        .into_iter()
        .zip(nodes_file.lon)
        .map(|(lat, lon)| NodeRecord { lat, lon })
        .collect();

    if edges_meta.from_start.len() != n_nodes + 1 {
        return Err(format!(
            "edges_meta.bin: from_start has {} entries, expected {}",
            edges_meta.from_start.len(),
            n_nodes + 1
        )
        .into());
    }
    for node in 0..n_nodes {
        let start = edges_meta.from_start[node] as usize;
        let end = edges_meta.from_start[node + 1] as usize;
        for e in &edges_meta.edges[start..end] {
            if e.from as usize != node {
                return Err(format!(
                    "edges_meta.bin: from_start puts edge with from={} in node {node}'s range [{start}, {end})",
                    e.from
                )
                .into());
            }
        }
    }

    if !geometry_bytes.len().is_multiple_of(8) {
        return Err(format!(
            "geometry.bin: {} bytes is not a multiple of 8 (lat,lon f32 pairs)",
            geometry_bytes.len()
        )
        .into());
    }
    let n_geometry_points = geometry_bytes.len() / 8;
    let mut geometry = Vec::with_capacity(n_geometry_points);
    for chunk in geometry_bytes.as_chunks::<8>().0 {
        let lat = f32::from_le_bytes(chunk[0..4].try_into().unwrap());
        let lon = f32::from_le_bytes(chunk[4..8].try_into().unwrap());
        geometry.push(GeomVertex {
            lat,
            lon,
            elev_m: 0,
            _pad: 0,
        });
    }

    let mut speed_clamped = 0usize;
    let edges: Vec<EdgeHot> = edges_meta
        .edges
        .iter()
        .map(|e| {
            let speed_kmh = if e.speed_kmh <= 0.0 {
                speed_clamped += 1;
                MIN_SPEED_KMH
            } else {
                e.speed_kmh
            };
            EdgeHot {
                target: e.to,
                length_m: e.length_m,
                speed_kmh,
                ascent_m: 0.0,
                descent_m: 0.0,
                road_class: 0,
                _pad: [0; 3],
                ch_middle_node: CH_MIDDLE_NODE_NONE,
                geom_offset: e.geom_offset,
                geom_count: e.geom_len,
            }
        })
        .collect();
    let n_edges = edges.len();

    let snap_grid = build_snap_grid(&nodes, SNAP_CELL_SIZE_DEG);

    let model = RegionGraphModel {
        nodes,
        csr_first_edge: edges_meta.from_start,
        edges,
        ch_order: vec![0u32; n_nodes],
        geometry,
        snap_grid,
    };

    Ok((
        model,
        ImportStats {
            n_nodes,
            n_edges,
            n_geometry_points,
            speed_clamped,
        },
    ))
}

/// Builds a `cell_size_deg`-wide snap grid over `nodes`'s own bounding box,
/// replicating the grid-building approach `pipeline/tests/rpack_roundtrip.rs`
/// uses against a fixed bbox, but computed from the data instead.
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
