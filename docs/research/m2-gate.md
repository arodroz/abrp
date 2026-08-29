# M2 gate — Drivable v0.1 on-device check (wayfinder #45)

Date: 2026-08-29. Device: iPhone 15 Pro (iPhone16,1), Release build of the full
Wayfinder app (org.anteras.wayfinder), corridor packs sideloaded into Documents via
`devicectl` appDataContainer copy — the same location M3's installer will write.
Core: main at 473cbe3, xcframework rebuilt from it (see the finding below).

## Results

| Measure | Sim (Debug, M-series Mac) | iPhone 15 Pro (Release) | Predicted (m1-gate addendum) | ADR 0001 bar |
|---|---|---|---|---|
| Warm `plan()` | 23.8 ms | **25.1 / 25.4 ms** | ~40 ms | < 1 s ✓ |
| SoC-slider replan | 23.3 ms | **24.6 / 24.7 ms** | — | (the #38 cache's case) |
| Cold-start → first plan | 2142 ms | **2313 / 2339 ms** | ~2.0 s | < 3 s ✓ |
| `phys_footprint` | 76 MB | **391 / 441 MB** | — | < 1 GB ✓ |

- Two consecutive `--autotest perf` runs quoted (first-launch-after-install runs,
  which page in the freshly copied pack, were excluded: 3.57 s / 2.45 s).
- The footprint is the **full app** (MapLibre, PMTiles rendering, chargers layer),
  not m1's bare planner harness (240 MB) — still under half the bar.
- All six autotest modes pass on the device (plan-golden, perf, map-smoke,
  editor-smoke, card-smoke, settings-smoke). Every golden is digit-identical to the
  Mac and simulator runs, e.g. total 14829.701758637884 s and the card's derived
  stop distance 212142.82152175903 m on all three platforms.

## Finding: the gate caught a stale xcframework

The first device run measured warm `plan()` at 2450–2471 ms — byte-for-byte the M1
gate's PRE-optimisation numbers (2454–2489 ms). Cause: `planner_ffi.xcframework`
had last been built at 19:37 on Aug 28, before the perf levers merged (e9519c6,
20:57) — `scripts/build-xcframework.sh` is a manual step, so every app build since
#39 had linked the pre-#38 core. The repo was right; the artifact was old.
Rebuilding the xcframework and re-running dropped warm from ~2.45 s to 25 ms.

Takeaway for later tickets: after any `core/` change, the xcframework must be
rebuilt before app-side numbers mean anything; goldens can't catch it (they are
deliberately identical across the optimisation), only timings can.

## Verdict

**M2 — Drivable v0.1 passes.** The full app plans the golden corridor routes
bit-exactly on the phone through the production path, warm interaction (search
edits, SoC slider) sits at ~25 ms, cold start clears the bar with 660 ms of
headroom, and memory sits under half the bar with the whole map surface loaded.
The #38 prediction (~40 ms warm / ~2.0 s cold at the measured 1.9× Mac→device
factor) was conservative on warm and slightly optimistic on cold.

## Reproduce

Build Release for `generic/platform=iOS`, install with `devicectl device install
app`, copy the five corridor artifacts with `devicectl device copy to --domain-type
appDataContainer --domain-identifier org.anteras.wayfinder --destination
Documents/<name>`, then launch each mode with `devicectl device process launch
--console ... org.anteras.wayfinder --autotest <mode>`. Phone unlocked and on the
cable throughout (this consumed M2's one phone unlock).
