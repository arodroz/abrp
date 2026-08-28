//! `.rpack` Region Pack binary format: header/section-table layout, the
//! per-section Pod element types, and the in-memory graph model that both the
//! pipeline writer and this crate's tests build against.
//!
//! See docs/adr/0007-region-pack-format.md for the design rationale.

use bytemuck::{Pod, Zeroable};

pub const MAGIC: [u8; 4] = *b"RPCK";
pub const FORMAT_MAJOR: u16 = 1;
pub const FORMAT_MINOR: u16 = 1;

/// Fixed-width region name stored NUL-padded in the header.
pub const REGION_NAME_LEN: usize = 32;

pub const ALIGN: u64 = 8;

pub const SECTION_NODES: u32 = 1;
pub const SECTION_CSR: u32 = 2;
pub const SECTION_EDGES_HOT: u32 = 3;
pub const SECTION_CH_ORDER: u32 = 4;
pub const SECTION_GEOMETRY: u32 = 5;
pub const SECTION_SNAP_GRID: u32 = 6;
/// Format 1.1: baked reverse-adjacency CSR row index, length `n_nodes + 1`.
pub const SECTION_REVERSE_CSR: u32 = 7;
/// Format 1.1: edge indices into `EDGES_HOT`, grouped by target node per
/// `SECTION_REVERSE_CSR`. Length `n_edges`.
pub const SECTION_REVERSE_EDGES: u32 = 8;

/// Rounds `n` up to the next multiple of `ALIGN` (8 bytes).
pub const fn align_up(n: u64) -> u64 {
    (n + (ALIGN - 1)) & !(ALIGN - 1)
}

/// Fixed-size prefix of the header (56 bytes), immediately followed by the
/// section table (`section_count` entries of `SectionEntry`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct HeaderFixed {
    pub magic: [u8; 4],
    pub format_major: u16,
    pub format_minor: u16,
    pub osm_snapshot_epoch: u64,
    pub region_id: u32,
    pub region_name: [u8; REGION_NAME_LEN],
    pub section_count: u32,
}

/// One section-table entry. `offset` is absolute from the start of the file
/// and 8-byte aligned; `len_bytes` is the exact section payload length
/// (before any trailing alignment padding).
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct SectionEntry {
    pub section_id: u32,
    pub _pad: u32,
    pub offset: u64,
    pub len_bytes: u64,
    pub crc32: u32,
    pub _pad2: u32,
}

/// NODES section element: a junction's coordinates.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct NodeRecord {
    pub lat: f32,
    pub lon: f32,
}

/// EDGES_HOT section element: everything the CH search touches for one
/// directed edge. Ascent and descent are both stored (energy is asymmetric
/// in climb vs. descent, per ADR 0007). `ch_middle_node == u32::MAX` marks an
/// original (non-shortcut) edge; otherwise it's the CH-contracted middle
/// node. `geom_offset`/`geom_count` index into the GEOMETRY section.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct EdgeHot {
    pub target: u32,
    pub length_m: f32,
    pub speed_kmh: f32,
    pub ascent_m: f32,
    pub descent_m: f32,
    pub road_class: u8,
    pub _pad: [u8; 3],
    pub ch_middle_node: u32,
    pub geom_offset: u32,
    pub geom_count: u32,
}

pub const CH_MIDDLE_NODE_NONE: u32 = u32::MAX;

/// GEOMETRY section element: one cold polyline vertex.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct GeomVertex {
    pub lat: f32,
    pub lon: f32,
    pub elev_m: i16,
    pub _pad: i16,
}

/// Grid header stored at the start of the SNAP_GRID section, followed by a
/// CSR-style `cell_offsets: [u32; n_rows * n_cols + 1]` and then
/// `node_ids: [u32]` grouped row-major by cell.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct SnapGridHeader {
    pub min_lat: f32,
    pub min_lon: f32,
    pub cell_size_deg: f32,
    pub n_rows: u32,
    pub n_cols: u32,
    pub _pad: u32,
}

/// In-memory mirror of the pack's sections, built by pipeline consumers and
/// consumed by `pipeline::write_rpack`. Plain vecs so callers can build them
/// however they like before validating and serializing.
#[derive(Clone, Debug, Default)]
pub struct RegionGraphModel {
    pub nodes: Vec<NodeRecord>,
    /// CSR row index into `edges`, length `nodes.len() + 1`.
    pub csr_first_edge: Vec<u32>,
    pub edges: Vec<EdgeHot>,
    /// CH rank/level per node, length `nodes.len()`.
    pub ch_order: Vec<u32>,
    pub geometry: Vec<GeomVertex>,
    pub snap_grid: SnapGridModel,
}

#[derive(Clone, Debug, Default)]
pub struct SnapGridModel {
    pub min_lat: f32,
    pub min_lon: f32,
    pub cell_size_deg: f32,
    pub n_rows: u32,
    pub n_cols: u32,
    pub cell_offsets: Vec<u32>,
    pub node_ids: Vec<u32>,
}

impl RegionGraphModel {
    /// Checks the invariants the reader relies on: CSR is monotone and sized
    /// `n+1`, edge targets and CH middle nodes are in range, geometry offsets
    /// stay inside the geometry blob, and the snap grid only references real
    /// node ids with a CSR of the right shape.
    pub fn validate(&self) -> Result<(), crate::error::RpackError> {
        use crate::error::RpackError;

        let n_nodes = self.nodes.len();

        if self.csr_first_edge.len() != n_nodes + 1 {
            return Err(RpackError::Validation(format!(
                "csr_first_edge has {} entries, expected {}",
                self.csr_first_edge.len(),
                n_nodes + 1
            )));
        }
        if self.ch_order.len() != n_nodes {
            return Err(RpackError::Validation(format!(
                "ch_order has {} entries, expected {n_nodes}",
                self.ch_order.len()
            )));
        }
        for w in self.csr_first_edge.windows(2) {
            if w[1] < w[0] {
                return Err(RpackError::Validation(
                    "csr_first_edge is not monotone".into(),
                ));
            }
        }
        let n_edges = self.edges.len() as u64;
        if let Some(&last) = self.csr_first_edge.last() {
            if last as u64 != n_edges {
                return Err(RpackError::Validation(format!(
                    "csr_first_edge's last entry {last} does not match edge count {n_edges}"
                )));
            }
        }

        let n_geom = self.geometry.len() as u64;
        for (i, e) in self.edges.iter().enumerate() {
            if e.target as usize >= n_nodes {
                return Err(RpackError::Validation(format!(
                    "edge {i} targets out-of-range node {}",
                    e.target
                )));
            }
            if e.ch_middle_node != CH_MIDDLE_NODE_NONE && e.ch_middle_node as usize >= n_nodes {
                return Err(RpackError::Validation(format!(
                    "edge {i} has out-of-range ch_middle_node {}",
                    e.ch_middle_node
                )));
            }
            let geom_end = e.geom_offset as u64 + e.geom_count as u64;
            if geom_end > n_geom {
                return Err(RpackError::Validation(format!(
                    "edge {i} geometry range [{}, {geom_end}) exceeds geometry blob of {n_geom}",
                    e.geom_offset
                )));
            }
        }

        let grid = &self.snap_grid;
        let n_cells = grid.n_rows as u64 * grid.n_cols as u64;
        if grid.cell_offsets.len() as u64 != n_cells + 1 {
            return Err(RpackError::Validation(format!(
                "snap grid cell_offsets has {} entries, expected {}",
                grid.cell_offsets.len(),
                n_cells + 1
            )));
        }
        for w in grid.cell_offsets.windows(2) {
            if w[1] < w[0] {
                return Err(RpackError::Validation(
                    "snap grid cell_offsets is not monotone".into(),
                ));
            }
        }
        if let Some(&last) = grid.cell_offsets.last() {
            if last as usize != grid.node_ids.len() {
                return Err(RpackError::Validation(
                    "snap grid cell_offsets last entry does not match node_ids length".into(),
                ));
            }
        }
        for &node_id in &grid.node_ids {
            if node_id as usize >= n_nodes {
                return Err(RpackError::Validation(format!(
                    "snap grid references out-of-range node {node_id}"
                )));
            }
        }

        Ok(())
    }
}
