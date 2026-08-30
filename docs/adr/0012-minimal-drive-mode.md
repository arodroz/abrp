# 12. Minimal Drive Mode: route-following without turn-by-turn

Date: 2026-08-30
Status: Accepted, amended same day (see Amendment)
Wayfinder ticket: https://github.com/arodroz/abrp/issues/58

## Context

The app dead-ends at a planned route: destination in, Plan and result card out,
no way to drive it. The M4 gate (calibration on real drives) is blocked on the
app being usable *as a driver's app*. Research across Google Maps, Apple Maps,
and the navigation SDKs (`docs/research/drive-flow-ux.md`, issue #57) settled
the facts this decision stands on: both reference apps share one five-state
skeleton (browse → place → route preview → active guidance → arrival) with
Go/Start on the preview card and guidance gated on origin = current location;
every SDK examined ships the during-drive HUD as separable components over a
core progress engine, with maneuver banners, lane guidance, and voice as pure
consumers — so route-following without turn-by-turn is established
architecture (ABRP's own drive mode ships exactly that), with documented
calibration numbers (snap ≤10 m, off-route ~50 m sustained, arrival ~40 m,
ETA updates throttled to ≥1 s deltas). The recurring in-drive EV pattern is a
single continuously re-estimated battery-%-at-arrival, stops carrying arrival
SoC + charge time, and energy/position-driven (never tap-driven) stop
progression. Live car data is out of scope for this effort (decision map), so
no telemetry anchor exists mid-drive.

## Decision

1. **Adopt the five-state skeleton; the drive state is route-following, not
   turn-by-turn.** Go on the result card enters Drive Mode; arrival or an
   explicit End exits back to planning. No maneuver banners, no lane guidance,
   no voice — those need per-step instruction data the Plan does not carry and
   are a separate later effort.
2. **Go is gated on the Plan's origin being the current location** (the
   reference apps' rule). No Go→Steps fallback: a remote-origin Plan simply has
   no Go.
3. **The drive HUD is the research's minimal five**: snap-to-route puck (≤10 m
   snap, capped course smoothing), heading-up following camera with free-look
   on gesture + Re-center + overview toggle, remaining-distance/ETA from
   remaining Leg geometry (UI updates throttled), off-route detection at ~50 m
   sustained deviation, and per-stop/destination arrival detection (~40 m)
   advancing the current Leg — the only stepper, replacing maneuver tracking.
4. **The in-drive EV surface is a compact drive card plus the existing SoC
   chart with its marker pinned to the live position** (ABRP's next-Leg graph
   shape): ETA, remaining distance, and arrival SoC at the next Charging Stop
   (destination when none remain). Stop check-off is position-driven; no
   tap-to-skip.
5. **The displayed SoC is model-driven**: the Plan's own predicted SoC curve
   read at the snapped position. Without telemetry there is no live anchor;
   manual mid-drive dash-SoC correction (ABRP's pattern) is a named follow-up,
   deliberately not in v1 — it drags a replan-from-position-and-SoC planner
   entry point into scope and the M4 gate must not wait on it.
6. **Off-route replans are automatic and silent** (Google's pattern) with a
   brief toast: detection decoupled from replan, the replan running through the
   app's own planner from the snapped position so Charging Stops are re-solved,
   never a road-only reroute. Better-route offers are out of scope.
7. **Go opens the drive's Trip Log**: the dash-SoC prompt folds into Go, and
   End/arrival closes the capture with the end-SoC prompt — calibration becomes
   a side effect of driving. The standalone record button stays for non-planned
   drives.
8. **The planning UI is hidden during a drive** (search pill, route editor,
   settings); the drive HUD owns the screen. Exit restores planning with the
   Plan intact. Mid-drive editing is out of scope; the only mid-drive
   mutations are automatic (replans, Leg advance).

## Consequences

- The M4 gate's calibration drives exercise the app as a driver actually uses
  it; capture stops being a separate chore.
- Turn-by-turn, better-route offers, mid-drive editing, and manual SoC
  correction are consciously deferred, each a clean extension point (per-step
  data on Legs; a prompt surface; editor-in-drive; a
  replan-from-position-and-SoC entry point).
- A drive's honesty depends on the off-route replan path: with model-only SoC,
  a silent replan from position is the sole mechanism keeping the displayed
  plan truthful.

## Amendment (2026-08-30)

On seeing what the route-following cut means in practice, the driver overrode
decision 1's deferral: **turn-by-turn guidance is in scope and gates the M4
milestone** — the calibration drives happen only when the app speaks turns.
Decisions 2–8 stand unchanged; route-following remains the foundation
turn-by-turn stacks on (banners/voice as consumers of the progress engine, the
layering every SDK uses). What this pulls into scope: street-name and junction
data through the pipeline into the Region Pack (a format bump), maneuver
generation in the planner, and banner + voice UI in the app — charted via the
research at issue #64, which graduates into build tickets. "Better-route
offers" and mid-drive editing remain out of scope.
