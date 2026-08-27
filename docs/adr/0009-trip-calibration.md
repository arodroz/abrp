# 9. Energy Model calibration: manual Trip Logs, median-ratio refit, SoC-points acceptance

Date: 2026-08-27
Status: Accepted
Wayfinder ticket: https://github.com/arodroz/abrp/issues/20

## Context

ADR 0003's hybrid Energy Model carries three hidden calibration scalars behind
one user-facing Reference Consumption, gated at ±5 % against published targets,
and deferred how real Ioniq 5 trips calibrate it. Live car data (Hyundai API,
OBD) is out of scope for this effort, so the battery is only observable through
the dash SoC display (integer %); the phone can observe everything else. There
is no backend, and ADR 0004 places all energy math in Rust.

## Decision

1. **A Trip Log captures**: GPS trace (position, speed, altitude, ~1 Hz),
   timestamps, ambient temperature via the existing Open-Meteo integration —
   all automatic — plus start and end SoC typed in from the dash. Two integer
   fields are the entire manual burden per trip.
2. **Manual trip detection.** Explicit start/stop with an SoC prompt at each
   end. Background auto-detection is real engineering that still could not
   capture the dash reading, so the manual moment exists regardless.
3. **Fit = single-scalar Reference Consumption refit, robustly.** Each trip
   implies a ratio actual ÷ predicted energy (ΔSoC × usable capacity vs the
   model replayed over the logged trace). Scale Reference Consumption by the
   energy-weighted **median** ratio across trips — 1-point SoC quantization
   makes short trips noisy, and a median shrugs off outliers where least
   squares chases them. The 3-scalar (aero/rolling/HVAC) least-squares fit is
   the decided escalation path, unlocked only when ≥ ~10 trips span distinct
   speed/temperature regimes; it is not built until then.
4. **Acceptance metric**: calibrated when |predicted − actual arrival SoC|
   ≤ 3 points on trips ≥ 100 km and the mean absolute error over the last 10
   qualifying trips is ≤ 2 points. Shorter trips feed the median ratio
   (energy-weighted) but never gate acceptance — quantization alone can cost
   2 points on a 30 km hop. Consistent with ADR 0003's ±5 % model gate.
5. **Trip Logs are local JSON** in the app's documents, share-sheet
   exportable, never uploaded. **The fit runs in Rust** via one added
   `calibrate()` on the existing `Planner` surface, replaying traces through
   the same per-edge physics the planner uses; Swift only collects GPS and
   hands over files (ADR 0004 division).

## Consequences

- Calibration quality is bounded by dash quantization, not sensor quality;
  the ≥ 100 km rule and median statistics are how the design lives with that.
- Prediction and calibration share one physics implementation, so a
  calibrated scalar means the same thing to both.
- The escalation path to a 3-scalar fit changes no schema: Trip Logs already
  contain everything it needs.
- CONTEXT.md gains **Trip Log** as a glossary term.
