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
