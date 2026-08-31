//! `.rpack` Region Pack binary format: header/section-table layout, the
//! per-section Pod element types, and the in-memory graph model that both the
//! pipeline writer and this crate's tests build against.
//!
//! See docs/adr/0007-region-pack-format.md for the design rationale.

use bytemuck::{Pod, Zeroable};

pub const MAGIC: [u8; 4] = *b"RPCK";
pub const FORMAT_MAJOR: u16 = 2;
pub const FORMAT_MINOR: u16 = 0;

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

/// Format 2.0 (wayfinder #65): guidance string table — u32 offsets, length n_strings + 1.
pub const SECTION_STRING_OFFSETS: u32 = 9;
/// Format 2.0: guidance string table — one UTF-8 blob; string i is blob[offsets[i]..offsets[i+1]]. String id 0 is always the empty string.
pub const SECTION_STRING_BLOB: u32 = 10;
/// Format 2.0: unique (name, ref) pairs; entry 0 is always {0, 0} (unnamed).
pub const SECTION_EDGE_ATTRS: u32 = 11;
/// Format 2.0: one u32 per EDGES_HOT slot indexing EDGE_ATTRS; GUIDE_NONE for shortcut edges.
pub const SECTION_EDGE_GUIDE: u32 = 12;
/// Format 2.0: sparse destination signage, sorted by edge_slot (unique).
pub const SECTION_DEST_SIGNS: u32 = 13;
/// Format 2.0: sparse motorway_junction exit refs, sorted by node_id (unique).
pub const SECTION_EXIT_REFS: u32 = 14;

pub const GUIDE_NONE: u32 = u32::MAX;

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
/// `guide_flags` is format 2.0 guidance metadata (wayfinder #65): bits 0-3
/// are the highway class (`GUIDE_CLASS_*`), bit 4 is the `_link` flag, bit 5
/// is the roundabout flag; `0` for shortcuts and pre-2.0 data. This is
/// independent of `road_class`, which the energy crate still owns.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct EdgeHot {
    pub target: u32,
    pub length_m: f32,
    pub speed_kmh: f32,
    pub ascent_m: f32,
    pub descent_m: f32,
    pub road_class: u8,
    pub guide_flags: u8,
    pub _pad: [u8; 2],
    pub ch_middle_node: u32,
    pub geom_offset: u32,
    pub geom_count: u32,
}

pub const CH_MIDDLE_NODE_NONE: u32 = u32::MAX;

pub const GUIDE_CLASS_NONE: u8 = 0;
pub const GUIDE_CLASS_MOTORWAY: u8 = 1;
pub const GUIDE_CLASS_TRUNK: u8 = 2;
pub const GUIDE_CLASS_PRIMARY: u8 = 3;
pub const GUIDE_CLASS_SECONDARY: u8 = 4;
pub const GUIDE_CLASS_TERTIARY: u8 = 5;
pub const GUIDE_CLASS_UNCLASSIFIED: u8 = 6;
pub const GUIDE_CLASS_RESIDENTIAL: u8 = 7;
pub const GUIDE_CLASS_LIVING_STREET: u8 = 8;
pub const GUIDE_FLAG_LINK: u8 = 0x10;
pub const GUIDE_FLAG_ROUNDABOUT: u8 = 0x20;

impl EdgeHot {
    pub fn guide_class(&self) -> u8 {
        self.guide_flags & 0x0F
    }
    pub fn guide_is_link(&self) -> bool {
        self.guide_flags & GUIDE_FLAG_LINK != 0
    }
    pub fn guide_is_roundabout(&self) -> bool {
        self.guide_flags & GUIDE_FLAG_ROUNDABOUT != 0
    }
}

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

/// EDGE_ATTRS section element (wayfinder #65): a unique (name, ref) pair,
/// interned once and shared by every edge that carries it. Entry 0 is
/// always `{0, 0}` (unnamed): `SECTION_EDGE_GUIDE` points here for edges
/// with no name or ref.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct EdgeAttr {
    pub name_id: u32,
    pub ref_id: u32,
}

/// DEST_SIGNS section element (wayfinder #65): destination signage attached
/// to one original edge (`edge_slot` indexes `EDGES_HOT`), describing the
/// way's forward direction only (`destination:backward` is deferred). String
/// ids of `0` mean "not present".
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct DestSign {
    pub edge_slot: u32,
    pub dest_id: u32,
    pub dest_ref_id: u32,
    pub junction_ref_id: u32,
}

/// EXIT_REFS section element (wayfinder #65): a `highway=motorway_junction`
/// node's `ref` tag.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct ExitRef {
    pub node_id: u32,
    pub ref_id: u32,
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
    /// Format 2.0 guidance (wayfinder #65). `string_offsets`/`string_blob` hold the interned
    /// string table (id 0 = empty string). `edge_guide` is parallel to `edges` (GUIDE_NONE for
    /// shortcuts) — or empty, meaning "no guidance", which the writer expands to a minimal
    /// valid guidance (originals pointing at unnamed attr 0, shortcuts GUIDE_NONE).
    pub string_offsets: Vec<u32>, // length n_strings + 1 when non-empty; [0, 0] minimum
    pub string_blob: Vec<u8>,
    pub edge_attrs: Vec<EdgeAttr>,
    pub edge_guide: Vec<u32>,
    pub dest_signs: Vec<DestSign>,
    pub exit_refs: Vec<ExitRef>,
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

        // Format 2.0 guidance (wayfinder #65): an entirely empty model means
        // "no guidance" (the writer synthesizes a minimal valid one), so it's
        // exempt from every check below.
        let guidance_empty = self.string_offsets.is_empty()
            && self.string_blob.is_empty()
            && self.edge_attrs.is_empty()
            && self.edge_guide.is_empty()
            && self.dest_signs.is_empty()
            && self.exit_refs.is_empty();
        if !guidance_empty {
            if self.string_offsets.len() < 2 {
                return Err(RpackError::Validation(format!(
                    "string_offsets has {} entries, expected at least 2",
                    self.string_offsets.len()
                )));
            }
            if self.string_offsets[0] != 0 {
                return Err(RpackError::Validation("string_offsets[0] must be 0".into()));
            }
            for w in self.string_offsets.windows(2) {
                if w[1] < w[0] {
                    return Err(RpackError::Validation(
                        "string_offsets is not monotone nondecreasing".into(),
                    ));
                }
            }
            let blob_len = self.string_blob.len() as u32;
            if *self.string_offsets.last().unwrap() != blob_len {
                return Err(RpackError::Validation(format!(
                    "string_offsets' last entry does not match string_blob length {blob_len}"
                )));
            }
            if self.string_offsets[1] != 0 {
                return Err(RpackError::Validation(
                    "string id 0 must be the empty string".into(),
                ));
            }
            let n_strings = self.string_offsets.len() as u32 - 1;

            let Some(&attr0) = self.edge_attrs.first() else {
                return Err(RpackError::Validation("edge_attrs is empty".into()));
            };
            if attr0
                != (EdgeAttr {
                    name_id: 0,
                    ref_id: 0,
                })
            {
                return Err(RpackError::Validation(
                    "edge_attrs[0] must be {name_id: 0, ref_id: 0}".into(),
                ));
            }
            for (i, attr) in self.edge_attrs.iter().enumerate() {
                if attr.name_id >= n_strings || attr.ref_id >= n_strings {
                    return Err(RpackError::Validation(format!(
                        "edge_attrs[{i}] references out-of-range string id"
                    )));
                }
            }

            if self.edge_guide.len() != self.edges.len() {
                return Err(RpackError::Validation(format!(
                    "edge_guide has {} entries, expected {}",
                    self.edge_guide.len(),
                    self.edges.len()
                )));
            }
            let n_attrs = self.edge_attrs.len() as u32;
            for (i, (&guide, edge)) in self.edge_guide.iter().zip(&self.edges).enumerate() {
                let is_shortcut = edge.ch_middle_node != CH_MIDDLE_NODE_NONE;
                if is_shortcut {
                    if guide != GUIDE_NONE {
                        return Err(RpackError::Validation(format!(
                            "edge_guide[{i}] is {guide}, expected GUIDE_NONE for a shortcut edge"
                        )));
                    }
                } else if guide >= n_attrs {
                    return Err(RpackError::Validation(format!(
                        "edge_guide[{i}] references out-of-range attr {guide}"
                    )));
                }
            }

            let mut prev_slot: Option<u32> = None;
            for (i, sign) in self.dest_signs.iter().enumerate() {
                if let Some(prev) = prev_slot {
                    if sign.edge_slot <= prev {
                        return Err(RpackError::Validation(
                            "dest_signs is not strictly increasing by edge_slot".into(),
                        ));
                    }
                }
                prev_slot = Some(sign.edge_slot);
                let Some(edge) = self.edges.get(sign.edge_slot as usize) else {
                    return Err(RpackError::Validation(format!(
                        "dest_signs[{i}] references out-of-range edge_slot {}",
                        sign.edge_slot
                    )));
                };
                if edge.ch_middle_node != CH_MIDDLE_NODE_NONE {
                    return Err(RpackError::Validation(format!(
                        "dest_signs[{i}] edge_slot {} is a shortcut, not an original edge",
                        sign.edge_slot
                    )));
                }
                if sign.dest_id >= n_strings
                    || sign.dest_ref_id >= n_strings
                    || sign.junction_ref_id >= n_strings
                {
                    return Err(RpackError::Validation(format!(
                        "dest_signs[{i}] references out-of-range string id"
                    )));
                }
            }

            let mut prev_node: Option<u32> = None;
            for (i, exit) in self.exit_refs.iter().enumerate() {
                if let Some(prev) = prev_node {
                    if exit.node_id <= prev {
                        return Err(RpackError::Validation(
                            "exit_refs is not strictly increasing by node_id".into(),
                        ));
                    }
                }
                prev_node = Some(exit.node_id);
                if exit.node_id as usize >= n_nodes {
                    return Err(RpackError::Validation(format!(
                        "exit_refs[{i}] references out-of-range node {}",
                        exit.node_id
                    )));
                }
                if exit.ref_id >= n_strings {
                    return Err(RpackError::Validation(format!(
                        "exit_refs[{i}] references out-of-range string id {}",
                        exit.ref_id
                    )));
                }
            }
        }

        Ok(())
    }
}
