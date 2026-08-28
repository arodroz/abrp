//! Plain Dijkstra over the base (uncontracted) `RegionGraphModel`. This is
//! test ground truth only -- not the production query path (see
//! `crate::Router` for the CH kernel).

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use packs::RegionGraphModel;

/// A `(cost, node)` pair with a total order over f64, so it can live in a
/// `BinaryHeap`. Costs here are always finite and non-negative.
#[derive(Clone, Copy, PartialEq)]
struct HeapKey(f64, u32);

impl Eq for HeapKey {}

impl PartialOrd for HeapKey {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapKey {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0).then(self.1.cmp(&other.1))
    }
}

fn edge_cost(length_m: f32, speed_kmh: f32) -> f64 {
    length_m as f64 / speed_kmh as f64 * 3.6
}

/// Shortest travel-time cost in seconds from `source` to `target` over
/// `model`'s full edge set (no CH restriction). `None` if unreachable.
/// This is the test suite's ground truth, not a production code path.
pub fn dijkstra_cost(model: &RegionGraphModel, source: u32, target: u32) -> Option<f64> {
    if source == target {
        return Some(0.0);
    }
    let n = model.nodes.len();
    if source as usize >= n || target as usize >= n {
        return None;
    }

    let mut dist = vec![f64::INFINITY; n];
    let mut settled = vec![false; n];
    let mut heap: BinaryHeap<Reverse<HeapKey>> = BinaryHeap::new();

    dist[source as usize] = 0.0;
    heap.push(Reverse(HeapKey(0.0, source)));

    while let Some(Reverse(HeapKey(d, u))) = heap.pop() {
        if settled[u as usize] {
            continue;
        }
        settled[u as usize] = true;
        if u == target {
            return Some(d);
        }
        let start = model.csr_first_edge[u as usize] as usize;
        let end = model.csr_first_edge[u as usize + 1] as usize;
        for e in &model.edges[start..end] {
            if settled[e.target as usize] {
                continue;
            }
            let nd = d + edge_cost(e.length_m, e.speed_kmh);
            if nd < dist[e.target as usize] {
                dist[e.target as usize] = nd;
                heap.push(Reverse(HeapKey(nd, e.target)));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use packs::{EdgeHot, NodeRecord, SnapGridModel, CH_MIDDLE_NODE_NONE};

    fn edge(target: u32, length_m: f32, speed_kmh: f32) -> EdgeHot {
        EdgeHot {
            target,
            length_m,
            speed_kmh,
            ascent_m: 0.0,
            descent_m: 0.0,
            road_class: 0,
            _pad: [0; 3],
            ch_middle_node: CH_MIDDLE_NODE_NONE,
            geom_offset: 0,
            geom_count: 0,
        }
    }

    fn node(lat: f32, lon: f32) -> NodeRecord {
        NodeRecord { lat, lon }
    }

    /// 0 -> 1 -> 2 direct (100s), vs. 0 -> 2 direct (50s): the shorter
    /// direct edge should win.
    fn line_model() -> RegionGraphModel {
        RegionGraphModel {
            nodes: vec![node(0.0, 0.0), node(0.0, 1.0), node(0.0, 2.0)],
            csr_first_edge: vec![0, 2, 3, 3],
            edges: vec![
                edge(1, 500.0, 18.0), // 0->1: 100s
                edge(2, 500.0, 36.0), // 0->2: 50s
                edge(2, 500.0, 18.0), // 1->2: 100s
            ],
            ch_order: vec![0, 1, 2],
            geometry: vec![],
            snap_grid: SnapGridModel::default(),
        }
    }

    #[test]
    fn finds_shortest_cost_over_multiple_paths() {
        let m = line_model();
        assert!((dijkstra_cost(&m, 0, 2).unwrap() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn source_equals_target_is_zero() {
        let m = line_model();
        assert_eq!(dijkstra_cost(&m, 1, 1), Some(0.0));
    }

    #[test]
    fn unreachable_target_is_none() {
        let m = line_model();
        // Node 2 has no outgoing edges, so 2 -> 0 is unreachable.
        assert_eq!(dijkstra_cost(&m, 2, 0), None);
    }
}
