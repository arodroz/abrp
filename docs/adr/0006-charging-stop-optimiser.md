# 6. Charging Stop optimiser: label-setting search over a corridor Charger graph

Date: 2026-08-27
Status: Accepted
Wayfinder ticket: https://github.com/ilres-antonio/abrp/issues/22

## Context

The planner must choose Charging Stops minimising total time under the SoC constraints (#9), in < 1 s warm on-device (ADR 0001), with time-as-weight routing and energy evaluated per Leg (ADR 0001/0003). Alternatives: greedy range-limit selection (misses "charge less here, more at the faster site ahead"); Baum et al.'s exact energy-labelled search over the road graph (needs energy weights in the routing kernel, contradicting ADR 0001).

## Decision

1. **Candidate corridor**: compute the time-optimal origin→destination route once; take DC ≥ 50 kW CCS Chargers from the Charger Pack within *d* = 3 km of it (widen to 10 km if infeasible), capped at ≈300 by power/detour rank. Leg times between candidates via CH many-to-many (bucket / RPHAST-style) queries; Leg energy via the Energy Model.
2. **Search**: Dijkstra-like label-setting over (node, arrival SoC) with dominance on (time, SoC); nodes = origin, destination, candidates. At each Charger the label branches over discrete departure targets {just enough for the next candidate + Charger Arrival SoC, 60 %, 70 %, 80 %, Charger Max SoC}; charge time from the warm/cold Charging Curve capped by the Charger's power.
3. **Objective**: drive + charge + overhead per stop; overhead = 300 s × Stops Bias factor (few-long 3×, quickest 1×, many-short 0.33×); ties → fewer stops.
4. **Invalid Plan**: if the destination is unreachable, relax in order — Charger Arrival SoC → 0 %, Destination Arrival SoC → 0 %, corridor widening — and return the first Plan found with the relaxed Leg flagged `ARRIVAL_SOC_BELOW_WANTED`; if still none, flag the Leg that first reaches 0 %. Never an error.
5. **Waypoints**: consecutive segments solved left-to-right, arrival SoC feeding the next; a per-stop departure-SoC override pins that label.

## Consequences

- Exact over the candidate set, cheap because the set is small; quality depends on the corridor rule, which is tunable without changing the algorithm.
- Needs a many-to-many query API in the Rust routing kernel (input to the Region Pack / kernel work) — one-to-one queries alone would cost O(N²) ms.
- Global optimisation across waypoints and "adjust speed to reach the next Charger" are deliberately left out (fog).
- Reversal: the label-setting core can later carry speed as a label dimension (adjust-speed) or consume energy-weighted routes without changing the Plan contract.
