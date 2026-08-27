# Turn-restriction routing: RoutingKit importer + arc-expanded CH vs GraphHopper edge-based CH

Research note for issue #17. Date: 2026-08-27. Terms follow `CONTEXT.md` (Routing Engine, Leg, Plan, Region Pack). Input baselines: ADR 0001 (`docs/adr/0001-on-device-rust-routing.md`), the vertical slice (#15, `prototype/RESULTS.md` on `prototype/vertical-slice`), and `docs/research/on-device-routing.md` (#3).

## TL;DR

- **Recommendation: port RoutingKit's importer semantics and build an arc-expanded graph fed to the unchanged `fast_paths` CH kernel** (Option A). Do **not** port GraphHopper's edge-based CH (Option B).
- The decisive facts: (a) RoutingKit's CH knows nothing about turns — the importer only emits `forbidden_turn_from_arc/to_arc` pairs and the arc-expanded graph is left to the user ([issue #23](https://github.com/RoutingKit/RoutingKit/issues/23)), which is exactly the transformation we control; (b) **OSRM is the production proof of Option A at planet scale** — its `.osrm` graph is edge-expanded by construction, restrictions are compiled in by deleting turn edges, and plain CH/MLD runs on top ([OSRM wiki](https://github.com/Project-OSRM/osrm-backend/wiki/Graph-representation)); (c) GraphHopper's edge-based CH took its author (easbar, also the author of `fast_paths`) "over a year", with 5–8× shortcut explosion and 5× preparation time, and none of it exists in Rust ([PR #1247](https://github.com/graphhopper/graphhopper/pull/1247), [deploy.md](https://github.com/graphhopper/graphhopper/blob/master/docs/core/deploy.md)).
- **Estimated cost on the measured corridor** (LU+BE+NL: 1,552,569 nodes / 3,400,642 directed arcs, 694 MB peak on iPhone 15 Pro): expanded graph ≈ 3.40 M nodes / ~6.5–8 M turn-arcs (×2.2 nodes-as-arcs, ~×2–2.4 arcs); CH prepare 16.4 s → est. 1–3 min on the M4 (batch pipeline, irrelevant); query ×2–3; resident graph structures ×2.5–3, i.e. roughly **+200–300 MB → ~850–950 MB peak with the basemap, still under the <1 GB bar** (and the 786 MB PMTiles mmap is evictable; steady state was 613 MB).
- Rust ecosystem re-check (2026-08-27): still no crate doing OSM → turn-aware CH end-to-end. New since #3: `routx` (pure-Rust OSM importer that parses restrictions, A*-only), `routingkit-cch` (bindings, no turns), and `rust_road_router`'s reusable `line_graph()` confirming the expansion transform is small. Porting remains our work; the assembly is now cheaper.
- **Validation:** run Valhalla in Docker on the Luxembourg PBF as a legality oracle over all **1,649** `type=restriction` relations in LU (Overpass count, 2026-08-27) plus 1,000 random O-D pairs; pass = zero restricted-pair traversals on both corpora and no suspicious travel-time wins vs Valhalla. Detailed protocol in §7.

---

## 1. Baseline the estimates multiply (vertical slice, #15)

From `prototype/RESULTS.md` (branch `prototype/vertical-slice`), measured:

| Quantity | Value |
|---|---|
| Corridor graph (LU+BE+NL) | 1,552,569 junction nodes / 3,400,642 directed edges |
| CH prepare (`fast_paths`, M4) | 16.4 s |
| Plan latency (iPhone 15 Pro) | 456 ms (route 41 ms worst-case + optimiser 412 ms over ~4.1k memoized leg queries) |
| Peak resident | 694 MB (incl. 786 MB PMTiles basemap, mmap'd); 613 MB steady; CH graph fully resident |
| Bar (ADR 0001) | resident graph < 1 GB with the turn-aware penalty included |

Derived: average out-degree A/N = 3,400,642 / 1,552,569 ≈ **2.19**.

## 2. Option A: RoutingKit importer semantics + arc-expanded CH

### What RoutingKit actually does (source-verified)

- **Importer.** `decode_osm_car_turn_restrictions` (`src/osm_profile.cpp:715–856`) parses `type=restriction` relations: only the plain `restriction` tag (no `restriction:motorcar`, no `except`, no conditionals); exactly 8 values (`no_`/`only_` × `left_turn|right_turn|straight_on|u_turn`); `no_entry`/`no_exit` are logged and dropped; **via-way restrictions are rejected** — only via-node (or a via inferred when the ways cross exactly once). Prohibitive restrictions with several `from`/`to` members emit the cross product; mandatory need exactly 1×1. ([osm_profile.cpp](https://github.com/RoutingKit/RoutingKit/blob/master/src/osm_profile.cpp), [doc/OpenStreetMap.md](https://github.com/RoutingKit/RoutingKit/blob/master/doc/OpenStreetMap.md))
- **Arc matching.** `src/osm_graph_builder.cpp:375–830`: localize way/node IDs; infer missing via nodes; match from/to ways to the via node's in/out arcs; when a way passes through the via node (two candidate arcs per direction), disambiguate by `atan2` bearing sectors computed from each arc's first/last modelling node — which is why the loader forces `OSMRoadGeometry::first_and_last` when a decoder is supplied. Mandatory `only_X` is rewritten as: forbid `from` → every out-arc except `to`. Output: sorted, deduped parallel arrays `forbidden_turn_from_arc[i]` / `forbidden_turn_to_arc[i]` of local arc IDs ([osm_graph_builder.h](https://github.com/RoutingKit/RoutingKit/blob/master/include/routingkit/osm_graph_builder.h), [osm_graph_builder.cpp](https://github.com/RoutingKit/RoutingKit/blob/master/src/osm_graph_builder.cpp)).
- **The CH consumes none of it.** `contraction_hierarchy.h` has zero turn awareness; the author: "If you have a data source that provides a turn-expanded graph (i.e., nodes are roads, arcs are turns) then it will work. The current OSM parser unfortunately does not provide this feature" ([issue #23](https://github.com/RoutingKit/RoutingKit/issues/23)); a user (CartoType) reports building the edge-expanded graph themselves and running plain RoutingKit CH on it "fine" ([issue #71](https://github.com/RoutingKit/RoutingKit/issues/71)).

### The arc-expanded transform

Each directed arc of the road graph becomes a node; each *permitted* transition at a junction from in-arc *a* to out-arc *b* becomes an edge weighted `cost(b)` (+ optional turn cost); forbidden pairs and (by policy) u-turns are simply omitted. This is the standard "expanded graph" of the literature ([Buchhold et al., ATMOS 2020](https://drops.dagstuhl.de/entities/document/10.4230/OASIcs.ATMOS.2020.9)), the transform `rust_road_router` ships as `line_graph()` ([engine/src/datastr/graph.rs](https://github.com/kit-algo/rust_road_router)), and the representation OSRM uses for *everything*: "Graph *nodes* represent a specific direction (forward or backward) of an OSM segment… OSRM will also remove turns that are prohibited by turn restrictions" ([OSRM wiki: Graph representation](https://github.com/Project-OSRM/osrm-backend/wiki/Graph-representation)).

Two `fast_paths` limitations that block turn work on the node-based graph *disappear* in the expanded graph: parallel edges (collapsed to min weight) cannot occur — the turn a→b is unique — and loop edges (ignored) do not arise since a u-turn is a→reverse(a), two distinct expanded nodes ([fast_paths README](https://github.com/easbar/fast_paths)).

### Measured cost of expansion in the literature

- ATMOS 2020 (KIT, CCH with turn costs): the expanded network is "only about 3× larger"; naive preprocessing/customization slows up to ~10×, engineered down to ~3× ([paper](https://drops.dagstuhl.de/entities/document/10.4230/OASIcs.ATMOS.2020.9)).
- Bast et al. 2016 Table 2 (Western Europe, 100 s u-turn cost at *every* junction — worse than our restrictions-only model): arc-based expanded CH 3.14 GiB vs 0.60 GiB node-based incl. unpacking (×5.2), query 0.20–0.30 ms vs 0.11 ms (×2–3) ([arXiv:1504.05140](https://arxiv.org/abs/1504.05140)). Our setup (forbid u-turns, zero cost on permitted turns) sits below this because no turn-cost differentiation inflates the hierarchy.

### Porting effort

Faithful Rust port of the importer ≈ 350 relevant C++ lines: relation decode (~80 LoC), ID localization (~30), via inference / go-straight expansion (~100), arc matching + angular disambiguation (~150, keep first/last shape point per arc, copy the bearing math verbatim for parity), sort+dedup (~20). Estimate **2–4 days** with fixture tests against RoutingKit's own output on a small extract, plus **~1 day** for the expansion + forbidden-pair dropping. Snapping changes from node to arc: snap to an edge, enter/leave via its one or two directed arcs — `fast_paths` supports this directly with multi-source/multi-target queries.

### Deliberate deviations from RoutingKit to adopt

1. Also read `restriction:motorcar` and honor `except=*` (Valhalla and OSRM both do; RoutingKit ignores them).
2. Support `no_entry`/`no_exit` by cross-product expansion (OSRM semantics, [PR #5988](https://github.com/Project-OSRM/osrm-backend/pull/5988)).
3. Defer via-way restrictions, but note the proven upgrade path inside Option A: OSRM implements multi-via-way by duplicating expanded nodes along the via path ([PR #5907](https://github.com/Project-OSRM/osrm-backend/pull/5907)). Count them per region during validation (§7) to keep the deferral honest.
4. Log-and-count dropped restrictions (conditionals, ambiguous matches) into the pack build report.

## 3. Option B: GraphHopper edge-based CH

Source-verified design ([PR #1247](https://github.com/graphhopper/graphhopper/pull/1247), merged 2019; [docs/core/turn-restrictions.md](https://github.com/graphhopper/graphhopper/blob/master/docs/core/turn-restrictions.md)):

- The node-based graph is kept; nodes are contracted, but witness searches and query states are **directed edges ("edge keys")**. Shortcuts stay node-to-node but grow from 20 to 28 bytes, carrying the first/last original edge keys so turn costs at shortcut endpoints can be evaluated ([CHStorage.java](https://github.com/graphhopper/graphhopper/blob/master/core/src/main/java/com/graphhopper/storage/CHStorage.java)).
- Turn costs live in a sparse `TurnCostStorage`: 16-byte entries keyed `(fromEdge, viaNode, toEdge)`, linked-list per via node; a restriction is an infinite cost ([TurnCostStorage.java](https://github.com/graphhopper/graphhopper/blob/master/core/src/main/java/com/graphhopper/storage/TurnCostStorage.java), [DefaultTurnCostProvider.java](https://github.com/graphhopper/graphhopper/blob/master/core/src/main/java/com/graphhopper/routing/weighting/DefaultTurnCostProvider.java)).
- U-turns are forbidden by default (`INFINITE_U_TURN_COSTS = -1`), configurable to a finite seconds cost ([TurnCostsConfig.java](https://github.com/graphhopper/graphhopper/blob/master/web-api/src/main/java/com/graphhopper/util/TurnCostsConfig.java)).
- Via-way restrictions are decomposed into plain triples by adding an **artificial copy of the via edge** ([PR #2689](https://github.com/graphhopper/graphhopper/pull/2689), GH 7.0; multi-via and overlap handling in [PR #3030](https://github.com/graphhopper/graphhopper/pull/3030), [PR #3100](https://github.com/graphhopper/graphhopper/pull/3100)).

Measured impact (author's numbers): queries with turn costs "about two to three times slower" ([GH blog 2020](https://www.graphhopper.com/blog/2020/07/08/turn-restriction-support-for-graphhoppers-directions-api/)); Berlin shortcuts ~84k node-based → ~430–500k edge-based (×5–8), Berlin preparation ~4 min after a year of optimization (was 15+ min; at one point "40 min vs 8 s") ([PR #1247 discussion](https://github.com/graphhopper/graphhopper/pull/1247)); planet-wide CH preparation with turn costs 25 h / ~120 GB RAM vs 5 h without ([deploy.md](https://github.com/graphhopper/graphhopper/blob/master/docs/core/deploy.md)).

Why not port it:

1. **The witness search is the hard part.** The "turn replacement" witness algorithm and shortcut-explosion fight consumed easbar "over a year" in a codebase he knew ([PR #1247](https://github.com/graphhopper/graphhopper/pull/1247), [GH 0.12 blog](https://www.graphhopper.com/blog/2019/03/26/graphhopper-routing-engine-0-12-released/)). `fast_paths` (same author) has no trace of it — no issue, no PR, no stated plan; the crate is dormant since 2024-05 ([issues](https://github.com/easbar/fast_paths/issues?q=turn)). We would be re-deriving the trickiest part from a PR thread, alone.
2. **It does not buy memory.** The sparse turn table is tiny (LU has 1,649 restrictions; ~16 B each), but the 5–8× shortcut explosion at 28 B/record lands in the same ballpark as — or above — the expanded graph's ~3× total growth. GraphHopper chose this design to keep one graph serving both node- and edge-based *per-request* traversal and live turn-cost flexibility ([GH 0.12 blog](https://www.graphhopper.com/blog/2019/03/26/graphhopper-routing-engine-0-12-released/)) — needs a static Region Pack does not have.
3. Its one real functional edge — via-way restrictions today — is reachable inside Option A via OSRM-style node duplication when we need it.

## 4. How the comparison engines do it (context)

- **Valhalla** does not use CH at all: tiled hierarchical graph, dynamic run-time costing, bidirectional A* applying restrictions at expansion time; simple restrictions are a bitmask on the directed edge, complex ones variable-length records stored twice (forward+reverse indexed) per tile, with `IsBridgingEdgeRestricted()` validating the meeting point of the two searches ([directededge.h](https://github.com/valhalla/valhalla/blob/master/valhalla/baldr/directededge.h), [complexrestriction.h](https://github.com/valhalla/valhalla/blob/master/valhalla/baldr/complexrestriction.h), [graphtile.h](https://github.com/valhalla/valhalla/blob/master/valhalla/baldr/graphtile.h), [PR #2766](https://github.com/valhalla/valhalla/pull/2766), [why-tiles](https://valhalla.github.io/valhalla/concepts/why-tiles/), [dynamic costing](https://valhalla.github.io/valhalla/concepts/costing/dynamic-costing/)). Rich semantics (modes, time-domain conditionals) make it the right *oracle*, not the right architecture for a <1 s on-device CH.
- **OSRM**: edge-expanded graph from `osrm-extract` onward; restrictions deleted at graph build; turn penalties (7.5 s turn, 20 s u-turn, angle sigmoid) baked into expanded-edge weights from `car.lua`; CH (`osrm-contract`) or MLD runs on the expanded graph unchanged ([wiki](https://github.com/Project-OSRM/osrm-backend/wiki/Graph-representation), [car.lua](https://github.com/Project-OSRM/osrm-backend/blob/master/profiles/car.lua)). This is Option A, in production, at planet scale.

## 5. Rust ecosystem re-check (changes since #3, verified 2026-08-27)

| Project | Change | Relevance |
|---|---|---|
| [`fast_paths`](https://github.com/easbar/fast_paths) | none; dormant since 2024-05, no turn/edge-based issues | stays our CH kernel, unchanged, under Option A |
| [`rust_road_router`](https://github.com/kit-algo/rust_road_router) | correction: `line_graph()` turn-expansion *is* in the reusable engine lib; `cchpp/src/bin/turn_expand_osm.rs` consumes RoutingKit-format forbidden-pair files | reference implementation for our expansion step; not a dependency (unpublished, research-frozen) |
| [`routx`](https://github.com/mkuranowski/routx) v1.1.0 (2026-06) | **new**: pure-Rust OSM importer parsing turn restrictions, MIT — but A*-only, no CH | cross-check corpus for our importer's parse step |
| [`routingkit-cch`](https://github.com/HellOwhatAs/RoutingKit-cch) v0.1.4 (2026-07) | **new**: Rust bindings to C++ RoutingKit CCH; no turns, no OSM | not useful directly |
| [`osrm-binding`](https://github.com/blob-map/osrm-binding) v1.0.0 (2026-08) | **new**: in-process C++ OSRM v6 via FFI, turn-aware | escape hatch if the pure-Rust path fails; rejected for now (C++ toolchain in the iOS build, OSRM data pipeline) |
| [`cch`](https://github.com/Rodeapps/cch), [`cch-rs`](https://github.com/wmsnp/cch-rs), `osm4routing` | no turn support; osm4routing still ignores restriction relations | unchanged |

Conclusion unchanged: **no crate goes OSM → turn-aware CH end-to-end; we port it ourselves**, and Option A is the smallest port.

## 6. Estimated impact on the corridor graph

Corridor: N = 1,552,569, A = 3,400,642, avg out-degree 2.19. Expanded nodes = A = **3.40 M** (×2.19). Expanded arcs = Σ_v in(v)·out(v); Cauchy–Schwarz lower bound A²/N ≈ 7.45 M, minus forbidden u-turn pairs (≈ one per bidirectional arc) → **≈ 6.5–8 M turn-arcs** (~×2–2.4 the baseline arc count). *(Exact Σ in·out is one afternoon of pipeline work once the importer branch exists; treat 6.5–8 M as the planning number.)*

| Metric | Baseline (measured, #15) | Option A est. | Option B est. | Basis |
|---|---|---|---|---|
| Graph fed to CH | 1.55 M n / 3.40 M a | 3.40 M n / ~6.5–8 M a | unchanged + turn table (~50–100k triples Benelux, 1–2 MB) | §2, §3 |
| CH prepare (M4) | 16.4 s | ~1–3 min (×4–10) | ~1.5–8 min (×5 planet-wide ratio; witness search dominates) | ATMOS ~10× naive; GH deploy.md 5× |
| Shortcut growth | 1× | ~×2–3 (on a ×2.2 graph) | ×5–8, records 20→28 B (×7–11 storage) | Bast T2; PR #1247 Berlin |
| Query (worst leg, iPhone) | 41 ms | ~80–120 ms (×2–3) | ~80–120 ms (×2–3) | Bast T2; GH blog 2020 |
| Optimiser (4.1k leg queries) | 412 ms | ~0.8–1.2 s naive — mitigate with bucket-based many-to-many (already planned in RESULTS.md finding 1) | same | ×2–3 on queries |
| Resident graph structures | part of 694 MB peak / 613 MB steady | ×2.5–3 → **+200–300 MB**; ~850–950 MB peak incl. basemap | comparable or worse (shortcut explosion) | Bast T2 ×5.2 upper bound is for 100 s u-turn costs everywhere |
| <1 GB bar | passes | **passes, tight**; PMTiles mmap is evictable, and per-country packs (LU or BE alone) sit far below | passes only if shortcut explosion lands at the low end | §1 |

Prepare time stays a batch-pipeline concern (ADR 0001 builds packs off-device); even ×10 is irrelevant. The two live risks are the optimiser budget (mitigation exists and was already flagged) and peak memory (mitigations: per-country packs, mmap'd CH arrays, drop the node-based graph from the pack once turn-aware is default).

## 7. Validation against Valhalla on Luxembourg intersections

Valhalla is the oracle for *legality*, not for exact paths or times (different speed model, dynamic costing, richer restriction semantics — §4).

**Setup** ([docker/README.md](https://github.com/valhalla/valhalla/blob/master/docker/README.md); the former gis-ops image is archived upstream):

```bash
mkdir custom_files
wget -O custom_files/luxembourg-latest.osm.pbf \
  https://download.geofabrik.de/europe/luxembourg-latest.osm.pbf   # ~45 MB
docker run -dt --name valhalla -p 8002:8002 -v $PWD/custom_files:/custom_files \
  ghcr.io/valhalla/valhalla-scripted:latest    # builds tiles on first start, minutes
curl "localhost:8002/route?json={\"locations\":[{\"lat\":49.611,\"lon\":6.13},{\"lat\":49.60,\"lon\":6.14}],\"costing\":\"auto\"}"
```

**Test corpus** — restrictions via Overpass (count on 2026-08-27: **1,649** relations; small enough to test *all*, no sampling):

```
[out:json][timeout:50];
area["ISO3166-1"="LU"][admin_level=2]->.lu;
relation["type"="restriction"](area.lu);
out body; >; out skel qt;
```

Bucket every relation by kind: via-node `no_*`, via-node `only_*`, `no_entry`/`no_exit`, via-way, conditional/`except`. The last two buckets are the *documented exclusion list* for the first turn-aware pack (with counts — this also sizes the via-way deferral of §2).

**Three checks, all machine-run:**

1. **Self-check (no oracle):** for every supported restriction, synthesize a targeted O-D pair — origin ~300 m back along the from-way, destination ~300 m along the to-way — so the restricted turn is the naive shortest path. Assert our route never traverses a forbidden (from_arc → to_arc) pair, checked against a restriction set parsed *independently* of our importer (Overpass JSON directly, or `routx`'s parser as a second opinion) so importer bugs cannot self-certify. Also run each `only_*` case and assert the mandated continuation is taken or legally avoided.
2. **Differential vs Valhalla on the targeted pairs:** request the same O-D from Valhalla (`costing=auto`); assert (a) Valhalla also avoids the turn (confirms the case is well-formed), and (b) our detour time ≤ 1.25 × Valhalla's detour time (catches over-blocking: an importer that forbids too much produces absurd detours).
3. **Corpus check:** 1,000 uniformly random LU O-D pairs. Assert zero restricted-pair traversals on every route, and flag any pair where our travel time beats Valhalla's by >15 % as a suspected illegal shortcut for manual review (speed models differ; the threshold is a tripwire, not a spec).

**Pass bar:** 100 % on checks 1 and 2 for supported buckets; 0 violations and 0 unexplained flags on check 3; exclusion-list counts published in the pack build report. Precedent for cross-engine comparison harnesses: [urschrei/router_comparison](https://github.com/urschrei/router_comparison), [Telenav osrm-vs-valhalla](https://github.com/Telenav/open-source-spec/blob/master/osrm/doc/osrm-vs-valhalla.md); per-restriction micro-map fixtures as in Valhalla's gurka tests (`test/gurka/test_simple_restrictions.cc`) are the model for our importer unit tests.

## 8. Open questions carried forward

1. Measure the real Σ in·out and post-CH sizes on the corridor as the first commit of the implementation ticket; replace §6's ranges with measurements.
2. Decide whether the pack keeps the node-based graph alongside the expanded one (fast candidate-matrix queries for the optimiser vs pack size) or whether bucket-based many-to-many on the expanded CH alone holds the 1 s Plan bar.
3. Via-way restrictions: count per region (validation §7 yields LU's); schedule the OSRM-style node-duplication upgrade when a region's count or a navigation defect justifies it.
4. Turn *costs* (seconds per left turn etc., OSRM-style angle sigmoid) are cheap to add as expanded-edge weights later; out of scope for restriction correctness.
