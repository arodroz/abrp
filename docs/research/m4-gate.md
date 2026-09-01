# M4 gate: calibration on real drives

Status: **machinery verified end-to-end; acceptance pending one ≥ 100 km qualifying trip.**
Wayfinder ticket: https://github.com/arodroz/abrp/issues/55 · Date: 2026-09-01

## What was driven

Two real drives, captured via Go → drive → arrival end-SoC prompt on the `84763bd` phone
build (v2 eu-west pack, adopted live after the in-the-act adoption fix `6d453cb`):

| Trip Log | Start (local) | Duration | Distance (replay) | Display SoC | Ambient |
|---|---|---|---|---|---|
| `tlog-1788256322` | 11:52 | 21 min | 15.1 km | 56 → 54 % | 19.0 °C |
| `tlog-1788262528` | 13:35 | 17 min | 16.2 km | 53 → 49 % | 21.2 °C |

Both are city errands — **neither meets the ≥ 100 km qualifying rule**, so per ADR 0009
point 4 the acceptance metric cannot be evaluated yet. No trip-computer kWh/100 km or
phone-battery readings were noted by the driver (checklist step skipped); the capture
itself needed nothing manual beyond the two SoC prompts.

## Capture pipeline: verified

- **Trace quality is textbook**: exact 1 Hz (max Δt 1.0 s, zero gaps > 2 s across 2,279
  samples), 100 % altitude coverage, horizontal accuracy median ≈ 2.2 m / p95 ≤ 7.7 m.
  ADR 0009's "GPS trace, timestamps, ambient temperature — all automatic" holds on real roads.
- **Open-Meteo ambient stamping worked** on both logs (19.0 / 21.2 °C, plausible for the day).
- **The full FFI fit path agrees with an out-of-app replay**: `calibrate_of` mirrored
  locally against `core/energy` proposes refit ≈ 81.75 Wh/km; the phone's Settings →
  Calibration proposed ≈ 81 Wh/km on the same logs. Same math end to end.

## The fit's proposal was rejected — why

Median ratio 0.40 → proposed Reference Consumption ≈ 82 Wh/km (from 205), which is
physically absurd at 110 km/h. Two tangled causes, both anticipated by ADR 0009:

1. **Integer-SoC quantization dominates errand-sized trips.** Trip 1's displayed Δ2 means a
   true ΔSoC anywhere in 1.0–3.0 → its "actual" energy (ΔSoC × 70 kWh usable) is uncertain
   by ±50 %. This is precisely the noise the ≥ 100 km rule exists to drown.
2. **A genuine urban-regime over-prediction underneath.** Replay predicts 216–233 Wh/km
   average for ~45 km/h stop-start errands; trip 1's actual tops out at ~139 Wh/km even at
   quantization's most generous edge — the intervals don't overlap. Prime suspect:
   `eta_regen = 0.65` taxes every stop-start cycle, and the model was gate-validated (±5 %)
   only against highway tests (`gate_tests.rs`). A single-scalar refit would smear this
   urban error onto highway predictions — the planner's main regime — so accepting it on
   errand-only evidence would make the product worse.

Mechanism notes recorded for posterity:

- With two near-equal-weight trips, the energy-weighted median lands on the **lower**
  ratio (first to cross half-weight) — errand-only refits skew worst-case by construction.
- `acceptCalibration()` clamps to [120, 260] Wh/km, so even a mistaken Accept could not
  have set 82. Good defensive design; the driver Dismissed as instructed.
- The device-session checklist predicted the short trip "should be *excluded* by the fit";
  the implementation (matching the ADR, which is the authority) *uses* short trips in the
  median and only withholds them from acceptance. The checklist wording was loose, not the
  code.
- The phone displayed "current ≈ 209 Wh/km" where the 2WD model default is 204.8; no
  persisted override was found in the app's preferences plist snapshot. Unresolved
  cosmetic curiosity, superseded by the vehicle switch below (AWD default 208.3).

## Finding: the app modeled the wrong trim — fixed in the act

The driver's car is the **Ioniq 5 Long Range AWD**; the app shipped hardcoded to
`ioniq5_lr_2wd` (both trims share the 72.6 kWh pack / 70.0 kWh usable — AWD is +110 kg).
Fixed before the qualifying drive so its log is stamped correctly (a later switch would
have orphaned it via `calibrate_of`'s vehicle-mismatch exclusion):

- `PlanStore.vehicle` → `.ioniq5LrAwd`; `TripLogStore`'s tlog stamp → `ioniq5_lr_awd`
  (agreement asserted by triplog-smoke).
- Autotest's own plan config and the golden-bearing smokes (settings/editor/card) now
  **pin** `.ioniq5Lr2wd` explicitly: every golden was computed under the 2WD, and the AWD's
  +110 kg genuinely tips the razor-thin LU→Antwerp stop-free golden under the 10 % arrival
  floor (settings-smoke caught it — the physics working as intended).
- The two errand logs above remain stamped `ioniq5_lr_2wd` (deliberately unmodified gate
  evidence, Mac copies in `.trip-logs/`): future calibrations exclude them as vehicle
  mismatch, which is fine — their refit was being rejected anyway. Settings will show a red
  "every trip log was excluded" note until the first AWD log exists.

Verification on the final binary: **all ten autotest modes green** (plan-golden, map-,
editor-, card-, settings-, install-, triplog-, calibrate-, drive-smoke, perf; the latter
warm-plans at 12.8 ms), plus the earlier per-suite runs. Release device build installed on
the phone.

## Earlier in-the-act findings on this gate (recorded per ticket)

- **Active-region update had no live adoption path** — `runInstall` never reloaded the
  Planner, `setActiveRegion` no-ops on the active region → fixed as `6d453cb`
  (reset-and-reload on active-region commit; three install-smoke adoption asserts).
- **Silent pack-update phases hid an address-space failure** — the staged 4.6 GB v2 rpack
  was byte-perfect, but phone-side deep verify could not mmap it while the live planner +
  MapLibre held ~10 GB; every phase was silent → smart per-row phase UI + planner unload at
  verify time, `84763bd`.

## What remains for the gate

One ≥ 100 km drive on the AWD build (v2 pack, voice guidance), then: Settings →
Calibration, compare the proposal against the trip computer (note kWh/100 km + km this
time), Accept if sane, verify persistence + replan re-anchoring. The errand already
banked counts toward the fit's median only if re-driven under the AWD stamp — otherwise
the qualifying trip alone drives the refit, which the two-trip median quirk above suggests
is no loss.

## Forward look: OBD supersedes the manual half of this gate

The live-telemetry map (#72) plans Trip Log auto-capture with **measured kWh** from the
BMS cumulative-energy counters (0.1 kWh resolution) and auto-read SoC — eliminating both
the quantization problem and the forgotten-notes problem that shaped this gate. Its
delta-ADR to ADR 0009 should revisit the ≥ 100 km rule for measured-energy trips (the rule
exists only because of dash quantization). The qualifying drive can double as the first
OBD-instrumented drive (one drive, both maps' gates) — but nothing blocks it on that.
