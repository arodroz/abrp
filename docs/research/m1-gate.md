# M1 gate — core parity check against the vertical slice

Measurement note for issue #36. Date: 2026-08-28. Compares the production M1 core (packs → routing → energy → optimiser → UniFFI, merged through 2541d97) against the vertical slice's recorded numbers (456 ms Plan / 694 MB / 1 stop) and the ADR 0001 acceptance bars. Terms follow `CONTEXT.md`.

## Setup

- **Plan**: the golden LU (49.6116, 6.1319) → Amsterdam (52.3702, 4.8952), depart SoC 0.90 — the same request pinned in `core/optimiser/tests/golden_corridor.rs` and `GoldenPlanTests.swift`.
- **Pack**: the pipeline-built corridor artifacts from #35 — `corridor.rpack` (573 MB), 1,549 chargers (`cpack-1`), OSM epoch 2026-08-26.
- **Mac**: `plan_cli` (dev bin in `core/ffi`, behind the `cli` feature) driving the exact production path the app calls — `Planner::new` → `load_chargers` → `plan()` — in both the `release` and the shipping `ffi-release` profile. Peak RSS via `/usr/bin/time -l`.
- **iPhone 15 Pro**: a throwaway xcodegen harness app linking `PlannerKit` with the pack bundled as a resource, built in Release configuration, launched twice via `devicectl` with console capture. Memory via `task_vm_info.phys_footprint`. (A Debug build was measured too: timings identical within noise — the cost is in the Rust core, which is the same optimized static library either way.)

## Results

| Metric | Slice (#15) | Mac `release` | Mac `ffi-release` | iPhone 15 Pro | ADR 0001 bar | Verdict |
|---|---|---|---|---|---|---|
| Plan shape | 1 stop | golden, exact | golden, exact | golden, exact | — | ✅ parity |
| Warm `plan()` | 456 ms | 1031–1056 ms | 1286–1323 ms | 2454–2489 ms | < 1 s | ❌ |
| Cold start → first plan | — | 1673 ms | 1309 ms | 2750–2941 ms | < 3 s | ✅ |
| Peak memory | 694 MB | 734 MB RSS | 728 MB RSS | 240 MB footprint | < 1 GB | ✅ |
| Leg re-route | — | no re-route API in M1; CH p2p is ~1.9–2.4 ms (`bench_routing`) | | | < 300 ms | ✅ headroom |

Device detail: `planner_init` (mmap) 0.3–2.7 ms, charger load ~5 ms for 1,549 sites, first `plan()` 2.6–2.8 s. The Plan is byte-identical to the golden across Mac and phone — 1 stop, "Hyperfast charge laadpalen Nossegem Zaventem", SoC 0.129→0.765, 966 s charge, 14 830 s / 414 871 m total, no flags. The very first launch after a fresh install measured 3.39 s cold-start (first page-in of the just-copied pack); every subsequent launch was 2.68–2.94 s.

## Reading

- **Parity holds exactly.** The full ADR 0010 search reproduces the golden Plan on the phone down to the SoC digits.
- **The warm miss is assembly cost, not the boundary.** The production search evaluates ~1,500 speed-cap leg variants per plan (Mac split: assemble ~975 ms, solve ~11 ms); the slice's 456 ms came from its shortcut optimiser. The Swift/UniFFI layer is not a factor (Debug ≡ Release on device), and `ffi-release`'s opt-level `"s"` costs ~27 % over `3` on the Mac.
- **The device factor is ~1.9×** over the Mac at the same profile (2.46 s vs 1.32 s), so a Mac-side warm `plan_cli` of ≲ 400 ms in `ffi-release` predicts < 1 s on the phone.
- **Memory is comfortable.** The device footprint (240 MB) does not charge clean file-backed pack pages, hence the gap to Mac RSS (~730 MB, which does); both sit under the 1 GB bar and under the slice's 694 MB.

## Verdict

Three of four measurable bars pass and core parity is confirmed; warm `plan()` misses the < 1 s bar. The pre-named perf levers (opt-level, cross-call leg cache, stall-on-demand, m2m early-stop, cached Router) move to #38, which now gates M1 completion. CI green across the workspace at 2541d97.

## Reproduce

```sh
cargo build -p ffi --features cli --bin plan_cli --profile ffi-release
/usr/bin/time -l ./target/ffi-release/plan_cli \
  ~/abrp-data/dist/corridor/corridor.rpack \
  ~/abrp-data/dist/corridor/corridor-chargers.json
```

The device harness is deliberately not in the repo (throwaway, 549 MB bundle): an xcodegen single-target app over `app/PlannerKit`, resources `corridor.rpack` + `corridor-chargers.json`, timing lines printed with an `M1GATE:` prefix and captured with `xcrun devicectl device process launch --console`.
