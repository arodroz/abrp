//! Physics core: per-edge energy from the Vehicle Model, Calibration and
//! Conditions (ADR 0003 point 1, research §5.4 recommendation).

use crate::calibration::Calibration;
use crate::conditions::{air_density, p_hvac_w, Conditions};
use crate::vehicle::VehicleModel;

const G: f64 = 9.80665;
/// Rotational-mass uplift on the kinetic term (research §2.2: "a 3-5 % mass
/// uplift is an adequate stand-in" for wheel/motor inertia at link
/// resolution); the ticket fixes this at 4 %, the middle of that range.
const ROTATING_MASS_UPLIFT: f64 = 1.04;

/// Flat per-km surcharge for `road_class == 1` (urban), covering stop-start
/// losses the constant-speed physics core can't otherwise see. Tuned (free
/// parameter per the ticket) together with the city gate test's assumed
/// speed -- see `gate_tests.rs` test six for the derivation.
pub(crate) const URBAN_SURCHARGE_WH_PER_KM: f64 = 14.0;

/// `road_class` semantics (minimal; the pipeline assigns real classes
/// later): `0` = default/highway, no surcharge. `1` = urban, flat
/// [`URBAN_SURCHARGE_WH_PER_KM`] surcharge. Any other value is treated as
/// `0` (no surcharge).
pub type RoadClass = u8;

/// One Region Pack edge's kinematics, as the Routing Engine and DEM supply
/// them (ADR 0003 consequence: "the Region Pack must carry per-edge grade
/// ... and speed").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeInput {
    pub distance_m: f64,
    pub speed_kmh: f64,
    /// Boundary Δv into this edge (this edge's speed minus the previous
    /// edge's speed); may be negative. Drives the kinetic term.
    pub delta_v_kmh: f64,
    /// Climbed and descended over the edge, stored separately: climbing is
    /// always metered through `eta_drive`, descending always credited
    /// through `eta_regen`, regardless of the edge's other net sign.
    pub ascent_m: f64,
    pub descent_m: f64,
    pub road_class: RoadClass,
}

/// Linear 0 % (at 5 °C and above) to 2 % (at −10 °C and below) penalty on
/// `eta_drive` (ADR 0003 point 5 / ticket: "linear 0 → 2 % as temp goes
/// 5 °C → −10 °C").
fn cold_penalized_eta_drive(eta_drive: f64, temp_c: f64) -> f64 {
    const PENALTY_START_C: f64 = 5.0;
    const PENALTY_END_C: f64 = -10.0;
    const MAX_PENALTY: f64 = 0.02;

    let penalty = if temp_c >= PENALTY_START_C {
        0.0
    } else if temp_c <= PENALTY_END_C {
        MAX_PENALTY
    } else {
        MAX_PENALTY * (PENALTY_START_C - temp_c) / (PENALTY_START_C - PENALTY_END_C)
    };
    eta_drive * (1.0 - penalty)
}

/// Predicts the energy (Wh) to traverse one edge, closed-form at the edge's
/// constant speed (ADR 0003 point 1 / research §5.4):
///
/// - rolling: `k_roll·Crr·m·g·d`, cos θ approximated as 1 (edge grades here
///   are small enough -- under ~10 % -- that cos θ is within ~0.5 % of 1;
///   grade's actual energy contribution is handled exactly below via
///   ascent/descent instead of a sin θ term).
/// - grade, climb and descent metered separately: climb `m·g·ascent_m`
///   always goes through `eta_drive`; descent `m·g·descent_m` is always
///   credited through `eta_regen` -- independent of the sign of the other
///   terms, which is why the pack stores both ascent and descent rather
///   than a single net grade.
/// - aero: `½ρ·(k_aero·CdA)·(v + headwind)²·d`.
/// - kinetic: `½·m_eff·Δ(v²)` from the boundary Δv, with `m_eff` uplifted
///   4 % for rotating mass; can be positive (accelerating) or negative
///   (decelerating).
/// - rolling + aero + kinetic are summed as one "mechanical" bucket: if its
///   net is positive it goes through `eta_drive`, if net-negative (net
///   deceleration outweighing drag) it is credited through `eta_regen`.
/// - plus `(P_aux + k_hvac·P_hvac(T))·t`.
/// - plus the urban surcharge for `road_class == 1`.
pub fn edge_energy_wh(
    vehicle: &VehicleModel,
    calib: &Calibration,
    cond: &Conditions,
    input: &EdgeInput,
) -> f64 {
    let v_ms = input.speed_kmh / 3.6;
    let t_s = if v_ms > 0.0 {
        input.distance_m / v_ms
    } else {
        0.0
    };

    let rho = air_density(cond.temp_c, cond.altitude_m);
    let rolling_j = calib.k_roll * vehicle.crr * vehicle.mass_kg * G * input.distance_m;
    let relative_v_ms = v_ms + cond.headwind_ms;
    let aero_j = 0.5
        * rho
        * (calib.k_aero * vehicle.cda_m2)
        * relative_v_ms
        * relative_v_ms
        * input.distance_m;

    let dv_ms = input.delta_v_kmh / 3.6;
    let m_eff = vehicle.mass_kg * ROTATING_MASS_UPLIFT;
    // Δ(v²) = v_end² − v_start², with v_start = v_end − Δv.
    let delta_v_sq = 2.0 * v_ms * dv_ms - dv_ms * dv_ms;
    let kinetic_j = 0.5 * m_eff * delta_v_sq;

    let eta_drive = cold_penalized_eta_drive(vehicle.eta_drive, cond.temp_c);
    let mechanical_j = rolling_j + aero_j + kinetic_j;
    let mechanical_wh = if mechanical_j >= 0.0 {
        mechanical_j / eta_drive
    } else {
        mechanical_j * vehicle.eta_regen
    } / 3600.0;

    let climb_wh = (vehicle.mass_kg * G * input.ascent_m) / eta_drive / 3600.0;
    let descent_wh = -(vehicle.mass_kg * G * input.descent_m) * vehicle.eta_regen / 3600.0;

    let hvac_w = vehicle.p_aux_w + calib.k_hvac * p_hvac_w(cond.temp_c);
    let thermal_wh = hvac_w * t_s / 3600.0;

    let surcharge_wh = if input.road_class == 1 {
        URBAN_SURCHARGE_WH_PER_KM * (input.distance_m / 1000.0)
    } else {
        0.0
    };

    mechanical_wh + climb_wh + descent_wh + thermal_wh + surcharge_wh
}
