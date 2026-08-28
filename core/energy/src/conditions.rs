//! Per-Leg weather/road conditions and the temperature-dependent physics
//! that don't belong to a fixed Vehicle Model: air density and the HVAC
//! power draw (research §2.6, ADR 0003 point 5).

/// Per-edge/Leg weather and terrain context.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Conditions {
    pub temp_c: f64,
    pub headwind_ms: f64,
    pub altitude_m: f64,
}

/// ISA-ish air density from temperature and altitude (research §2.3):
/// `ρ = 1.225 · (288.15 / (T + 273.15)) · exp(−altitude / 8435)`.
pub fn air_density(temp_c: f64, altitude_m: f64) -> f64 {
    const RHO_SEA_LEVEL_15C: f64 = 1.225;
    const T_REF_K: f64 = 288.15;
    const SCALE_HEIGHT_M: f64 = 8435.0;
    RHO_SEA_LEVEL_15C * (T_REF_K / (temp_c + 273.15)) * (-altitude_m / SCALE_HEIGHT_M).exp()
}

/// Piecewise-linear HVAC power draw vs ambient temperature (research §2.6
/// "working model", heat-pump car): ~3.5 kW at −10 °C, ~2 kW at 0 °C,
/// ~300 W flat across 18-24 °C (no heating or cooling needed), ~1.3 kW
/// above 30 °C for A/C. Flat-extrapolated beyond the outermost knots.
const HVAC_KNOTS: &[(f64, f64)] = &[
    (-10.0, 3500.0),
    (0.0, 2000.0),
    (18.0, 300.0),
    (24.0, 300.0),
    (30.0, 1300.0),
];

pub fn p_hvac_w(temp_c: f64) -> f64 {
    if temp_c <= HVAC_KNOTS[0].0 {
        return HVAC_KNOTS[0].1;
    }
    for pair in HVAC_KNOTS.windows(2) {
        let (t0, p0) = pair[0];
        let (t1, p1) = pair[1];
        if temp_c <= t1 {
            let f = (temp_c - t0) / (t1 - t0);
            return p0 + f * (p1 - p0);
        }
    }
    HVAC_KNOTS[HVAC_KNOTS.len() - 1].1
}
