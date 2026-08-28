//! Vehicle Model: fixed Ioniq 5 LR parameters (ADR 0003 point 3, research §2).

/// A `(SoC fraction 0..1, power kW)` knot in a Charging Curve, interpolated
/// piecewise-linearly by [`crate::charging::charge_duration_s`].
pub type ChargeCurve = &'static [(f64, f64)];

/// Warm-battery Charging Curve for the 72.6 kWh LR pack, shared by both
/// drivetrain variants (research §4.1, Hyundai demo session digitised by
/// InsideEVs). Below 10 % SoC there is no published data point; this table
/// assumes the curve is flat at the 10 % value down to 0 %, since the
/// validation gate only exercises 10→80 % and this assumption is otherwise
/// unconstrained.
///
/// The 0.50→0.53 dip to ~120 kW is the "thermal" dip InsideEVs reports at
/// 52-54 % SoC. The ticket's shorthand table omits it, but it is needed to
/// reproduce Hyundai's "10 to 80 % in 18 minutes" DoD target within ±5 %:
/// without it the same knots integrate to ~15.6 minutes (see energy/src/gate_tests.rs
/// test seven_charge_time_warm_350kw for the derivation).
const WARM_CURVE: ChargeCurve = &[
    (0.00, 115.0),
    (0.10, 115.0),
    (0.15, 187.0),
    (0.30, 220.0),
    (0.50, 225.0),
    (0.53, 120.0),
    (0.80, 130.0),
    (0.85, 120.0),
    (0.90, 63.0),
    (0.95, 40.0),
    (1.00, 12.0),
];

/// Cold-battery (no preconditioning, MY21/22) Charging Curve, research §4.3:
/// three independent sessions at -5..0 °C all took "exactly 30 minutes" for
/// 10→80 %, peaking at 147 kW. There is no published per-percent shape for
/// the cold curve, only that one aggregate figure, so these knots are fitted
/// (not sourced point-by-point) to reproduce it: peak ~147 kW around
/// 20-35 % SoC, tapering steadily above that as internal resistance limits
/// power more aggressively than the warm curve does.
const COLD_CURVE: ChargeCurve = &[
    (0.00, 60.0),
    (0.10, 90.0),
    (0.15, 120.0),
    (0.20, 147.0),
    (0.30, 147.0),
    (0.45, 105.0),
    (0.60, 80.0),
    (0.80, 55.0),
    (0.85, 45.0),
    (0.90, 30.0),
    (0.95, 15.0),
    (1.00, 5.0),
];

/// Fixed Vehicle Model parameters (ADR 0003 point 3). `cda_m2` and `crr` are
/// tuned within the research's stated uncertainty ranges (0.69-0.75 m²,
/// 0.008-0.010), near their upper bounds -- see the reasoning in
/// `gate_tests.rs`, which documents each ±5% comparison and why the
/// remaining single outlier (Nyland's 90 km/h run) is accepted.
#[derive(Debug, Clone, Copy)]
pub struct VehicleModel {
    /// Usable battery capacity in kWh (research §2.1: EV Database estimate,
    /// SoC is defined against this figure, not the 72.6 kWh nominal pack).
    pub usable_capacity_kwh: f64,
    /// Kerb mass in kg, EU "unladen" convention (includes a 75 kg driver).
    pub mass_kg: f64,
    /// Drag area (Cd·A) in m².
    pub cda_m2: f64,
    /// Whole-vehicle rolling resistance coefficient.
    pub crr: f64,
    /// Battery-to-wheel efficiency for positive (propulsion) power.
    pub eta_drive: f64,
    /// Wheel-to-battery efficiency for recovered (regen) power.
    pub eta_regen: f64,
    /// Baseline auxiliary electrical load in watts (electronics, pumps;
    /// excludes HVAC, which is `p_hvac_w` and scaled separately by `k_hvac`).
    pub p_aux_w: f64,
    /// Warm-battery Charging Curve, `battery_warmth == 1.0`.
    pub warm_curve: ChargeCurve,
    /// Cold-battery Charging Curve, `battery_warmth == 0.0`.
    pub cold_curve: ChargeCurve,
}

const USABLE_CAPACITY_KWH: f64 = 70.0;
const CDA_M2: f64 = 0.75;
const CRR: f64 = 0.0099;
const ETA_DRIVE: f64 = 0.85;
const ETA_REGEN: f64 = 0.65;
const P_AUX_W: f64 = 300.0;

impl VehicleModel {
    /// Long Range 2WD (RWD), 72.6 kWh pack, 1985 kg kerb (research §2.2).
    pub fn ioniq5_lr_2wd() -> Self {
        Self {
            usable_capacity_kwh: USABLE_CAPACITY_KWH,
            mass_kg: 1985.0,
            cda_m2: CDA_M2,
            crr: CRR,
            eta_drive: ETA_DRIVE,
            eta_regen: ETA_REGEN,
            p_aux_w: P_AUX_W,
            warm_curve: WARM_CURVE,
            cold_curve: COLD_CURVE,
        }
    }

    /// Long Range AWD, 72.6 kWh pack, 2095 kg kerb (research §2.2).
    pub fn ioniq5_lr_awd() -> Self {
        Self {
            mass_kg: 2095.0,
            ..Self::ioniq5_lr_2wd()
        }
    }
}
