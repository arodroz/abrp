//! Test-only builder for tiny format-2.0 `.rpack` fixtures (wayfinder #66),
//! mirroring `core/optimiser/src/plan_api.rs::tiny_pack` and
//! `pipeline/tests/rpack_roundtrip.rs`'s guidance model builder: nodes at
//! small lat/lon offsets, per-directed-edge 2-point geometry (start/end node
//! coordinates -- entry and exit bearing then just fall out of the node
//! placement below), `csr_first_edge` grouped by `from`, `ch_order`
//! identity, a trivial one-cell snap grid, and a tiny local string/attr
//! interner. All edges are original (`CH_MIDDLE_NODE_NONE`).

use std::collections::HashMap;

use packs::{
    DestSign, EdgeAttr, EdgeHot, ExitRef, GeomVertex, NodeRecord, RegionGraphModel, Rpack,
    SnapGridModel, CH_MIDDLE_NODE_NONE,
};
use pipeline::{write_rpack, PackMeta};

/// A point at `bearing_deg` (clockwise from north) and `dist_m` meters from
/// `from`, via the flat-earth approximation (1 degree latitude ~= 111_320 m,
/// 1 degree longitude ~= 111_320 * cos(lat) m) -- accurate to a small
/// fraction of a degree of bearing at the few-hundred-meter scale these
/// fixtures use, comfortably inside every classifier threshold's margin.
pub fn dest_point(from: (f64, f64), bearing_deg: f64, dist_m: f64) -> (f32, f32) {
    let theta = bearing_deg.to_radians();
    let dlat = dist_m * theta.cos() / 111_320.0;
    let dlon = dist_m * theta.sin() / (111_320.0 * from.0.to_radians().cos());
    ((from.0 + dlat) as f32, (from.1 + dlon) as f32)
}

/// A point placed so that traveling FROM it TO `junction` arrives on
/// `incoming_bearing`.
pub fn behind(junction: (f64, f64), incoming_bearing: f64, dist_m: f64) -> (f32, f32) {
    dest_point(junction, incoming_bearing + 180.0, dist_m)
}

/// A point placed so that traveling FROM `junction` TO it departs on
/// `outgoing_bearing`. Alias of [`dest_point`], named for readability at
/// call sites.
pub fn ahead(junction: (f64, f64), outgoing_bearing: f64, dist_m: f64) -> (f32, f32) {
    dest_point(junction, outgoing_bearing, dist_m)
}

/// One directed edge to build, by node index. Geometry is always the
/// 2-point straight line between `nodes[from]` and `nodes[to]` -- for a
/// 2-point edge the entry bearing (last segment) and exit bearing (first
/// segment) are both that one segment, so placing nodes via [`behind`]/
/// [`ahead`] directly engineers the bearings the classifier will see.
#[derive(Clone, Copy)]
pub struct EdgeSpec {
    pub from: usize,
    pub to: usize,
    pub length_m: f32,
    pub speed_kmh: f32,
    pub class: u8,
    pub link: bool,
    pub roundabout: bool,
    pub name: &'static str,
    pub road_ref: &'static str,
    /// Destination signage on this edge, if any: `(dest, dest_ref, junction_ref)`.
    pub dest: Option<(&'static str, &'static str, &'static str)>,
}

impl EdgeSpec {
    /// A plain, unnamed, unsigned edge with sensible defaults -- override
    /// only the fields a test cares about.
    pub fn new(from: usize, to: usize, length_m: f32) -> Self {
        EdgeSpec {
            from,
            to,
            length_m,
            speed_kmh: 50.0,
            class: packs::GUIDE_CLASS_UNCLASSIFIED,
            link: false,
            roundabout: false,
            name: "",
            road_ref: "",
            dest: None,
        }
    }
}

struct Interner {
    offsets: Vec<u32>,
    blob: Vec<u8>,
    map: HashMap<&'static str, u32>,
}

impl Interner {
    fn new() -> Self {
        Interner {
            offsets: vec![0, 0],
            blob: Vec::new(),
            map: HashMap::new(),
        }
    }

    fn intern(&mut self, s: &'static str) -> u32 {
        if s.is_empty() {
            return 0;
        }
        if let Some(&id) = self.map.get(s) {
            return id;
        }
        self.blob.extend_from_slice(s.as_bytes());
        self.offsets.push(self.blob.len() as u32);
        let id = self.offsets.len() as u32 - 2;
        self.map.insert(s, id);
        id
    }
}

struct AttrInterner {
    attrs: Vec<EdgeAttr>,
    map: HashMap<(u32, u32), u32>,
}

impl AttrInterner {
    fn new() -> Self {
        AttrInterner {
            attrs: vec![EdgeAttr {
                name_id: 0,
                ref_id: 0,
            }],
            map: HashMap::from([((0, 0), 0)]),
        }
    }

    fn intern(&mut self, name_id: u32, ref_id: u32) -> u32 {
        if let Some(&id) = self.map.get(&(name_id, ref_id)) {
            return id;
        }
        let id = self.attrs.len() as u32;
        self.attrs.push(EdgeAttr { name_id, ref_id });
        self.map.insert((name_id, ref_id), id);
        id
    }
}

/// Builds and writes a tiny format-2.0 `.rpack` from explicit node
/// coordinates and directed edges, returning the backing tempdir (keep it
/// alive) and the pack's path. Edges are grouped into CSR rows by `from`,
/// preserving each bucket's insertion order -- so alternatives at a node
/// appear in the order given.
pub fn build_pack(
    nodes: &[(f32, f32)],
    edge_specs: &[EdgeSpec],
    exit_refs_in: &[(u32, &'static str)],
) -> (tempfile::TempDir, std::path::PathBuf) {
    let n_nodes = nodes.len();
    let node_records: Vec<NodeRecord> = nodes
        .iter()
        .map(|&(lat, lon)| NodeRecord { lat, lon })
        .collect();

    let mut buckets: Vec<Vec<EdgeSpec>> = vec![Vec::new(); n_nodes];
    for spec in edge_specs {
        buckets[spec.from].push(*spec);
    }

    let mut csr_first_edge = vec![0u32; n_nodes + 1];
    let mut edges = Vec::new();
    let mut geometry = Vec::new();
    let mut edge_guide = Vec::new();
    let mut dest_signs = Vec::new();
    let mut strings = Interner::new();
    let mut attrs = AttrInterner::new();

    for (node_id, bucket) in buckets.into_iter().enumerate() {
        csr_first_edge[node_id] = edges.len() as u32;
        for spec in bucket {
            let slot = edges.len() as u32;
            let geom_offset = geometry.len() as u32;
            let (from_lat, from_lon) = nodes[spec.from];
            let (to_lat, to_lon) = nodes[spec.to];
            geometry.push(GeomVertex {
                lat: from_lat,
                lon: from_lon,
                elev_m: 0,
                _pad: 0,
            });
            geometry.push(GeomVertex {
                lat: to_lat,
                lon: to_lon,
                elev_m: 0,
                _pad: 0,
            });

            let name_id = strings.intern(spec.name);
            let ref_id = strings.intern(spec.road_ref);
            let attr_id = attrs.intern(name_id, ref_id);

            let mut guide_flags = spec.class;
            if spec.link {
                guide_flags |= packs::GUIDE_FLAG_LINK;
            }
            if spec.roundabout {
                guide_flags |= packs::GUIDE_FLAG_ROUNDABOUT;
            }

            edges.push(EdgeHot {
                target: spec.to as u32,
                length_m: spec.length_m,
                speed_kmh: spec.speed_kmh,
                ascent_m: 0.0,
                descent_m: 0.0,
                road_class: 0,
                guide_flags,
                _pad: [0, 0],
                ch_middle_node: CH_MIDDLE_NODE_NONE,
                geom_offset,
                geom_count: 2,
            });
            edge_guide.push(attr_id);

            if let Some((dest, dest_ref, junction_ref)) = spec.dest {
                let dest_id = strings.intern(dest);
                let dest_ref_id = strings.intern(dest_ref);
                let junction_ref_id = strings.intern(junction_ref);
                dest_signs.push(DestSign {
                    edge_slot: slot,
                    dest_id,
                    dest_ref_id,
                    junction_ref_id,
                });
            }
        }
    }
    csr_first_edge[n_nodes] = edges.len() as u32;

    let mut exit_refs: Vec<ExitRef> = exit_refs_in
        .iter()
        .map(|&(node_id, r)| ExitRef {
            node_id,
            ref_id: strings.intern(r),
        })
        .collect();
    exit_refs.sort_by_key(|e| e.node_id);

    let ch_order: Vec<u32> = (0..n_nodes as u32).collect();

    // Unused by the classifier (it never calls `Rpack::snap`); one cell
    // covering the whole world is the simplest valid grid.
    let snap_grid = SnapGridModel {
        min_lat: -90.0,
        min_lon: -180.0,
        cell_size_deg: 360.0,
        n_rows: 1,
        n_cols: 1,
        cell_offsets: vec![0, n_nodes as u32],
        node_ids: (0..n_nodes as u32).collect(),
    };

    let model = RegionGraphModel {
        nodes: node_records,
        csr_first_edge,
        edges,
        ch_order,
        geometry,
        snap_grid,
        string_offsets: strings.offsets,
        string_blob: strings.blob,
        edge_attrs: attrs.attrs,
        edge_guide,
        dest_signs,
        exit_refs,
    };

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test.rpack");
    write_rpack(
        &model,
        &PackMeta {
            osm_snapshot_epoch: 0,
            region_id: 0,
            region_name: "test".to_string(),
        },
        &path,
    )
    .expect("write test.rpack");
    (dir, path)
}

/// Builds and opens a pack in one call, matching most tests' needs.
pub fn open_pack(
    nodes: &[(f32, f32)],
    edge_specs: &[EdgeSpec],
    exit_refs: &[(u32, &'static str)],
) -> (tempfile::TempDir, Rpack) {
    let (dir, path) = build_pack(nodes, edge_specs, exit_refs);
    let pack = Rpack::open(&path).expect("open test.rpack");
    (dir, pack)
}
