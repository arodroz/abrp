//! Calibration: the three scalars ADR 0003 point 4 keeps hidden behind one
//! user-facing Reference Consumption number.

use crate::edge::edge_energy_wh;
use crate::vehicle::VehicleModel;
use crate::{Conditions, EdgeInput};

/// `k_aero` multiplies `VehicleModel::cda_m2`, `k_roll` multiplies `crr`,
/// `k_hvac` multiplies `p_hvac_w`. All default to 1.0 (the uncalibrated
/// physics core).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calibration {
    pub k_aero: f64,
    pub k_roll: f64,
    pub k_hvac: f64,
}

impl Default for Calibration {
    fn default() -> Self {
        Self {
            k_aero: 1.0,
            k_roll: 1.0,
            k_hvac: 1.0,
        }
    }
}

/// The Reference Consumption is defined, like Iternio's public concept
/// (research §5.2), at a constant 110 km/h on flat, calm, mild ground --
/// taken here as the same 23 °C "Highway - Mild" condition as the research
/// §3 110 km/h target, since that is the number `from_reference_consumption`
/// is validated against (research §3 row 3 / gate test 9).
const REFERENCE_SPEED_KMH: f64 = 110.0;
const REFERENCE_TEMP_C: f64 = 23.0;
pub(crate) const REFERENCE_DISTANCE_M: f64 = 1000.0;

/// The Reference Consumption's fixed `Conditions`/`EdgeInput` (110 km/h,
/// flat, no wind, 23 °C, 1 km) -- shared with `fit::reference_consumption_wh_per_km`
/// so the "what is Reference Consumption" definition lives in exactly one
/// place.
pub(crate) fn reference_conditions_and_input() -> (Conditions, EdgeInput) {
    (
        Conditions {
            temp_c: REFERENCE_TEMP_C,
            headwind_ms: 0.0,
            altitude_m: 0.0,
        },
        EdgeInput {
            distance_m: REFERENCE_DISTANCE_M,
            speed_kmh: REFERENCE_SPEED_KMH,
            delta_v_kmh: 0.0,
            ascent_m: 0.0,
            descent_m: 0.0,
            road_class: 0,
        },
    )
}

impl Calibration {
    /// Scales all three calibration scalars by the same factor so that
    /// `vehicle`'s Reference Consumption prediction (110 km/h, flat, no
    /// wind, 23 °C) equals `wh_per_km` exactly.
    ///
    /// This is a closed-form solve, not a search: at fixed speed the
    /// mechanical energy is linear in `k_aero` and `k_roll` (both scale a
    /// single edge's positive aero/rolling terms), and the HVAC term is
    /// linear in `k_hvac`; scaling all three by one factor `s` makes the
    /// whole prediction affine in `s`, which is solved directly against a
    /// `k=1` baseline.
    pub fn from_reference_consumption(vehicle: &VehicleModel, wh_per_km: f64) -> Self {
        let (cond, input) = reference_conditions_and_input();

        // e(s) = baseline_fixed + s * baseline_scaled, both computed at k=0
        // and k=1 respectively so the affine coefficients fall out without
        // duplicating edge_energy_wh's physics here.
        let zero = Self {
            k_aero: 0.0,
            k_roll: 0.0,
            k_hvac: 0.0,
        };
        let one = Self::default();
        let e_at_zero = edge_energy_wh(vehicle, &zero, &cond, &input);
        let e_at_one = edge_energy_wh(vehicle, &one, &cond, &input);
        let target = wh_per_km * (REFERENCE_DISTANCE_M / 1000.0);

        let slope = e_at_one - e_at_zero;
        let s = if slope != 0.0 {
            (target - e_at_zero) / slope
        } else {
            1.0
        };

        Self {
            k_aero: s,
            k_roll: s,
            k_hvac: s,
        }
    }
}
