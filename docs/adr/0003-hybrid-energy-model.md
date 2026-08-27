# 3. Hybrid Energy Model: physics core with three calibration scalars

Date: 2026-08-27
Status: Accepted
Wayfinder ticket: https://github.com/arodroz/abrp/issues/12

## Context

The Energy Model predicts a Leg's energy from the Vehicle Model plus the Routing Engine's static per-edge speed (ADR 0001), DEM grade, temperature and wind. Research (#5, `docs/research/ioniq5-energy-model.md`) assembled public parameters for the Ioniq 5 (2022, EU, 72.6 kWh LR) and checked a backward-facing physics model against Bjørn Nyland's 90/120 km/h tests (within 4 %). Alternatives: ABRP's public "reference consumption" concept (one Wh/km scaling a fixed, proprietary speed curve — no grade/wind/payload response), or pure physics with no calibration.

## Decision

1. **Form: hybrid.** Per edge, closed-form physics at constant speed — rolling (Crr·m·g·cosθ), grade (m·g·sinθ), aero (½ρ·CdA·(v+v_headwind)²), kinetic term from Δv at edge boundaries, divided by η_drive; negative energy multiplied by η_regen; plus (P_aux + P_hvac(T))·t; plus a per-road-class urban surcharge (single Wh/km constant).
2. **Speed profile**: driver holds `min(edge speed × k_speed, user max speed)`, `k_speed` default in 1.0–1.15.
3. **Vehicle Model inputs (fixed)**: usable 70 kWh; mass 1 985 kg (LR 2WD) / 2 095 kg (LR AWD) + user payload; CdA 0.72 m²; Crr 0.009; η_drive 0.85; η_regen 0.65; P_aux 300 W; warm and cold Charging Curve tables `P(SoC)` with a battery-temperature scalar between them, capped by the Charger's power, 400 V branch ≈150 kW.
4. **Calibration**: three scalars `k_aero`, `k_roll`, `k_hvac` in the model. v1 UI exposes **one Reference Consumption number** that sets all three proportionally; separable fitting from real trips comes later without a UI redesign.
5. **Temperature**: air density from T and altitude; `P_hvac(T)` curve; 1–3 % penalty on η_drive below 5 °C; Charging Curve interpolated warm↔cold. MY22 has no preconditioning, so the cold curve is the real winter case.
6. **Validation gate**: a Rust test suite reproducing the research targets within ±5 % — 90/110/120 km/h mild, 110 km/h cold, 70 mph cold, warm and cold 10→80 % charge durations.
7. **Location**: a pure Rust function `energy(leg, vehicle, weather) → Wh` called by the Plan optimiser; Swift never computes energy (confirming the boundary sketched in ADR 0001; #13 records the full boundary).

## Consequences

- The Region Pack must carry per-edge grade (or per-node elevation) and speed — an input to the Region Pack format decision (#16).
- Weather enters as per-Leg temperature and wind vectors (source decided in #14).
- Reference Consumption as a single knob keeps parity with what users know from ABRP; the hidden scalars are the escape hatch when one number cannot fit both highway and winter city driving.
- Reversal: switching to an empirical curve later would keep the Vehicle Model and tests, discard the physics core.
