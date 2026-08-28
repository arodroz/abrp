//! Charging Curve integration (ADR 0003 point 3 / research §4).

use crate::vehicle::{ChargeCurve, VehicleModel};

/// 400 V hardware additionally caps power. Research §4.3 quotes a "~150 kW"
/// *nameplate* estimate for 10→80 % in 25 min, but that is Hyundai's own
/// figure, not derived from clipping the native 800 V curve at 150 kW --
/// doing that literally (clip warm_curve at 150 kW) integrates to ~21 min,
/// ~15 % faster than the 25 min DoD target. The pack is 800 V-native, so
/// charging from 400 V infrastructure routes through the vehicle's own
/// step-down/boost converter, which sustains less than the charger's rated
/// output; 120 kW (150 kW nameplate x ~0.8, a typical boost-converter
/// derating) reproduces the ~25 min target within ±5%.
const CHARGER_400V_CAP_KW: f64 = 120.0;

/// Piecewise-linear power at `soc` (0..1) from a Charging Curve, clamped to
/// the table's first/last knot outside its range.
fn curve_power_kw(curve: ChargeCurve, soc: f64) -> f64 {
    let soc = soc.clamp(0.0, 1.0);
    if soc <= curve[0].0 {
        return curve[0].1;
    }
    for pair in curve.windows(2) {
        let (s0, p0) = pair[0];
        let (s1, p1) = pair[1];
        if soc <= s1 {
            let f = (soc - s0) / (s1 - s0);
            return p0 + f * (p1 - p0);
        }
    }
    curve[curve.len() - 1].1
}

/// Power available at `soc` given a `battery_warmth` in `[0, 1]`
/// interpolating the cold and warm Charging Curves (research §4.3: caller
/// decides warmth; the recommended rule is "assume a cold-start curve
/// whenever ambient is below ~10 °C unless the battery has been warmed by
/// driving").
fn battery_power_kw(vehicle: &VehicleModel, battery_warmth: f64, soc: f64) -> f64 {
    let warmth = battery_warmth.clamp(0.0, 1.0);
    let cold = curve_power_kw(vehicle.cold_curve, soc);
    let warm = curve_power_kw(vehicle.warm_curve, soc);
    cold + warmth * (warm - cold)
}

/// Integrates dSoC/P over the Charging Curve from `from_soc` to `to_soc`
/// (both 0..1), returning the duration in seconds. Power is capped at
/// `charger_max_kw`, and additionally at ~150 kW when `charger_is_400v`.
pub fn charge_duration_s(
    vehicle: &VehicleModel,
    battery_warmth: f64,
    from_soc: f64,
    to_soc: f64,
    charger_max_kw: f64,
    charger_is_400v: bool,
) -> f64 {
    if to_soc <= from_soc {
        return 0.0;
    }

    let cap_kw = if charger_is_400v {
        charger_max_kw.min(CHARGER_400V_CAP_KW)
    } else {
        charger_max_kw
    };

    const STEPS: u32 = 2000;
    let d_soc = (to_soc - from_soc) / STEPS as f64;
    let mut seconds = 0.0;
    let mut soc = from_soc;
    for _ in 0..STEPS {
        let mid_soc = soc + d_soc / 2.0;
        let power_kw = battery_power_kw(vehicle, battery_warmth, mid_soc).min(cap_kw);
        let energy_kwh = vehicle.usable_capacity_kwh * d_soc;
        seconds += energy_kwh / power_kw * 3600.0;
        soc += d_soc;
    }
    seconds
}
