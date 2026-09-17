# Live gate — first instrumented drive end-to-end

Findings note for issue #82. Date: 2026-09-17, on the drive of 2026-09-02. Partial result: the drive was recorded and replayed, but the live-telemetry half of the gate was not exercised. Terms follow `CONTEXT.md`.

## Setup

- **Drive**: Luxembourg (49.4802, 5.7476) → Champagne (48.5474, 4.1905), Ioniq 5 LR AWD, ambient 16.9 °C. Trip Log `tlog-1788327781-DA35087B.json` (format `tlog-1`, 14 105 fixes at ~1 Hz), pulled from the phone's app container with `devicectl` and kept in `.trip-logs/`.
- **Build on phone**: Debug-iphoneos of afb4cc3 (2026-09-02 00:08), so it included the telemetry auto-capture from #80 / ADR 0014.
- **Dongle**: OBDLink CX plugged in — but held by the commercial ABRP app for the whole drive.
- **Replay**: `calibrate_of` for `Ioniq5LrAwd` via a temporary `#[ignore]` test in `core/ffi`, plus a temporary integration test over the public `replay_trace_wh` / `edge_energy_wh` in `core/energy` for the term decomposition. Neither test is kept.

## Results

| Metric | Value |
|---|---|
| Recording window | 07:43 → 11:50 (247 min); actual departure 08:32 after 40 min parked at the start point |
| Distance | 212.0 km (haversine over fixes) |
| Speed | moving mean 97 km/h; 28 % of fixes above 120 km/h; max 129 km/h |
| Stops ≥ 5 min | two at home before departure (8 + 21 min), 6 + 5 min en route, 12 min at 10:59 near 49.0285 / 4.5237 |
| SoC | 100 % → 37 % (manual prompts) |
| SoC-implied energy | 44.1 kWh · 208 Wh/km (70 kWh usable, linear display SoC) |
| Model prediction | 76.2 kWh · 360 Wh/km · ratio 0.58 · post-refit error 22.7 points |
| `telemetry` block | absent — no live reading ever landed |
| Live SoC on HUD | not observed |

Term decomposition of the prediction:

| Variant | Predicted | Kinetic term | Climb net |
|---|---|---|---|
| As recorded | 76.2 kWh | 26.4 kWh | 2.2 kWh |
| Positions rebuilt from logged `speed_mps`, same timing and altitude | 53.3 kWh | 3.8 kWh | 2.1 kWh |

Reference points from the same model: 209 Wh/km steady at 110 km/h flat, 248 Wh/km at 125 km/h.

## Reading

- **The live link never had a chance.** The OBDLink CX is a BLE peripheral; it holds one central connection and stops advertising once connected. With ABRP attached, Wayfinder's scan could not see it. The gate opens at Go (DriveStore, #80), so whether Go was tapped is moot for this drive. The link policy cannot distinguish "dongle held by another app" from "dongle absent" — both are a scan that never finds anything.
- **The replay over-predicts by 32 kWh, and 23 kWh of that is a bug.** `replay_trace_wh` derives segment speed from fix-to-fix distance and charges the asymmetric kinetic term for every phantom Δv that GPS jitter produces (positive Δv sums to 3.1× the logged-speed figure). Filed as #85. Aero and altitude are not implicated.
- **After the jitter is removed the model is +21 % high**, which the ADR 0009 reference-consumption refit is designed to absorb once a measured kWh figure exists. The 44.1 kWh denominator is itself soft: it assumes linear display SoC and no charge during the 12 min stop at 10:59 (unknown). ABRP was logging the same drive with BMS data; its trip history would replace both unknowns.
- **Short trips agree in direction.** The two 15–16 km 2WD logs from 2026-09-01 replay at ratios 0.40 and 0.80 — noisy from 1 % SoC quantization (700 Wh) but also over-predicted.

## Verdict

Gate not passed: no live SoC on the HUD and no auto-captured telemetry, for an environmental reason (dongle owned by another app) rather than a transport or decoder fault. The drive still produced the first real replay, which surfaced #85. Neither fog graduates yet — replan-from-live-SoC and DC charging-curve capture both need a link that has actually connected in the car, which is #81's driveway smoke (with "quit ABRP" as its first step). #82 stays open for the next long drive.

## Reproduce

```sh
# pull logs (phone unlocked)
xcrun devicectl device copy from --device ED47BA12-C341-5363-AEFE-C20015477C96 \
  --domain-type appDataContainer --domain-identifier org.anteras.wayfinder \
  --source Documents/trip-logs --destination <dir>/
```

Replay: an `#[ignore]` test in `core/ffi/src/triplog.rs` reading `TLOG_PATHS` and printing `calibrate_of(&logs, FfiVehicle::Ioniq5LrAwd, None)`; decomposition as described on #85.
