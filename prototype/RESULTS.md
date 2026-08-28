# Vertical slice benchmark — results (wayfinder #15)

THROWAWAY PROTOTYPE. This directory exists to answer one question: does the
performance hypothesis behind ADR 0001/0002/0006 hold on a real iPhone?
It is not production code and lives only on this branch.

## Answer: yes. All three bars pass.

Plan: Luxembourg City (49.6116, 6.1319) → Amsterdam (52.3676, 4.9041),
412.45 km, Ioniq 5 (2022), depart 90% SoC, arrive ≥10%.
Device: iPhone 15 Pro (iPhone16,1), iOS 26.6. Release builds.

| Bar | Target | Measured on-device |
|---|---|---|
| Plan latency | < 1 s | **456 ms** (route 41 ms + optimiser 412 ms; planner init 516 ms) |
| Resident memory | < 1 GB | **694 MB peak**, 613 MB steady (phys_footprint, includes 786 MB PMTiles map + full pack) |
| Frame rate | 120 fps sustained | ~~119.79 fps avg, 111.98 min~~ measured over a basemap that never rendered (see caveat below) — honest re-measure: **118.63 fps avg, 111.98 min** |

**Frame-rate caveat and re-measure (wayfinder #26, 2026-08-28).** The
original flyover ran over a black void: the generated style had empty
`paint` on every layer (MapLibre defaults unset colors to black) and the
PMTiles URL wrapped a bare file path (CFNetwork rejects it), so no tile
was ever decoded or drawn. Both bugs were found and fixed in the planner-UI
prototype (#23). Re-run on the same iPhone 15 Pro with the fixed style
actually rendering (same route, `-benchmark-flyover` launch argument,
45 s flyover): **fps_avg 118.63, fps_min 111.98, mem_peak 859.7 MB**.
The min is still the first tile-load second; steady state oscillates
116–120 fps with real tile decode in the frame budget. Memory peak rose
694 → 860 MB (tile textures), still under the 1 GB bar. Verdict: both
bars still pass; rendering the world costs ~1.2 fps on average.

Plan result: 1 charging stop (Bloemenkwekerij Scheers, 360 kW, arrive 12% →
depart 60%, 10.6 min), total 4 h 00 min, arrival SoC 14%.

Mac (M4) reference for the same plan: 193 ms total, 665 MB peak RSS.

## What the corridor is

LU + BE + NL Geofabrik extracts merged (osmium), 2.0 GB PBF →
1,552,569 junction nodes / 3,400,642 directed edges, fast_paths CH
prepare 16.4 s on the M4. 1,549 DC ≥50 kW CCS chargers parsed from the
three keyless national feeds (NDW OCPI, Road OCPI, Chargy KML).
Basemap: Protomaps daily build, corridor bbox z14 extract (786 MB),
official light flavor (69 layers), rendered from a local PMTiles file.

## Findings that matter for the production build

1. **The optimiser, not the route query, is the latency budget.** CH
   point-to-point is 0.4–41 ms; the naive candidate×candidate leg matrix
   (28k queries) cost 4.6 s until pruned (dedup ~300 m, forward window,
   fan-out cap 30, memoization → 4.1k queries, 412 ms on-device). The
   RoutingKit-style port should include bucket-based many-to-many.
2. **`nearest_node` was the hidden 3.3 s.** A linear scan over 1.55 M
   nodes per candidate dominated everything until replaced with a 0.1°
   grid index. Snap indexing belongs in the pack format.
3. **Memory is serialization hygiene, not graph size.** Streaming bincode
   + mmap'd geometry (raw f32 blob) + CSR edge lookup cut peak RSS from
   1.47 GB to 665 MB with the CH graph fully resident.
4. **Zero-charge "pass-through" labels must be excluded** from stop
   expansion or they surface as phantom stops.
5. Elevation was flat (grade 0) throughout — energy numbers are corridor-
   plausible but uncalibrated; irrelevant to the three bars.
6. MapLibre iOS NSExpression does not support `CONCAT`; precompute label
   strings into GeoJSON properties.

## Reproduce

```
# data (all keyless): Geofabrik lu/be/nl PBFs, osmium merge → corridor.osm.pbf
# chargers: NDW charging_point_locations_ocpi.json.gz, Road locations.json, Chargy KML
# basemap: pmtiles extract https://build.protomaps.com/<daily>.pmtiles --bbox=4.2,49.3,7.1,52.7 --maxzoom=14
cd prototype/slice
cargo build --release
target/release/build_pack --pbf corridor.osm.pbf --out pack-corridor \
  --ndw ndw_chargers.json.gz --road road_chargers.json --kml chargy.kml
target/release/plan_cli --pack pack-corridor --origin 49.6116,6.1319 --dest 52.3676,4.9041 --depart-soc 0.9
cargo swift package --target aarch64-apple-ios -r -n Planner -y
cd ../ios && xcodegen generate && xcodebuild -scheme SliceProto -configuration Release \
  -destination 'generic/platform=iOS' -allowProvisioningUpdates build
# install app, push pack-corridor/* + corridor-z14.pmtiles + ios/assets/style.json
# into the app's Documents via devicectl, launch; it benchmarks itself and
# prints PROTO lines to the console.
```

style.json (prototype/ios/assets/) was generated with the
`@protomaps/basemaps` npm package, light flavor, glyphs/sprites remote.
