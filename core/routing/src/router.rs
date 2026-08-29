//! Production CH query kernel: bidirectional point-to-point search and
//! bucket-based many-to-many, both over a `&Rpack`'s baked CSR/reverse-CSR
//! and `ch_order`.

use std::borrow::Cow;
use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

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

/// One direction's search state for `p2p`, keyed sparsely by node id. A CH
/// query settles a few thousand nodes out of millions, so dense per-query
/// `vec![...; node_count]` state -- ~390 MB allocated and zero-filled PER
/// QUERY at eu-west's 11.4 M nodes -- dominated both cold-plan time and the
/// planner's memory ceiling (the M3 gate's 91,313,632-byte allocator abort
/// was exactly one of those vectors). Sparse storage makes a query's cost
/// track its search space; the traversal itself (pop order, float
/// arithmetic, tie-breaking) is unchanged, so routes are bit-identical to
/// the dense implementation's (issue #50).
#[derive(Default)]
struct SearchLabels {
    map: HashMap<u32, NodeLabel>,
}

struct NodeLabel {
    dist: f64,
    settled: bool,
    prev_edge: u32,
}

impl SearchLabels {
    fn dist(&self, node: u32) -> f64 {
        self.map.get(&node).map_or(f64::INFINITY, |l| l.dist)
    }

    fn is_settled(&self, node: u32) -> bool {
        self.map.get(&node).is_some_and(|l| l.settled)
    }

    fn settle(&mut self, node: u32) {
        self.map
            .get_mut(&node)
            .expect("settling a node that was never labelled")
            .settled = true;
    }

    fn prev_edge(&self, node: u32) -> Option<u32> {
        self.map.get(&node).map(|l| l.prev_edge)
    }

    /// Seeds the search origin: distance 0, no incoming edge.
    fn seed(&mut self, node: u32) {
        self.map.insert(
            node,
            NodeLabel {
                dist: 0.0,
                settled: false,
                prev_edge: u32::MAX,
            },
        );
    }

    /// Relaxation write: records `dist`/`prev_edge` for `node`. Only called
    /// when the caller has already established `dist < self.dist(node)`.
    fn improve(&mut self, node: u32, dist: f64, prev_edge: u32) {
        let label = self.map.entry(node).or_insert(NodeLabel {
            dist,
            settled: false,
            prev_edge,
        });
        label.dist = dist;
        label.prev_edge = prev_edge;
    }
}

/// A CH query kernel over one open `Rpack`. Holds a precomputed `from` node
/// per edge index (the pack stores only `target` per edge; `from` is
/// recovered once from the forward CSR here rather than re-derived per
/// query).
pub struct Router<'a> {
    pack: &'a Rpack,
    from_of_edge: Cow<'a, [u32]>,
}

impl<'a> Router<'a> {
    pub fn new(pack: &'a Rpack) -> Self {
        Router {
            pack,
            from_of_edge: Cow::Owned(Self::precompute_from_of_edge(pack)),
        }
    }

    /// The `from` node per edge index, recovered from the forward CSR. An
    /// O(edges) pass allocating 4 bytes/edge (~100 MB at eu-west scale), so
    /// a long-lived caller (the FFI `Planner`) computes it once and hands it
    /// to [`Router::with_from_of_edge`] per query batch instead of paying it
    /// inside every `Router::new` (issue #50).
    pub fn precompute_from_of_edge(pack: &Rpack) -> Vec<u32> {
        let csr = pack.csr_first_edge();
        let mut from_of_edge = vec![0u32; pack.edges().len()];
        for (node, window) in csr.windows(2).enumerate() {
            from_of_edge[window[0] as usize..window[1] as usize].fill(node as u32);
        }
        from_of_edge
    }

    /// As [`Router::new`], but borrowing a `from_of_edge` the caller already
    /// computed via [`Router::precompute_from_of_edge`] for this same pack.
    pub fn with_from_of_edge(pack: &'a Rpack, from_of_edge: &'a [u32]) -> Self {
        debug_assert_eq!(from_of_edge.len(), pack.edges().len());
        Router {
            pack,
            from_of_edge: Cow::Borrowed(from_of_edge),
        }
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

    /// Stall-on-demand for the forward search (Geisberger et al.): `v` is
    /// stalled if some node `u` with strictly HIGHER contraction rank
    /// already has a forward label (`dist_f[u]` finite, from some other
    /// ascending path from `source` that never went through `v`) and an
    /// edge `u -> v` -- a "down" edge from `u`'s perspective, which the
    /// up-only forward relaxation deliberately never follows when it
    /// settles `u` (`u`'s own relaxation only continues to strictly-higher
    /// ranks, never back down to `v`) -- such that `dist_f[u] + w(u,v)` is
    /// cheaper than `v`'s current label. That witness proves `dist_f[v]` is
    /// not `v`'s true shortest distance from `source`, so `v` cannot be the
    /// peak of the true shortest path -- its up-edges are not relaxed.
    /// Checking `ch_order[u] < ch_order[v]` here instead would be a
    /// tautology: those are exactly the up-edges the search already used
    /// when it settled `u`, so by the heap's monotonic pop order
    /// `dist_f[u] + w >= dist_f[v]` always holds and the check could never
    /// fire. Uses the pack's baked reverse CSR to find `v`'s incoming edges
    /// without a linear scan.
    fn forward_is_stalled(&self, v: u32, labels_f: &SearchLabels, ch_order: &[u32]) -> bool {
        let Some(in_edges) = self.pack.reverse_edge_ids_for(v) else {
            return false;
        };
        in_edges.iter().any(|&idx| {
            let u = self.from_of_edge[idx as usize];
            ch_order[u as usize] > ch_order[v as usize]
                && labels_f.dist(u) + edge_cost(&self.pack.edges()[idx as usize]) < labels_f.dist(v)
        })
    }

    /// Stall-on-demand for the backward search: the mirror of
    /// `forward_is_stalled`. The backward search settles nodes via the
    /// reverse CSR, extending to strictly HIGHER-ranked predecessors; the
    /// edge set it never follows from a settled node `u` is `u`'s own
    /// outgoing edges (original forward CSR) to a node `w` that is *also*
    /// strictly higher-ranked than `u` but was reached earlier via some
    /// other path -- so `w`'s already-known `dist_b[w]` can undercut `u`'s
    /// label via that unfollowed edge. Mirrors forward's use of the CSR
    /// opposite the one its own relaxation walks, with the same
    /// higher-rank-witness filter.
    fn backward_is_stalled(&self, u: u32, labels_b: &SearchLabels, ch_order: &[u32]) -> bool {
        let Some(range) = self.pack.edge_range(u) else {
            return false;
        };
        range.into_iter().any(|idx| {
            let e = &self.pack.edges()[idx];
            let w = e.target;
            ch_order[w as usize] > ch_order[u as usize]
                && labels_b.dist(w) + edge_cost(e) < labels_b.dist(u)
        })
    }

    /// Bidirectional CH point-to-point search: forward search relaxes only
    /// edges going "up" in contraction rank; backward search does the same
    /// over the reverse index. Stall-on-demand prunes, in each direction,
    /// any settled node provably not on the true shortest path (a
    /// higher-ranked node already reaches it more cheaply via a "down" edge
    /// the search's own up-only relaxation never follows) -- such a node's
    /// up-edges are not relaxed, and it is not considered as a meeting
    /// candidate.
    pub fn p2p(&self, source: u32, target: u32) -> Option<Route> {
        if source == target {
            return Some(Route {
                cost_seconds: 0.0,
                length_m: 0.0,
                nodes: vec![source],
                edges: vec![],
            });
        }

        let ch_order = self.pack.ch_order();

        let mut labels_f = SearchLabels::default();
        let mut labels_b = SearchLabels::default();

        let mut heap_f: BinaryHeap<Reverse<HeapKey>> = BinaryHeap::new();
        let mut heap_b: BinaryHeap<Reverse<HeapKey>> = BinaryHeap::new();
        labels_f.seed(source);
        labels_b.seed(target);
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
                if !labels_f.is_settled(u) && d <= labels_f.dist(u) {
                    labels_f.settle(u);
                    if !self.forward_is_stalled(u, &labels_f, ch_order) {
                        if labels_b.is_settled(u) {
                            let total = labels_f.dist(u) + labels_b.dist(u);
                            if total < best {
                                best = total;
                                meeting = Some(u);
                            }
                        }
                        if let Some(range) = self.pack.edge_range(u) {
                            for idx in range {
                                let e = &self.pack.edges()[idx];
                                if ch_order[e.target as usize] > ch_order[u as usize] {
                                    let nd = labels_f.dist(u) + edge_cost(e);
                                    if nd < labels_f.dist(e.target) {
                                        labels_f.improve(e.target, nd, idx as u32);
                                        heap_f.push(Reverse(HeapKey(nd, e.target)));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if can_b {
                let Reverse(HeapKey(d, u)) = heap_b.pop().unwrap();
                if !labels_b.is_settled(u) && d <= labels_b.dist(u) {
                    labels_b.settle(u);
                    if !self.backward_is_stalled(u, &labels_b, ch_order) {
                        if labels_f.is_settled(u) {
                            let total = labels_f.dist(u) + labels_b.dist(u);
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
                                    let nd = labels_b.dist(u) + edge_cost(e);
                                    if nd < labels_b.dist(src) {
                                        labels_b.improve(src, nd, idx);
                                        heap_b.push(Reverse(HeapKey(nd, src)));
                                    }
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
            let idx = labels_f
                .prev_edge(cur)
                .expect("meeting node is reachable from source");
            forward_edges.push(idx);
            cur = self.from_of_edge[idx as usize];
        }
        forward_edges.reverse();

        // Walk the backward tree forward from `meeting` to `target`.
        let mut backward_edges = Vec::new();
        let mut cur = meeting;
        while cur != target {
            let idx = labels_b
                .prev_edge(cur)
                .expect("meeting node reaches target");
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
