//! Contraction Hierarchy preprocessing: turns a base (uncontracted)
//! `RegionGraphModel` into a contracted one carrying CH shortcuts and a
//! node contraction order, per ADR 0007.
//!
//! Node priority is `edge_difference + deleted_neighbours`, evaluated
//! lazily (recomputed on pop, re-queued if stale) so we never do an O(n)
//! pass over every node per contraction. Per-contraction witness search is a
//! settle-capped, cost-capped local Dijkstra from each in-neighbour of the
//! node being contracted.

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::Instant;

use packs::{EdgeHot, RegionGraphModel};

/// Witness-search settle cap: a local Dijkstra that has already settled this
/// many nodes gives up and assumes no witness exists (conservative: may add
/// an unnecessary shortcut, never drops a needed one down here since the
/// through-cost cap below also bounds the search). 50 was enough for the
/// corridor but collapses at eu-west scale: with every core search capping
/// out, unnecessary shortcuts densify the hierarchy top faster than it
/// shrinks (37 M shortcuts and ~40 h projected on 11.4 M nodes). At 500 the
/// searches find their witnesses and the core stays sparse.
const WITNESS_SETTLE_CAP: usize = 500;

/// Stats from one `ch_prepare` run.
#[derive(Debug, Clone, Copy)]
pub struct ChStats {
    pub shortcuts_added: usize,
    /// The largest number of nodes any single witness-search Dijkstra
    /// settled before stopping (capped at `WITNESS_SETTLE_CAP`).
    pub max_settled: usize,
}

/// One edge in the working adjacency used during contraction: enough to run
/// Dijkstra and to reconstruct a shortcut's aggregate physical attributes.
/// Costs/lengths are f64 through contraction; only the final `EdgeHot` casts
/// down to f32.
#[derive(Clone, Copy)]
struct WorkEdge {
    other: u32,
    cost: f64,
    length_m: f64,
    ascent_m: f64,
    descent_m: f64,
}

fn edge_cost(e: &EdgeHot) -> f64 {
    e.length_m as f64 / e.speed_kmh as f64 * 3.6
}

/// A small helper wrapping `(cost, node)` with a total order over f64 so it
/// can live in a `BinaryHeap`. All costs here are finite and non-negative.
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

/// Bounded witness Dijkstra from `source`, over the current working graph,
/// excluding `avoid` (the node being contracted) entirely. Stops once it has
/// settled `WITNESS_SETTLE_CAP` nodes or the frontier's minimum exceeds
/// `cost_cap`. Returns the settled distances and how many nodes were
/// actually settled (for stats).
fn witness_dijkstra(
    fwd: &[Vec<WorkEdge>],
    contracted: &[bool],
    source: u32,
    avoid: u32,
    cost_cap: f64,
) -> (std::collections::HashMap<u32, f64>, usize) {
    let mut dist: std::collections::HashMap<u32, f64> = std::collections::HashMap::new();
    let mut settled: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut heap: BinaryHeap<Reverse<HeapKey>> = BinaryHeap::new();

    dist.insert(source, 0.0);
    heap.push(Reverse(HeapKey(0.0, source)));

    while let Some(Reverse(HeapKey(d, node))) = heap.pop() {
        if settled.contains(&node) {
            continue;
        }
        if d > cost_cap {
            break;
        }
        if dist.get(&node).is_some_and(|&best| d > best) {
            continue;
        }
        settled.insert(node);
        if settled.len() >= WITNESS_SETTLE_CAP {
            break;
        }
        for edge in &fwd[node as usize] {
            if edge.other == avoid || contracted[edge.other as usize] {
                continue;
            }
            let nd = d + edge.cost;
            if nd > cost_cap {
                continue;
            }
            if dist.get(&edge.other).is_none_or(|&best| nd < best) {
                dist.insert(edge.other, nd);
                heap.push(Reverse(HeapKey(nd, edge.other)));
            }
        }
    }

    let settled_count = settled.len();
    (dist, settled_count)
}

/// One shortcut that contracting `v` would introduce (or does introduce, if
/// committed): `from -> to` via middle `v`, with its aggregate cost/length.
struct PendingShortcut {
    from: u32,
    to: u32,
    cost: f64,
    length_m: f64,
    ascent_m: f64,
    descent_m: f64,
}

/// Computes the shortcuts that contracting `v` requires, given the current
/// working graph. Already deduped against existing (cheaper-or-equal)
/// parallel edges, so the returned list's length is exactly the
/// `shortcuts_added` term of `v`'s edge difference. Read-only: never mutates
/// `fwd`/`bwd` -- callers that decide to commit apply the returned list
/// themselves.
fn shortcuts_for_contraction(
    fwd: &[Vec<WorkEdge>],
    bwd: &[Vec<WorkEdge>],
    contracted: &[bool],
    v: u32,
    max_settled: &mut usize,
) -> Vec<PendingShortcut> {
    let mut out = Vec::new();

    let in_neighbours: Vec<WorkEdge> = bwd[v as usize]
        .iter()
        .copied()
        .filter(|e| !contracted[e.other as usize])
        .collect();
    let out_neighbours: Vec<WorkEdge> = fwd[v as usize]
        .iter()
        .copied()
        .filter(|e| !contracted[e.other as usize])
        .collect();

    if in_neighbours.is_empty() || out_neighbours.is_empty() {
        return out;
    }

    for inb in &in_neighbours {
        let u = inb.other;
        let cost_uv = inb.cost;

        let cap = out_neighbours
            .iter()
            .filter(|outb| outb.other != u)
            .map(|outb| cost_uv + outb.cost)
            .fold(0.0_f64, f64::max);
        if cap <= 0.0 {
            continue;
        }

        let (witness, settled) = witness_dijkstra(fwd, contracted, u, v, cap);
        *max_settled = (*max_settled).max(settled);

        for outb in &out_neighbours {
            let w = outb.other;
            if w == u {
                continue;
            }
            let through = cost_uv + outb.cost;
            let witness_dist = witness.get(&w).copied().unwrap_or(f64::INFINITY);
            if witness_dist <= through {
                continue; // a witness path already covers this shortcut.
            }

            // Parallel-edge dedup: skip if a cheaper-or-equal u->w edge (or
            // shortcut) is already in the working graph.
            let already_covered = fwd[u as usize]
                .iter()
                .any(|e| e.other == w && e.cost <= through);
            if already_covered {
                continue;
            }

            out.push(PendingShortcut {
                from: u,
                to: w,
                cost: through,
                length_m: inb.length_m + outb.length_m,
                ascent_m: inb.ascent_m + outb.ascent_m,
                descent_m: inb.descent_m + outb.descent_m,
            });
        }
    }

    out
}

/// Contracts `base` into a CH-ready model: original edges preserved
/// unchanged, shortcut edges appended, CSR rebuilt over all edges, and
/// `ch_order` filled with each node's contraction rank (0 = contracted
/// first / least important, n-1 = contracted last / most important). `base`
/// must have every edge's `ch_middle_node == CH_MIDDLE_NODE_NONE`; its
/// `ch_order` content is ignored.
pub fn ch_prepare(base: &RegionGraphModel) -> (RegionGraphModel, ChStats) {
    let n = base.nodes.len();
    let mut fwd: Vec<Vec<WorkEdge>> = vec![Vec::new(); n];
    let mut bwd: Vec<Vec<WorkEdge>> = vec![Vec::new(); n];

    for (node, window) in base.csr_first_edge.windows(2).enumerate() {
        let (start, end) = (window[0] as usize, window[1] as usize);
        for &e in &base.edges[start..end] {
            let cost = edge_cost(&e);
            fwd[node].push(WorkEdge {
                other: e.target,
                cost,
                length_m: e.length_m as f64,
                ascent_m: e.ascent_m as f64,
                descent_m: e.descent_m as f64,
            });
            bwd[e.target as usize].push(WorkEdge {
                other: node as u32,
                cost,
                length_m: e.length_m as f64,
                ascent_m: e.ascent_m as f64,
                descent_m: e.descent_m as f64,
            });
        }
    }

    let mut contracted = vec![false; n];
    let mut deleted_neighbours = vec![0i32; n];
    let mut ch_order = vec![0u32; n];
    let mut max_settled = 0usize;
    let mut shortcuts: Vec<(u32, EdgeHot)> = Vec::new(); // (from, edge)

    fn evaluate(
        fwd: &[Vec<WorkEdge>],
        bwd: &[Vec<WorkEdge>],
        contracted: &[bool],
        deleted_neighbours: &[i32],
        v: u32,
        max_settled: &mut usize,
    ) -> (i64, Vec<PendingShortcut>) {
        let pending = shortcuts_for_contraction(fwd, bwd, contracted, v, max_settled);
        let removed = fwd[v as usize]
            .iter()
            .filter(|e| !contracted[e.other as usize])
            .count() as i64
            + bwd[v as usize]
                .iter()
                .filter(|e| !contracted[e.other as usize])
                .count() as i64;
        let priority = pending.len() as i64 - removed + deleted_neighbours[v as usize] as i64;
        (priority, pending)
    }

    let t0 = Instant::now();
    let mut last_log = Instant::now();
    let mut re_evals: u64 = 0;

    let mut heap: BinaryHeap<Reverse<(i64, u32)>> = BinaryHeap::new();
    for v in 0..n as u32 {
        let (p, _) = evaluate(
            &fwd,
            &bwd,
            &contracted,
            &deleted_neighbours,
            v,
            &mut max_settled,
        );
        heap.push(Reverse((p, v)));
        if last_log.elapsed().as_secs() >= 60 {
            println!(
                "[ch] ordering: {}/{} initial evaluations, {:.0}s",
                v + 1,
                n,
                t0.elapsed().as_secs_f64()
            );
            last_log = Instant::now();
        }
    }

    let mut rank = 0u32;
    while let Some(Reverse((p, v))) = heap.pop() {
        if contracted[v as usize] {
            continue;
        }
        let (fresh, pending) = evaluate(
            &fwd,
            &bwd,
            &contracted,
            &deleted_neighbours,
            v,
            &mut max_settled,
        );
        if last_log.elapsed().as_secs() >= 60 {
            println!(
                "[ch] contraction: {}/{} nodes, heap {}, shortcuts {}, re-evals {}, max_settled {}, {:.0}s",
                rank,
                n,
                heap.len(),
                shortcuts.len(),
                re_evals,
                max_settled,
                t0.elapsed().as_secs_f64()
            );
            last_log = Instant::now();
        }
        if fresh > p {
            re_evals += 1;
            heap.push(Reverse((fresh, v)));
            continue;
        }

        // Commit: actually contract v, applying the shortcuts just computed.
        for s in &pending {
            let shortcut = EdgeHot {
                target: s.to,
                length_m: s.length_m as f32,
                speed_kmh: if s.cost > 0.0 {
                    (s.length_m * 3.6 / s.cost) as f32
                } else {
                    50.0
                },
                ascent_m: s.ascent_m as f32,
                descent_m: s.descent_m as f32,
                road_class: 0,
                _pad: [0; 3],
                ch_middle_node: v,
                geom_offset: 0,
                geom_count: 0,
            };
            fwd[s.from as usize].push(WorkEdge {
                other: s.to,
                cost: s.cost,
                length_m: s.length_m,
                ascent_m: s.ascent_m,
                descent_m: s.descent_m,
            });
            bwd[s.to as usize].push(WorkEdge {
                other: s.from,
                cost: s.cost,
                length_m: s.length_m,
                ascent_m: s.ascent_m,
                descent_m: s.descent_m,
            });
            shortcuts.push((s.from, shortcut));
        }

        for e in &fwd[v as usize] {
            if !contracted[e.other as usize] {
                deleted_neighbours[e.other as usize] += 1;
            }
        }
        for e in &bwd[v as usize] {
            if !contracted[e.other as usize] {
                deleted_neighbours[e.other as usize] += 1;
            }
        }

        contracted[v as usize] = true;
        ch_order[v as usize] = rank;
        rank += 1;
    }

    // Rebuild edges + CSR: originals first (in their existing CSR order,
    // which already groups by `from`), then shortcuts, stable-sorted by
    // `from` so each node's group keeps its originals ahead of its
    // shortcuts.
    let mut all: Vec<(u32, EdgeHot)> = Vec::with_capacity(base.edges.len() + shortcuts.len());
    for (node, window) in base.csr_first_edge.windows(2).enumerate() {
        let (start, end) = (window[0] as usize, window[1] as usize);
        for &e in &base.edges[start..end] {
            all.push((node as u32, e));
        }
    }
    all.extend(shortcuts.iter().copied());
    all.sort_by_key(|(from, _)| *from);

    let mut counts = vec![0u32; n];
    for (from, _) in &all {
        counts[*from as usize] += 1;
    }
    let mut csr_first_edge = vec![0u32; n + 1];
    for i in 0..n {
        csr_first_edge[i + 1] = csr_first_edge[i] + counts[i];
    }

    let shortcuts_added = all.len() - base.edges.len();
    let edges: Vec<EdgeHot> = all.into_iter().map(|(_, e)| e).collect();

    let model = RegionGraphModel {
        nodes: base.nodes.clone(),
        csr_first_edge,
        edges,
        ch_order,
        geometry: base.geometry.clone(),
        snap_grid: base.snap_grid.clone(),
    };

    (
        model,
        ChStats {
            shortcuts_added,
            max_settled,
        },
    )
}
