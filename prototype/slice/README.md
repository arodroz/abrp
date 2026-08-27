THROWAWAY PERFORMANCE PROTOTYPE — measures feasibility of on-device Rust routing (ADR 0001/0003/0006); not production code, no error handling beyond "doesn't crash", do not build on top of it.

# planner

Vertical-slice prototype: OSM PBF + charger feeds -> a contraction-hierarchy pack ->
a time-optimal route with a simplified label-setting charging-stop optimiser, exposed
to Swift via UniFFI.

## Layout

- `src/lib.rs` — energy model, CH leg queries, the label-setting optimiser, and the
  UniFFI surface (`Planner::new(pack_dir)`, `Planner::plan_json(request_json)`).
- `src/bin/build_pack.rs` — OSM PBF + charger feeds -> pack directory.
- `src/bin/plan_cli.rs` — pack dir + origin/dest -> one plan; prints the response
  JSON on stdout, pack-load time and peak RSS (via `getrusage`) on stderr.

## Building a pack

```
cargo build --release
target/release/build_pack --pbf <region.osm.pbf> --out <pack-dir> \
  [--ndw <ndw_chargers.json.gz>] [--road <road_chargers.json>] [--kml <chargy.kml>]
```

Charger args are independent and optional; pass whichever feeds cover the region.

## Running a plan

```
target/release/plan_cli --pack <pack-dir> --origin lat,lon --dest lat,lon \
  [--depart-soc 0.9] [--arrival-min-soc 0.1] [--charger-arrival-min-soc 0.1] \
  [--charger-max-soc 0.8] [--stops-bias 1.0]
```

## Physics simplifications (this prototype only)

- **Flat grade everywhere.** No DEM/elevation join; ADR 0003's grade term is always
  zero. Real corridors (Ardennes, Luxembourg's plateau edges) will be flattered.
- **No temperature/wind.** Fixed warm-battery charging curve, no `P_hvac`, no cold
  penalty on `η_drive`.
- Aux load is added to `P` *before* dividing by `η_drive` (task-spec formula), not
  added after like ADR 0003's literal formula — a deliberate simplification for this
  slice, not a correction of the ADR.
- Charging curve is a hand-digitised 9-point piecewise-linear table from
  `docs/research/ioniq5-energy-model.md` §4.1 (peak ~220-225kW low/mid SoC, thermal
  dip ~52-54%, hard taper above 80%), not the full published per-percent table.

## Optimiser simplifications vs ADR 0006

- Fixed 10km corridor radius (task spec), no 3km→10km widening.
- Relaxation on infeasibility is two-step (charger-arrival-min-SoC → 0, then
  destination-arrival-min-SoC → 0) rather than the ADR's three-step order with
  corridor widening as a last resort.
- SoC dominance buckets are 2% wide and keep only the fastest label per bucket,
  matching the ADR; "just enough" depart targets are computed per outgoing edge
  (there's no single global "just enough" once you don't know the next hop yet).
- `soc_curve` samples SoC at the next route vertex at-or-after each ~2km mark
  (nearest-following, not interpolated) assuming uniform energy density along each
  edge.
- Candidates within ~300m of each other are deduped (highest `power_kw` wins) before
  the search graph is built — the charger feeds carry direction-pair duplicates
  (opposite carriageways of the same physical site).
- Candidate→candidate fan-out is forward-pruned: edges are only considered to
  candidates more than 20km further along the route, capped at the nearest 30 such
  candidates per node, to keep CH-query count roughly linear instead of O(n²) at
  corridor scale. `query_leg` results are memoized per `(from,to)` node pair.
- A depart target within 2% of the arrival SoC is dropped rather than clamped to
  zero-charge, so a charger can never appear as a free, zero-penalty "stop" that just
  happens to sit on the optimal path (see `MIN_CHARGE_SOC` in `run_label_search`) —
  any genuine bypass of that candidate is already covered by direct edges to its
  neighbours.

## Pack format (v2 — changed for corridor-scale memory)

The corridor (BE+NL+LU) pack surfaced 1.47GB peak RSS in `plan_cli`, driven mostly by
fully deserializing a geometry array of millions of points and a `(from,to) -> edge`
`HashMap` with millions of entries. Fixed by:

- `edges.bin` split into `edges_meta.bin` (bincode `EdgesMetaFile { edges, from_start }`,
  `edges` sorted by `(from, to)`) and `geometry.bin` (a raw, unwrapped little-endian f32
  byte blob — no bincode framing, so it can be `mmap`ped directly). `EdgeMeta.geom_offset`
  counts *points* into `geometry.bin`, not bytes (multiply by 8).
- Edge lookup by `(from, to)` is now a CSR row (`from_start[n]..from_start[n+1]`) plus a
  binary search on `to`, instead of a HashMap — this is the "acceptable but optional" CSR
  variant, worth doing since the HashMap was a meaningful share of the 1.47GB.
- `Planner::new` streams bincode via `BufReader` + `deserialize_from` instead of
  `fs::read` + `deserialize`, avoiding a transient double-buffer peak on the larger files.
- Route/candidate/soc_curve code only ever calls `edge_geometry()` for edges actually on
  a path, so geometry pages in lazily instead of being resident for the whole graph.
- `graph.bin` (the CH `FastGraph`) and `nodes.bin` are unchanged and still fully resident
  — that's an accepted tradeoff, not a gap.

This is a breaking pack format change: repacks are required (`pack-lu` and
`pack-corridor` were both rebuilt with the new `build_pack`).

## Graph-build simplifications

- Turn restrictions ignored (per ADR 0001).
- Node coordinates for **all** nodes referenced by drivable ways are loaded into one
  `HashMap` in memory (two full PBF passes). Fine for Luxembourg (47MB); would not
  scale to a Belgium+Netherlands corridor build without streaming.
- After building the directed graph, only the **largest strongly connected
  component** is kept. Real extracts have small directed dead-end fragments (private
  driveways, tile-boundary clipping) that a CH query can never route across; keeping
  them causes silent "no route found" for otherwise-reasonable coordinates. This
  wasn't in the original spec — it's a necessary addition, not a shortcut.
- Nearest-node snapping is a plain linear scan (as the spec allows), restricted to
  nodes that actually have an outgoing edge (for the origin/candidates) or an
  incoming edge (for the destination/candidates), for the same directed-dead-end
  reason above.
- `chargers.geojson` and `chargers.bin` only contain chargers parsed from whichever
  `--ndw`/`--road`/`--kml` args were passed to this `build_pack` invocation, not a
  fixed "all three" set.
