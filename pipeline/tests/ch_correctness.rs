//! The CH gate: contracts synthetic graphs, round-trips them through the
//! `.rpack` writer/reader, and checks `routing::Router::p2p` /
//! `many_to_many` against `routing::reference::dijkstra_cost` (plain
//! Dijkstra over the base graph) for correctness.

use packs::{EdgeHot, NodeRecord, RegionGraphModel, Rpack, SnapGridModel, CH_MIDDLE_NODE_NONE};
use pipeline::{ch_prepare, write_rpack, PackMeta};
use routing::{reference, Router};

mod common;
use common::Lcg;

const MIN_LAT: f32 = 49.4;
const MIN_LON: f32 = 5.7;
const LAT_STEP: f32 = 0.0045; // ~500m
const LON_STEP: f32 = 0.0069; // ~500m at this latitude

struct GraphSpec {
    n_rows: usize,
    n_cols: usize,
    n_long_range: usize,
    n_island: usize,
}

/// A single-cell grid covering every node: valid per `RegionGraphModel`'s
/// snap grid invariants, and sufficient since these tests never call
/// `Rpack::snap`.
fn trivial_snap_grid(n_nodes: usize) -> SnapGridModel {
    SnapGridModel {
        min_lat: MIN_LAT,
        min_lon: MIN_LON,
        cell_size_deg: 1.0,
        n_rows: 1,
        n_cols: 1,
        cell_offsets: vec![0, n_nodes as u32],
        node_ids: (0..n_nodes as u32).collect(),
    }
}

#[allow(clippy::too_many_arguments)]
fn edge(
    target: u32,
    length_m: f32,
    speed_kmh: f32,
    ascent_m: f32,
    descent_m: f32,
    road_class: u8,
) -> EdgeHot {
    EdgeHot {
        target,
        length_m,
        speed_kmh,
        ascent_m,
        descent_m,
        road_class,
        _pad: [0; 3],
        ch_middle_node: CH_MIDDLE_NODE_NONE,
        geom_offset: 0,
        geom_count: 0,
    }
}

/// Builds an `n_rows x n_cols` 4-neighbour grid graph over a
/// Luxembourg-sized bbox, plus `n_long_range` random long-range links,
/// with ~5% of all links made one-way (reverse dropped), plus a fully
/// disconnected ring "island" of `n_island` extra nodes. No geometry (empty
/// blob, all edges point at it with offset/count 0) and a trivial one-cell
/// snap grid, since neither is exercised by CH correctness.
fn build_graph(seed: u64, spec: &GraphSpec) -> RegionGraphModel {
    let mut rng = Lcg::new(seed);
    let n_grid = spec.n_rows * spec.n_cols;
    let n_nodes = n_grid + spec.n_island;
    let node_id = |row: usize, col: usize| (row * spec.n_cols + col) as u32;

    let mut nodes = Vec::with_capacity(n_nodes);
    for row in 0..spec.n_rows {
        for col in 0..spec.n_cols {
            nodes.push(NodeRecord {
                lat: MIN_LAT + row as f32 * LAT_STEP,
                lon: MIN_LON + col as f32 * LON_STEP,
            });
        }
    }
    // Island nodes sit far outside the grid's bbox; coordinates don't
    // matter for correctness here, only that no edge ever bridges the two.
    for i in 0..spec.n_island.max(1) {
        nodes.push(NodeRecord {
            lat: MIN_LAT - 5.0 - i as f32 * LAT_STEP,
            lon: MIN_LON - 5.0,
        });
    }
    nodes.truncate(n_nodes);

    let mut links: Vec<(u32, u32, f32)> = Vec::new();
    for row in 0..spec.n_rows {
        for col in 0..spec.n_cols {
            if col + 1 < spec.n_cols {
                links.push((node_id(row, col), node_id(row, col + 1), 500.0));
            }
            if row + 1 < spec.n_rows {
                links.push((node_id(row, col), node_id(row + 1, col), 500.0));
            }
        }
    }
    for _ in 0..spec.n_long_range {
        let a = rng.next_range_u32(0, n_grid as u32);
        let mut b = rng.next_range_u32(0, n_grid as u32);
        while b == a {
            b = rng.next_range_u32(0, n_grid as u32);
        }
        let (na, nb) = (nodes[a as usize], nodes[b as usize]);
        let dlat = (na.lat - nb.lat) as f64 * 111_000.0;
        let cos_lat = (MIN_LAT as f64).to_radians().cos();
        let dlon = (na.lon - nb.lon) as f64 * 111_000.0 * cos_lat;
        let dist = (dlat * dlat + dlon * dlon).sqrt().max(500.0) as f32;
        links.push((a, b, dist));
    }
    // Directed ring holds the island strongly connected internally; distinct
    // node ids, so no self-loop even at n_island == 1 (ring of one skipped).
    if spec.n_island >= 2 {
        for i in 0..spec.n_island {
            let a = (n_grid + i) as u32;
            let b = (n_grid + (i + 1) % spec.n_island) as u32;
            links.push((a, b, 500.0));
        }
    }

    let mut pairs: Vec<(u32, EdgeHot)> = Vec::new();
    for &(a, b, dist) in &links {
        let one_way = rng.next_f64() < 0.05;
        pairs.push((
            a,
            edge(
                b,
                dist,
                rng.next_range_f32(30.0, 130.0),
                rng.next_range_f32(0.0, 20.0),
                rng.next_range_f32(0.0, 20.0),
                3,
            ),
        ));
        if !one_way {
            pairs.push((
                b,
                edge(
                    a,
                    dist,
                    rng.next_range_f32(30.0, 130.0),
                    rng.next_range_f32(0.0, 20.0),
                    rng.next_range_f32(0.0, 20.0),
                    3,
                ),
            ));
        }
    }

    pairs.sort_by_key(|(from, _)| *from);
    let mut counts = vec![0u32; n_nodes];
    for (from, _) in &pairs {
        counts[*from as usize] += 1;
    }
    let mut csr_first_edge = vec![0u32; n_nodes + 1];
    for i in 0..n_nodes {
        csr_first_edge[i + 1] = csr_first_edge[i] + counts[i];
    }
    let edges: Vec<EdgeHot> = pairs.into_iter().map(|(_, e)| e).collect();

    RegionGraphModel {
        nodes,
        csr_first_edge,
        edges,
        ch_order: vec![0; n_nodes],
        geometry: Vec::new(),
        snap_grid: trivial_snap_grid(n_nodes),
    }
}

fn write_and_open(model: &RegionGraphModel, region_name: &str) -> (Rpack, tempfile::TempDir) {
    let meta = PackMeta {
        osm_snapshot_epoch: 1_756_252_800,
        region_id: 1,
        region_name: region_name.to_string(),
    };
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.rpack");
    write_rpack(model, &meta, &path).unwrap();
    let pack = Rpack::open(&path).unwrap();
    pack.verify_checksums().unwrap();
    (pack, dir)
}

fn assert_is_permutation(ch_order: &[u32]) {
    let mut sorted = ch_order.to_vec();
    sorted.sort_unstable();
    let expected: Vec<u32> = (0..ch_order.len() as u32).collect();
    assert_eq!(sorted, expected, "ch_order is not a permutation of 0..n");
}

const RELATIVE_TOLERANCE: f64 = 1e-3;

fn assert_costs_close(a: f64, b: f64, context: &str) {
    let scale = a.abs().max(b.abs()).max(1.0);
    assert!(
        (a - b).abs() / scale <= RELATIVE_TOLERANCE,
        "{context}: {a} vs {b} exceeds relative tolerance {RELATIVE_TOLERANCE}"
    );
}

/// Checks `n_pairs` seeded random (s, t) pairs: `Router::p2p` cost vs.
/// `reference::dijkstra_cost` on the base model, Some/None must match and
/// costs must agree within relative tolerance. For `n_path_checks` of those
/// pairs, additionally validates the returned path's structure.
fn assert_p2p_matches_reference(
    router: &Router,
    pack: &Rpack,
    base: &RegionGraphModel,
    seed: u64,
    n_pairs: usize,
    n_path_checks: usize,
) {
    let n = base.nodes.len() as u32;
    let mut rng = Lcg::new(seed);
    for i in 0..n_pairs {
        let s = rng.next_range_u32(0, n);
        let t = rng.next_range_u32(0, n);
        let expected = reference::dijkstra_cost(base, s, t);
        let got = router.p2p(s, t);

        match (expected, &got) {
            (None, None) => {}
            (Some(e), Some(r)) => assert_costs_close(e, r.cost_seconds, &format!("p2p({s},{t})")),
            _ => panic!("p2p({s},{t}) reachability mismatch: reference={expected:?} ch={got:?}"),
        }

        if i < n_path_checks {
            let (Some(expected_cost), Some(route)) = (expected, &got) else {
                continue;
            };
            assert_eq!(route.nodes.first().copied(), Some(s));
            assert_eq!(route.nodes.last().copied(), Some(t));
            assert_eq!(route.nodes.len(), route.edges.len() + 1);

            let mut summed_cost = 0.0f64;
            let mut summed_length = 0.0f64;
            for (idx, &edge_id) in route.edges.iter().enumerate() {
                let e = &pack.edges()[edge_id as usize];
                assert_eq!(
                    e.ch_middle_node, CH_MIDDLE_NODE_NONE,
                    "route edge {edge_id} is a shortcut, not an original edge"
                );
                let from = route.nodes[idx];
                let to = route.nodes[idx + 1];
                assert_eq!(
                    e.target, to,
                    "edge {edge_id} does not lead to the expected node"
                );
                let owns_edge = pack
                    .edge_range(from)
                    .is_some_and(|r| r.contains(&(edge_id as usize)));
                assert!(
                    owns_edge,
                    "edge {edge_id} is not in node {from}'s outgoing range"
                );
                summed_cost += e.length_m as f64 / e.speed_kmh as f64 * 3.6;
                summed_length += e.length_m as f64;
            }
            assert_costs_close(summed_cost, route.cost_seconds, "route per-edge cost sum");
            assert_costs_close(summed_length, route.length_m, "route per-edge length sum");
            assert_costs_close(expected_cost, route.cost_seconds, "route cost vs reference");
        }
    }
}

fn assert_many_to_many_matches_p2p(
    router: &Router,
    seed: u64,
    n: u32,
    n_sources: usize,
    n_targets: usize,
) {
    let mut rng = Lcg::new(seed);
    let sources: Vec<u32> = (0..n_sources).map(|_| rng.next_range_u32(0, n)).collect();
    let targets: Vec<u32> = (0..n_targets).map(|_| rng.next_range_u32(0, n)).collect();

    let table = router.many_to_many(&sources, &targets);
    assert_eq!(table.len(), sources.len());
    for (si, &s) in sources.iter().enumerate() {
        assert_eq!(table[si].len(), targets.len());
        for (ti, &t) in targets.iter().enumerate() {
            let expected = router.p2p(s, t).map(|r| r.cost_seconds);
            let got = table[si][ti];
            match (expected, got) {
                (None, None) => {}
                (Some(e), Some(g)) => assert_costs_close(e, g, &format!("many_to_many[{s}][{t}]")),
                _ => panic!("many_to_many[{s}][{t}] mismatch: p2p={expected:?} m2m={got:?}"),
            }
        }
    }
}

#[test]
fn big_synthetic_graph_ch_matches_reference() {
    let spec = GraphSpec {
        n_rows: 40,
        n_cols: 50,
        n_long_range: 200,
        n_island: 20,
    };
    let base = build_graph(0xA11CE, &spec);
    let n_nodes = base.nodes.len();
    let n_grid = spec.n_rows * spec.n_cols;

    let start = std::time::Instant::now();
    let (contracted, stats) = ch_prepare(&base);
    let elapsed = start.elapsed();
    eprintln!(
        "big graph: {n_nodes} nodes, {} base edges, {} shortcuts added, max_settled={}, contracted in {elapsed:?}",
        base.edges.len(),
        stats.shortcuts_added,
        stats.max_settled,
    );

    assert_is_permutation(&contracted.ch_order);
    assert!(
        stats.shortcuts_added > 0,
        "expected at least one shortcut on a grid this size"
    );

    let (pack, _dir) = write_and_open(&contracted, "grid-big");
    let router = Router::new(&pack);

    assert_p2p_matches_reference(&router, &pack, &base, 0xF00D, 300, 30);

    // Island <-> grid pairs must be unreachable both ways.
    let mut rng = Lcg::new(0x1DEA);
    for _ in 0..10 {
        let grid_node = rng.next_range_u32(0, n_grid as u32);
        let island_node = n_grid as u32 + rng.next_range_u32(0, spec.n_island as u32);
        assert_eq!(
            reference::dijkstra_cost(&base, grid_node, island_node),
            None
        );
        assert_eq!(
            reference::dijkstra_cost(&base, island_node, grid_node),
            None
        );
        assert!(router.p2p(grid_node, island_node).is_none());
        assert!(router.p2p(island_node, grid_node).is_none());
    }

    assert_many_to_many_matches_p2p(&router, 0xB0B0, n_nodes as u32, 25, 25);
}

#[test]
fn smaller_synthetic_graph_ch_matches_reference_with_different_seed() {
    let spec = GraphSpec {
        n_rows: 20,
        n_cols: 16,
        n_long_range: 40,
        n_island: 8,
    };
    let base = build_graph(0x5EED2, &spec);
    let n_nodes = base.nodes.len();

    let (contracted, stats) = ch_prepare(&base);
    assert_is_permutation(&contracted.ch_order);
    assert!(stats.shortcuts_added > 0);

    let (pack, _dir) = write_and_open(&contracted, "grid-small");
    let router = Router::new(&pack);

    assert_p2p_matches_reference(&router, &pack, &base, 0xC0DE2, 300, 30);
    assert_many_to_many_matches_p2p(&router, 0xD00D2, n_nodes as u32, 25, 25);
}

/// Hand-checked: a 5-node line 0->1->2->3->4, each hop 1000m @ 100km/h
/// (36s), through full prepare -> write -> query.
#[test]
fn five_node_line_graph_hand_checked() {
    let nodes: Vec<NodeRecord> = (0..5)
        .map(|i| NodeRecord {
            lat: MIN_LAT + i as f32 * LAT_STEP,
            lon: MIN_LON,
        })
        .collect();
    let mut pairs: Vec<(u32, EdgeHot)> = Vec::new();
    for i in 0..4u32 {
        pairs.push((i, edge(i + 1, 1000.0, 100.0, 0.0, 0.0, 0)));
    }
    let n_nodes = nodes.len();
    let mut counts = vec![0u32; n_nodes];
    for (from, _) in &pairs {
        counts[*from as usize] += 1;
    }
    let mut csr_first_edge = vec![0u32; n_nodes + 1];
    for i in 0..n_nodes {
        csr_first_edge[i + 1] = csr_first_edge[i] + counts[i];
    }
    let base = RegionGraphModel {
        nodes,
        csr_first_edge,
        edges: pairs.into_iter().map(|(_, e)| e).collect(),
        ch_order: vec![0; n_nodes],
        geometry: Vec::new(),
        snap_grid: trivial_snap_grid(n_nodes),
    };

    let (contracted, _stats) = ch_prepare(&base);
    assert_is_permutation(&contracted.ch_order);
    let (pack, _dir) = write_and_open(&contracted, "line5");
    let router = Router::new(&pack);

    let route = router.p2p(0, 4).expect("0 -> 4 is connected");
    assert_costs_close(route.cost_seconds, 144.0, "5-node line 0->4");
    assert_costs_close(route.length_m, 4000.0, "5-node line length");
    assert_eq!(route.nodes, vec![0, 1, 2, 3, 4]);

    // No reverse edges exist.
    assert!(router.p2p(4, 0).is_none());
}

/// Hand-checked: a 4-node diamond, 0->1->3 (2x36s=72s) vs 0->2->3
/// (2x72s=144s). The cheap path must win, and the redundant shortcut that
/// contracting the expensive-path node would otherwise add must be skipped
/// (a cheaper 0->3 edge/shortcut already exists).
#[test]
fn four_node_diamond_hand_checked() {
    let nodes = vec![
        NodeRecord {
            lat: MIN_LAT,
            lon: MIN_LON,
        },
        NodeRecord {
            lat: MIN_LAT + LAT_STEP,
            lon: MIN_LON,
        },
        NodeRecord {
            lat: MIN_LAT,
            lon: MIN_LON + LON_STEP,
        },
        NodeRecord {
            lat: MIN_LAT + LAT_STEP,
            lon: MIN_LON + LON_STEP,
        },
    ];
    let mut pairs: Vec<(u32, EdgeHot)> = vec![
        (0, edge(1, 1000.0, 100.0, 0.0, 0.0, 0)), // 0->1: 36s
        (1, edge(3, 1000.0, 100.0, 0.0, 0.0, 0)), // 1->3: 36s
        (0, edge(2, 2000.0, 100.0, 0.0, 0.0, 0)), // 0->2: 72s
        (2, edge(3, 2000.0, 100.0, 0.0, 0.0, 0)), // 2->3: 72s
    ];
    pairs.sort_by_key(|(from, _)| *from);
    let n_nodes = nodes.len();
    let mut counts = vec![0u32; n_nodes];
    for (from, _) in &pairs {
        counts[*from as usize] += 1;
    }
    let mut csr_first_edge = vec![0u32; n_nodes + 1];
    for i in 0..n_nodes {
        csr_first_edge[i + 1] = csr_first_edge[i] + counts[i];
    }
    let base = RegionGraphModel {
        nodes,
        csr_first_edge,
        edges: pairs.into_iter().map(|(_, e)| e).collect(),
        ch_order: vec![0; n_nodes],
        geometry: Vec::new(),
        snap_grid: trivial_snap_grid(n_nodes),
    };

    let (contracted, _stats) = ch_prepare(&base);
    assert_is_permutation(&contracted.ch_order);
    let (pack, _dir) = write_and_open(&contracted, "diamond4");
    let router = Router::new(&pack);

    let route = router.p2p(0, 3).expect("0 -> 3 is connected");
    assert_costs_close(route.cost_seconds, 72.0, "4-node diamond 0->3");
    assert_eq!(route.nodes, vec![0, 1, 3]);
}
