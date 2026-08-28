# 10. Speed Caps in the Charging Stop search and global waypoint optimisation

Date: 2026-08-28
Status: Accepted (amends ADR 0006)
Wayfinder ticket: https://github.com/arodroz/abrp/issues/25

## Context

ADR 0006 deliberately fogged two extensions: "adjust speed to reach the next Charger" and global optimisation across waypoint segments. The planner-UI prototype (#23) produced a "+7 % · 1 min" Charging Stop on a LU→Antwerp Plan — time-optimal under the 10 % arrival floor, but absurd to a driver. Multi-waypoint trips are wanted in the product. The vertical slice (#15) showed the label sets are small (192 candidates, 412 ms on-device), so both extensions are affordable.

## Decision

1. **Speed Caps enter the search as per-Leg branching, not label state.** When the search relaxes an edge between candidates, it expands it once per cap in {uncapped, 110, 100, 90 km/h}: the Leg's speed profile is clamped at the cap (road segments already slower are untouched), yielding up to 4 (time, energy) outcomes into the same (node, SoC) label space. Dominance on (time, SoC) is unchanged. Each Leg of the finished Plan carries its chosen Speed Cap (absent when uncapped) in the Plan contract, so the UI can advise the driver.
2. **Caps, not factors.** Multiplicative slow-down ({90 %, 80 %} of every limit) slows villages absurdly and cannot be told to a driver; a cap ("hold ≤ 100 km/h") can. Below 90 km/h the energy savings flatten while trip time balloons, so three capped options suffice.
3. **Minimum-useful-charge soft penalty.** The objective (ADR 0006 point 3) gains an additive ramp penalty on any Charging Stop whose predicted charging duration is under ~10 minutes: 0 s at ≥ 10 min scaling to +300 s at 0 min, composing with the Stops Bias overhead. Soft, never hard — a genuine short splash at the only feasible Charger must remain plannable.
4. **Waypoint segments are optimised globally, replacing ADR 0006 point 5.** One label search runs over the whole journey with state (node, next-waypoint index, SoC); a label advances past waypoint *i* only by visiting it, and dominance applies within the same waypoint index. The candidate corridor is computed around the full origin→waypoints→destination route. A per-stop departure-SoC override becomes a pinned constraint: labels leaving that waypoint below the override are discarded. Charging at a waypoint that is itself a Charger falls out naturally.
5. **Opt-in stop-free alternative.** When a Plan's only Charging Stop is a micro-stop, the caller may request the stop-free Plan alongside, produced with the arrival floor relaxed and the failing Leg flagged `ARRIVAL_SOC_BELOW_WANTED` (the existing Invalid Plan mechanism), so the UI can offer "skip the 1-min stop, arrive at 8 %". Whether the UI surfaces it is a UI decision; the planner contract only makes it possible.

## Consequences

- The search stays exact over its discretisation; cost grows ~4× in Leg evaluations and stays well under the 1 s warm bar measured in #15.
- The label core is structurally untouched (state stays (node, SoC) plus a waypoint index); the Plan contract grows one optional per-Leg field, Speed Cap.
- The ramp penalty makes micro-stops rare; the speed dimension removes most of the rest; the alternative-Plan option covers what survives. No repair pass is needed.
- Waypoint entry/editing UI is not covered here — it is fog on the map ("waypoint entry and editing in the Google idiom") and graduates later; the planner capability does not wait for it.
