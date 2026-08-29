# M3 gate — on-device check at eu-west scale (wayfinder #48)

Date: 2026-08-29. Device: iPhone 15 Pro, Release build, `eu-west` installed
**through the production installer** from the hosted catalog
(`wayfinder-packs.home.anteras.org`, ADR 0011) — a real 9.4 GB download on the
phone over Tailscale. Core: main (xcframework current; the cap-500 change is
pipeline-side and does not touch the on-device kernel).

## Install path: passes

- The Packs UI installed `eu-west` end-to-end: background download with live
  progress, streamed sha256 verification of the 4.37 GB rpack and 5.67 GB
  pmtiles, all-or-nothing commit, `installed-eu-west.json` recorded.
- The first attempt was **jetsam-killed during verification**: hashing GBs on
  the main actor autoreleases every 4 MB chunk into a pool the blocked runloop
  never drains. Fixed (per-chunk `autoreleasepool` + hashing detached off the
  main actor); the retry installed cleanly. install-smoke's 49 MB fixture could
  never have caught this — GB-scale verification needs a device-scale test.
- `install-smoke` passes **on the phone** (live index → lu-dev install →
  17 chargers via PlannerClient → delete), over the phone's own tailnet route.

## Scale bars: two fail

Three perf runs (first-launch page-in run excluded: 13.3 s / 1184 MB):

| Measure | eu-west (device) | corridor (M2) | ADR 0001 bar |
|---|---|---|---|
| Warm `plan()` | **102–105 ms** | 25 ms | < 1 s ✓ |
| SoC-slider replan | 101–104 ms | 24.7 ms | ✓ |
| Cold-start → first plan | **11.8–11.9 s** | 2.3 s | < 3 s ✗ (3.9×) |
| `phys_footprint` (planner path) | **1172–1316 MB** | 441 MB | < 1 GB ✗ |

- Goldens stay bit-exact at scale: `plan-golden` passes on the eu-west pack on
  the device (LU→Amsterdam reproduces digit-identical, as at the pack-run QA).
- **The four store-driven modes (map surface + planner) crash**: after the
  chargers layer loads all 40,944 sites, the first plan's corridor assembly
  aborts in the Rust allocator (`memory allocation of 91313632 bytes failed`,
  signal 6). Planner + full map together exceed the per-process ceiling that
  the planner-only path stays (barely) under.

## Diagnosis

Cold time and memory fail together and point at the same thing: **corridor
assembly scales with the total charger count, not with near-route
candidates** (Mac cold: corridor 1.03 s at 1,549 chargers → eu-west 8.0 s at
40,944; device ×~1.5). The m2m/charge-table work over the full charger set
also touches far more of the 52.9 M-edge graph than any single query needs,
ballooning mmap residency. Warm plans are untouched (the #38 corridor cache).

## Verdict

**M3 does not close on this run.** The install/refresh path — the milestone's
new machinery — passes end-to-end on-device; the scale bars fail on the
pre-existing assembly path. The fix (spatial candidate pruning before
snap/m2m; bound the chargers layer's resident geometry if needed) proceeds as
a lever ticket gating the milestone, the same shape as M1's warm-plan miss
(#36 → #38). Corridor stays in the hosted catalog until the bars pass.

## Reproduce

Install eu-west via Settings → Packs on a tailnet phone, then
`devicectl device process launch --console ... --autotest perf` (three runs,
discard the first after a fresh install). The store-driven modes reproduce the
allocator abort. Autotest modes follow the persisted `activeRegion`.

## Addendum (wayfinder #50)

The diagnosis above located the scaling in the wrong place: candidate
selection was already tight (78 kept of 40,944 at eu-west, near-identical to
corridor's 76). The real cost was `Router::p2p` allocating and zero-filling
six `vec![...; node_count]` state vectors PER QUERY — 91,313,632 bytes each
for the `f64` ones at eu-west's 11.4 M nodes, the exact allocation that
aborted — times ~1,550 leg-evaluation queries. After #50 (sparse per-query
search state + `from_of_edge` precomputed once in the Planner): Mac cold
6.28 s → 0.27 s, peak footprint 3.93 GB → 245 MB, warm 95 ms → 11 ms; sim
at eu-west scale: cold-from-launch 0.36 s, footprint 482 MB, all seven
autotest modes green. Routes are bit-identical (same traversal, sparse
storage). Device numbers seal at the next unlock.

## Sealed device numbers (wayfinder #54)

Date: 2026-08-29, iPhone 15 Pro, Release build ba30d27, eu-west active.
Three perf runs (first run after a fresh app install excluded per this doc's
own protocol: 1.73 s / 431 MB page-in run):

| Measure | eu-west (device, sealed) | at the failed gate | ADR 0001 bar |
|---|---|---|---|
| Cold-start → first plan | **526–536 ms** | 11.8–11.9 s | < 3 s ✓ (5.6×) |
| Warm `plan()` | **13.5–14.2 ms** | 102–105 ms | < 1 s ✓ |
| SoC-slider replan | 13.0–14.7 ms | 101–104 ms | ✓ |
| `phys_footprint` (planner path) | **431–445 MB** | 1172–1316 MB | < 1 GB ✓ |

All four store-driven modes (map surface + planner in one process) pass on
the device — the 91 MB allocator abort is gone. Goldens digit-identical
throughout. **M3's bars pass on the device; the milestone's verdict stands
closed.** Corridor retired from the hosted catalog and the phone's
sideloaded corridor files cleaned in the same session (#54).
