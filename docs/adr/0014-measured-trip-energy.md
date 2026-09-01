# 14. Trip Log auto-capture: measured trip energy from OBD counters

Date: 2026-09-01
Status: Accepted
Wayfinder ticket: https://github.com/arodroz/abrp/issues/80

**Delta to ADR 0009** (Energy Model calibration): this ticket keeps ADR 0009's Trip
Log/median-ratio/acceptance design intact and changes one seam inside it -- how a
trip's `actual_wh` is observed -- now that live telemetry (ADR 0013, #77-#79) can
answer that question directly, without displacing the no-dongle path ADR 0009 built.

## Context

ADR 0009 could only observe the battery through the dash SoC display: `actual_wh` was
always `ΔSoC/100 × usable capacity`, an inference riding on 1-point quantization and a
manufacturer-defined usable window that need not equal the physical pack. Live
telemetry (wayfinder #78/#79) now puts a phone-connected dongle's readings on screen
while driving, including the car's own cumulative charge/discharge kWh counters (a
running tally the BMS itself maintains) -- a direct measurement of energy moved,
independent of any SoC-to-capacity assumption. Not every driver runs a dongle, so the
manual/no-telemetry path must keep working exactly as before.

## Decision

1. **tlog-1 gains an optional `telemetry` block; the format id stays `tlog-1`.** All
   eight fields (`start_display_soc_pct`, `end_display_soc_pct`, `start_bms_soc_pct`,
   `end_bms_soc_pct`, `start_cumulative_charge_kwh`, `end_cumulative_charge_kwh`,
   `start_cumulative_discharge_kwh`, `end_cumulative_discharge_kwh`) are individually
   optional. Serde/Codable both treat unknown/missing fields as absent already, so an
   old tlog-1 parses unchanged in new code and a new one parses fine in old code -- no
   format bump, same reasoning ADR 0009 point 5 already relies on for schema growth.
2. **Raw start/end snapshots, not a precomputed delta.** The counters' scaling
   (`÷10`) is triangulated from community sources, not vector-proven (ADR 0013's own
   provenance note); their absolute unit truth is still pending a driveway check
   (#81). Storing the raw snapshots, not a delta computed at capture time, lets an
   already-saved log be re-audited or reinterpreted if that scaling turns out wrong --
   a delta would bake in today's (possibly incorrect) assumption permanently.
3. **`actual_wh` is chosen by precedence, and the choice is recorded.** In order:
   (a) **measured** -- both cumulative counters present with non-negative deltas
   (they're monotonic; a negative delta means a corrupt/rolled-over snapshot, treated
   as absent rather than trusted), `net_kwh = discharge_delta − charge_delta` (net of
   regen, the same way `edge_energy_wh`'s descent term nets against its climb term),
   excluded as "no net discharge (measured)" when `net_kwh <= 0`; (b) **display-SoC
   floats** -- both present and `start > end`, finer than the dash's integer read;
   (c) **manual dash ints** -- ADR 0009's original path, unchanged, including its
   `start <= end` exclusion. A source that's simply absent or corrupt falls through
   silently; only (a)'s and (c)'s "no net energy" cases are terminal exclusions.
   `FfiTripFit.measured` records which of (a) vs. (b)/(c) applied.
4. **One median, no up-weighting.** Measured and inferred trips feed the same
   energy-weighted median (ADR 0009 point 3) with no special treatment for a measured
   trip's ratio -- the median already weights by `predicted_wh`, and a direct
   measurement earning a place in the list is enough; it doesn't need to dominate it.
5. **Acceptance is unchanged: still SoC-points, still ADR 0009's thresholds.** Inside
   that computation, BOTH ends prefer the display-SoC float when present -- the
   predicted curve's start anchor and the actual end reading alike (finer than the
   ints, same reasoning as point 3(b)); reading the two ends from different
   instruments would inject the start reading's rounding into every error.
6. **The Trip Log capture side is lazy about the start snapshot.** The BLE link opens
   at Go (moved earlier than drive entry specifically so the link is usually connected
   by the time the driver answers the start-SoC prompt), but can still land its first
   sweep a few seconds into the trip; the start snapshot is filled by `confirmStartSoc`
   if a fresh reading already exists, else by the first fresh reading during recording.
   This is an accepted v1 bias -- a few hundred meters' drift on the *measured* start
   counter, small next to the trips this feeds a median across.
7. **BMS SoC's schema slots exist but stay null.** No profile signal maps to it yet
   (ADR 0013's provenance note: neither of the Ioniq 5's two SoC fields is BMS-true);
   the fields are captured and stored regardless, so a future profile mapping (#81)
   needs no further schema change, only a value.

## Consequences

- A driver running a dongle gets a materially better calibration input than dash SoC
  ever could, with no change to the manual fallback for a driver who isn't.
- Stored Trip Logs remain fully re-auditable: nothing about the counters' unit scaling
  is baked in past the raw snapshot.
- `FfiTripFit.measured` is new API surface a caller can use to show which trips were
  measured vs. inferred, though this ticket adds no UI for it.
- CONTEXT.md's Trip Log glossary entry is updated to mention automatic telemetry
  capture as the live-dongle case, dash SoC remaining the fallback.
