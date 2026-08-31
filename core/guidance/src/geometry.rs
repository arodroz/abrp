//! Geometry/bearing primitives for maneuver classification (wayfinder #66):
//! finding an edge's `from` node from its CSR slot, and the entry/exit
//! bearing of an edge at a junction, both computed from the cold GEOMETRY
//! section rather than the node coordinates alone (a road curves right up
//! to the junction; the last/first geometry segment is what a driver
//! actually perceives as "the direction of travel").

use packs::{EdgeHot, Rpack};

/// The CSR row (node id) owning edge `edge_slot`: the CSR row index is
/// monotone nondecreasing, so the owning node is the last row whose start is
/// `<= edge_slot`. `EdgeHot` itself only carries the target, not the source.
/// Mirrors `pipeline/src/bin/inspect_rpack.rs::slot_from`.
pub(crate) fn from_node(pack: &Rpack, edge_slot: usize) -> u32 {
    let csr = pack.csr_first_edge();
    (csr.partition_point(|&start| start as usize <= edge_slot) - 1) as u32
}

/// Bearing from `from` to `to` in degrees clockwise from north, normalized
/// to `[0, 360)`. Standard great-circle initial bearing formula.
fn bearing_deg(from: (f64, f64), to: (f64, f64)) -> f64 {
    let (lat1, lon1) = (from.0.to_radians(), from.1.to_radians());
    let (lat2, lon2) = (to.0.to_radians(), to.1.to_radians());
    let dlon = lon2 - lon1;
    let y = dlon.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlon.cos();
    let theta = y.atan2(x).to_degrees();
    (theta + 360.0) % 360.0
}

/// Bearing of an edge's endpoint nodes, used only as the degenerate-geometry
/// fallback below.
fn endpoint_bearing(pack: &Rpack, edge_slot: usize, edge: &EdgeHot) -> f64 {
    let from = pack.nodes()[from_node(pack, edge_slot) as usize];
    let to = pack.nodes()[edge.target as usize];
    bearing_deg(
        (from.lat as f64, from.lon as f64),
        (to.lat as f64, to.lon as f64),
    )
}

/// The bearing a driver arrives at the junction with: the direction of the
/// LAST distinct geometry segment of the incoming edge, walking backward
/// from the end and skipping zero-length segments (consecutive identical
/// vertices). Falls back to the straight line between the edge's endpoint
/// nodes if the whole geometry is degenerate (fewer than 2 vertices, or all
/// vertices identical).
pub(crate) fn entry_bearing(pack: &Rpack, edge_slot: usize, edge: &EdgeHot) -> f64 {
    let verts = pack.geometry_for_edge(edge);
    if verts.len() >= 2 {
        let last = verts[verts.len() - 1];
        for v in verts[..verts.len() - 1].iter().rev() {
            if v.lat != last.lat || v.lon != last.lon {
                return bearing_deg(
                    (v.lat as f64, v.lon as f64),
                    (last.lat as f64, last.lon as f64),
                );
            }
        }
    }
    endpoint_bearing(pack, edge_slot, edge)
}

/// The bearing a driver leaves the junction with: the direction of the
/// FIRST distinct geometry segment of the outgoing edge, walking forward
/// from the start and skipping zero-length segments. Same degenerate
/// fallback as [`entry_bearing`].
pub(crate) fn exit_bearing(pack: &Rpack, edge_slot: usize, edge: &EdgeHot) -> f64 {
    let verts = pack.geometry_for_edge(edge);
    if verts.len() >= 2 {
        let first = verts[0];
        for v in &verts[1..] {
            if v.lat != first.lat || v.lon != first.lon {
                return bearing_deg(
                    (first.lat as f64, first.lon as f64),
                    (v.lat as f64, v.lon as f64),
                );
            }
        }
    }
    endpoint_bearing(pack, edge_slot, edge)
}

/// Normalizes a bearing difference into `(-180, 180]`. Positive = right
/// turn, negative = left.
pub(crate) fn normalize_signed(deg: f64) -> f64 {
    let mut d = deg % 360.0;
    if d <= -180.0 {
        d += 360.0;
    } else if d > 180.0 {
        d -= 360.0;
    }
    d
}

/// Signed turn delta from `entry` to the exit bearing of `(edge_slot,
/// edge)`: `normalize_signed(exit_bearing - entry)`.
pub(crate) fn delta_from(pack: &Rpack, entry: f64, edge_slot: usize, edge: &EdgeHot) -> f64 {
    normalize_signed(exit_bearing(pack, edge_slot, edge) - entry)
}
