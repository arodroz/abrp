//! Production CH query kernel: bidirectional point-to-point search and
//! bucket-based many-to-many, both over a `&Rpack`'s baked CSR/reverse-CSR
//! and `ch_order`.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use packs::{EdgeHot, Rpack, CH_MIDDLE_NODE_NONE};

/// A resolved point-to-point route: cost and length are summed over the
/// *unpacked* original edges (never the shortcut aggregate), so they are
/// exact against the base graph's per-edge attributes, not a rounded
/// reconstruction.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub cost_seconds: f64,
    pub length_m: f64,
    pub nodes: Vec<u32>,
    pub edges: Vec<u32>,
}

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

fn edge_cost(e: &EdgeHot) -> f64 {
    e.length_m as f64 / e.speed_kmh as f64 * 3.6
}

/// A CH query kernel over one open `Rpack`. Holds a precomputed `from` node
/// per edge index (the pack stores only `target` per edge; `from` is
/// recovered once from the forward CSR here rather than re-derived per
/// query).
pub struct Router<'a> {
    pack: &'a Rpack,
    from_of_edge: Vec<u32>,
}

impl<'a> Router<'a> {
    pub fn new(pack: &'a Rpack) -> Self {
        let csr = pack.csr_first_edge();
        let mut from_of_edge = vec![0u32; pack.edges().len()];
        for (node, window) in csr.windows(2).enumerate() {
            from_of_edge[window[0] as usize..window[1] as usize].fill(node as u32);
        }
        Router { pack, from_of_edge }
    }

    /// Finds, among the edges leaving `from` that target `to`, the one with
    /// minimum cost. Shortcuts unpack by re-deriving their constituent edge
    /// this way rather than storing sub-edge links, so ties are broken
    /// arbitrarily (first minimum found) -- unpacking never relies on float
    /// equality between a shortcut's stored cost and a recomputed one.
    fn min_cost_edge_index(&self, from: u32, to: u32) -> u32 {
        let range = self.pack.edge_range(from).expect("from is a valid node");
        range
            .filter(|&idx| self.pack.edges()[idx].target == to)
            .min_by(|&a, &b| {
                edge_cost(&self.pack.edges()[a]).total_cmp(&edge_cost(&self.pack.edges()[b]))
            })
            .expect("from->to edge must exist: caller derived it from the search tree")
            as u32
    }

    /// Recursively unpacks `edge_idx` into original edges (in travel order),
    /// appending them to `out`. A shortcut `u -> w` via middle `m` unpacks
    /// to (min-cost edge `u -> m`, min-cost edge `m -> w`), recursively.
    fn unpack_edge(&self, edge_idx: u32, out: &mut Vec<u32>) {
        let e = &self.pack.edges()[edge_idx as usize];
        if e.ch_middle_node == CH_MIDDLE_NODE_NONE {
            out.push(edge_idx);
            return;
        }
        let from = self.from_of_edge[edge_idx as usize];
        let mid = e.ch_middle_node;
        let to = e.target;
        let left = self.min_cost_edge_index(from, mid);
        self.unpack_edge(left, out);
        let right = self.min_cost_edge_index(mid, to);
        self.unpack_edge(right, out);
    }

    /// Bidirectional CH point-to-point search: forward search relaxes only
    /// edges going "up" in contraction rank; backward search does the same
    /// over the reverse index. No stall-on-demand in v1 -- known perf lever,
    /// left for a later pass once query latency is measured.
    pub fn p2p(&self, source: u32, target: u32) -> Option<Route> {
        if source == target {
            return Some(Route {
                cost_seconds: 0.0,
                length_m: 0.0,
                nodes: vec![source],
                edges: vec![],
            });
        }

        let n = self.pack.node_count();
        let ch_order = self.pack.ch_order();

        let mut dist_f = vec![f64::INFINITY; n];
        let mut dist_b = vec![f64::INFINITY; n];
        let mut settled_f = vec![false; n];
        let mut settled_b = vec![false; n];
        let mut prev_edge_f: Vec<Option<u32>> = vec![None; n];
        let mut prev_edge_b: Vec<Option<u32>> = vec![None; n];

        let mut heap_f: BinaryHeap<Reverse<HeapKey>> = BinaryHeap::new();
        let mut heap_b: BinaryHeap<Reverse<HeapKey>> = BinaryHeap::new();
        dist_f[source as usize] = 0.0;
        dist_b[target as usize] = 0.0;
        heap_f.push(Reverse(HeapKey(0.0, source)));
        heap_b.push(Reverse(HeapKey(0.0, target)));

        let mut best = f64::INFINITY;
        let mut meeting: Option<u32> = None;

        loop {
            let f_top = heap_f.peek().map(|r| r.0 .0);
            let b_top = heap_b.peek().map(|r| r.0 .0);
            let can_f = f_top.is_some_and(|d| d <= best);
            let can_b = b_top.is_some_and(|d| d <= best);
            if !can_f && !can_b {
                break;
            }

            if can_f {
                let Reverse(HeapKey(d, u)) = heap_f.pop().unwrap();
                if !settled_f[u as usize] && d <= dist_f[u as usize] {
                    settled_f[u as usize] = true;
                    if settled_b[u as usize] {
                        let total = dist_f[u as usize] + dist_b[u as usize];
                        if total < best {
                            best = total;
                            meeting = Some(u);
                        }
                    }
                    if let Some(range) = self.pack.edge_range(u) {
                        for idx in range {
                            let e = &self.pack.edges()[idx];
                            if ch_order[e.target as usize] > ch_order[u as usize] {
                                let nd = dist_f[u as usize] + edge_cost(e);
                                if nd < dist_f[e.target as usize] {
                                    dist_f[e.target as usize] = nd;
                                    prev_edge_f[e.target as usize] = Some(idx as u32);
                                    heap_f.push(Reverse(HeapKey(nd, e.target)));
                                }
                            }
                        }
                    }
                }
            }

            if can_b {
                let Reverse(HeapKey(d, u)) = heap_b.pop().unwrap();
                if !settled_b[u as usize] && d <= dist_b[u as usize] {
                    settled_b[u as usize] = true;
                    if settled_f[u as usize] {
                        let total = dist_f[u as usize] + dist_b[u as usize];
                        if total < best {
                            best = total;
                            meeting = Some(u);
                        }
                    }
                    if let Some(in_edges) = self.pack.reverse_edge_ids_for(u) {
                        for &idx in in_edges {
                            let src = self.from_of_edge[idx as usize];
                            if ch_order[src as usize] > ch_order[u as usize] {
                                let e = &self.pack.edges()[idx as usize];
                                let nd = dist_b[u as usize] + edge_cost(e);
                                if nd < dist_b[src as usize] {
                                    dist_b[src as usize] = nd;
                                    prev_edge_b[src as usize] = Some(idx);
                                    heap_b.push(Reverse(HeapKey(nd, src)));
                                }
                            }
                        }
                    }
                }
            }
        }

        let meeting = meeting?;

        // Walk the forward tree back from `meeting` to `source`.
        let mut forward_edges = Vec::new();
        let mut cur = meeting;
        while cur != source {
            let idx = prev_edge_f[cur as usize].expect("meeting node is reachable from source");
            forward_edges.push(idx);
            cur = self.from_of_edge[idx as usize];
        }
        forward_edges.reverse();

        // Walk the backward tree forward from `meeting` to `target`.
        let mut backward_edges = Vec::new();
        let mut cur = meeting;
        while cur != target {
            let idx = prev_edge_b[cur as usize].expect("meeting node reaches target");
            backward_edges.push(idx);
            cur = self.pack.edges()[idx as usize].target;
        }

        let mut original_edges = Vec::new();
        for idx in forward_edges.into_iter().chain(backward_edges) {
            self.unpack_edge(idx, &mut original_edges);
        }

        let mut cost_seconds = 0.0;
        let mut length_m = 0.0;
        let mut nodes = Vec::with_capacity(original_edges.len() + 1);
        nodes.push(source);
        for &idx in &original_edges {
            let e = &self.pack.edges()[idx as usize];
            cost_seconds += edge_cost(e);
            length_m += e.length_m as f64;
            nodes.push(e.target);
        }

        Some(Route {
            cost_seconds,
            length_m,
            nodes,
            edges: original_edges,
        })
    }

    /// Full (non-early-stopping) upward Dijkstra from `source` over the
    /// forward CSR: only relaxes edges going to strictly higher rank.
    fn forward_upward_full(&self, source: u32) -> Vec<f64> {
        let n = self.pack.node_count();
        let ch_order = self.pack.ch_order();
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
            if let Some(range) = self.pack.edge_range(u) {
                for idx in range {
                    let e = &self.pack.edges()[idx];
                    if ch_order[e.target as usize] > ch_order[u as usize] {
                        let nd = d + edge_cost(e);
                        if nd < dist[e.target as usize] {
                            dist[e.target as usize] = nd;
                            heap.push(Reverse(HeapKey(nd, e.target)));
                        }
                    }
                }
            }
        }
        dist
    }

    /// Full (non-early-stopping) upward Dijkstra from `target` over the
    /// reverse CSR: only relaxes incoming edges whose source has strictly
    /// higher rank. `dist[node]` is the cost from `node` to `target`.
    fn backward_upward_full(&self, target: u32) -> Vec<f64> {
        let n = self.pack.node_count();
        let ch_order = self.pack.ch_order();
        let mut dist = vec![f64::INFINITY; n];
        let mut settled = vec![false; n];
        let mut heap: BinaryHeap<Reverse<HeapKey>> = BinaryHeap::new();
        dist[target as usize] = 0.0;
        heap.push(Reverse(HeapKey(0.0, target)));
        while let Some(Reverse(HeapKey(d, u))) = heap.pop() {
            if settled[u as usize] {
                continue;
            }
            settled[u as usize] = true;
            if let Some(in_edges) = self.pack.reverse_edge_ids_for(u) {
                for &idx in in_edges {
                    let src = self.from_of_edge[idx as usize];
                    if ch_order[src as usize] > ch_order[u as usize] {
                        let e = &self.pack.edges()[idx as usize];
                        let nd = d + edge_cost(e);
                        if nd < dist[src as usize] {
                            dist[src as usize] = nd;
                            heap.push(Reverse(HeapKey(nd, src)));
                        }
                    }
                }
            }
        }
        dist
    }

    /// Bucket-based CH many-to-many (Knopp et al.): costs only, no paths.
    /// Each search runs to exhaustion of the upward graph -- correctness
    /// first, no early stopping.
    pub fn many_to_many(&self, sources: &[u32], targets: &[u32]) -> Vec<Vec<Option<f64>>> {
        let n = self.pack.node_count();
        let mut buckets: Vec<Vec<(usize, f64)>> = vec![Vec::new(); n];

        for (ti, &t) in targets.iter().enumerate() {
            let dist_b = self.backward_upward_full(t);
            for (node, &d) in dist_b.iter().enumerate() {
                if d.is_finite() {
                    buckets[node].push((ti, d));
                }
            }
        }

        let mut result = vec![vec![None; targets.len()]; sources.len()];
        for (si, &s) in sources.iter().enumerate() {
            let dist_f = self.forward_upward_full(s);
            let mut best = vec![f64::INFINITY; targets.len()];
            for (node, &d) in dist_f.iter().enumerate() {
                if !d.is_finite() {
                    continue;
                }
                for &(ti, bd) in &buckets[node] {
                    let total = d + bd;
                    if total < best[ti] {
                        best[ti] = total;
                    }
                }
            }
            for (ti, &b) in best.iter().enumerate() {
                result[si][ti] = if b.is_finite() { Some(b) } else { None };
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_cost_matches_length_over_speed_times_3_6() {
        let e = EdgeHot {
            target: 0,
            length_m: 1000.0,
            speed_kmh: 50.0,
            ascent_m: 0.0,
            descent_m: 0.0,
            road_class: 0,
            _pad: [0; 3],
            ch_middle_node: CH_MIDDLE_NODE_NONE,
            geom_offset: 0,
            geom_count: 0,
        };
        // 1000m at 50km/h = 72s.
        assert!((edge_cost(&e) - 72.0).abs() < 1e-9);
    }

    #[test]
    fn heap_key_orders_by_cost_then_node() {
        let a = HeapKey(1.0, 5);
        let b = HeapKey(1.0, 3);
        let c = HeapKey(2.0, 0);
        assert!(a > b);
        assert!(c > a);
    }
}
