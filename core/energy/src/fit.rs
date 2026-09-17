//! Trip Log calibration fit (ADR 0009 points 3-5): replays a logged GPS
//! trace through the same per-edge physics `edge_energy_wh` uses, and the
//! pure statistics (weighted median, acceptance) the fit is built from.
//! Zero external dependencies, matching the rest of this crate.

use crate::calibration::reference_conditions_and_input;
use crate::edge::{edge_energy_wh, EdgeInput};
use crate::vehicle::VehicleModel;
use crate::{Calibration, Conditions};

const EARTH_RADIUS_M: f64 = 6_371_000.0;
/// Ambient temperature fallback when a Trip Log has none (Open-Meteo lookup
/// failed): a neutral mid-range value where the HVAC curve (`p_hvac_w`) is
/// flat at its baseline, so a missing reading doesn't bias the replay
/// towards either heating or cooling load.
const NEUTRAL_TEMP_C: f64 = 15.0;
/// Altitude hysteresis deadband (ticket): a pending altitude drift only
/// commits as ascent/descent once it exceeds this magnitude, so GPS/baro
/// noise on a flat trace doesn't register as climbing. Strictly exceeds,
/// not "at least" -- see the comment on the deadband check itself.
const ALTITUDE_HYSTERESIS_M: f64 = 3.0;
/// A trip only gates acceptance past this distance (ADR 0009 point 4):
/// shorter trips feed the median ratio but SoC quantization alone can cost
/// a couple of points on them.
const QUALIFYING_DISTANCE_M: f64 = 100_000.0;
const ACCEPTANCE_MAX_ERROR_POINTS: f64 = 3.0;
const ACCEPTANCE_MAE_POINTS: f64 = 2.0;

/// One 1 Hz Trip Log fix (ADR 0009 point 1). `speed_mps`, when present,
/// drives `delta_v_kmh` in `replay_trace_wh` (issue #85: GPS position
/// jitter must not be read as phantom acceleration); it is otherwise kept
/// here for the 3-scalar least-squares escalation path ADR 0009 point 3
/// still defers to (it needs an independently observed speed to separate
/// aero from rolling terms).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TraceSample {
    pub t_s: f64,
    pub lat: f64,
    pub lon: f64,
    pub speed_mps: Option<f64>,
    pub alt_m: Option<f64>,
}

/// One `replay_trace_wh` result.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReplaySummary {
    pub predicted_wh: f64,
    pub distance_m: f64,
}

fn haversine_m(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let dphi = (lat2 - lat1).to_radians();
    let dlambda = (lon2 - lon1).to_radians();
    let a = (dphi / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dlambda / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * a.sqrt().asin()
}

/// Replays a Trip Log's trace through `edge_energy_wh`, one call per
/// consecutive sample pair (ADR 0009 point 5: "replaying traces through the
/// same per-edge physics the planner uses").
///
/// - Pairs with `dt <= 0` are skipped defensively (a duplicate or
///   out-of-order fix), and don't advance `delta_v_kmh`'s "previous speed".
/// - `delta_v_kmh` (issue #85): when both fixes carry `speed_mps`, it is
///   `(to.speed_mps - from.speed_mps) * 3.6` -- an independently observed
///   speed rather than one derived from this segment's own (jitter-prone)
///   distance/dt, so GPS position noise no longer reads as phantom
///   acceleration into the kinetic term's asymmetric eta_drive/eta_regen
///   split. Otherwise it falls back to this segment's speed minus the
///   previous segment's speed, previous starting at 0.0 (a trip starts at
///   rest, so the first moving segment pays its kinetic ramp-up, same as
///   `optimiser::eval_leg` does for a road-graph Leg).
/// - `road_class` is always 0 (highway/default, no urban surcharge): the
///   surcharge exists because the planner's constant-speed model can't see
///   stop-start, but this 1 Hz replay sees stop-start directly as kinetic
///   `delta_v` terms -- applying the surcharge on top would double-count it.
/// - Ascent/descent use a hysteresis accumulator over the altitude series
///   rather than raw per-segment differencing (see
///   [`AltitudeHysteresis`]), committing each threshold-crossing delta into
///   the segment where it commits. This is equivalent to committing it at
///   the trip level: `edge_energy_wh`'s climb/descent terms
///   (`m·g·ascent/η_drive`, `m·g·descent·η_regen`) are linear and
///   independent of the segment's speed, so summing them per-segment or
///   attributing the same total to one arbitrary segment integrates to the
///   same total energy.
pub fn replay_trace_wh(
    vehicle: &VehicleModel,
    calib: &Calibration,
    ambient_temp_c: Option<f64>,
    samples: &[TraceSample],
) -> ReplaySummary {
    let temp_c = ambient_temp_c.unwrap_or(NEUTRAL_TEMP_C);
    let mut altitude = AltitudeHysteresis::new(samples.first().and_then(|s| s.alt_m));

    let mut predicted_wh = 0.0;
    let mut distance_m = 0.0;
    let mut prev_speed_kmh = 0.0;

    for pair in samples.windows(2) {
        let (from, to) = (&pair[0], &pair[1]);
        let dt_s = to.t_s - from.t_s;
        if dt_s <= 0.0 {
            continue;
        }

        let seg_distance_m = haversine_m(from.lat, from.lon, to.lat, to.lon);
        let speed_kmh = seg_distance_m / dt_s * 3.6;
        let delta_v_kmh = match (from.speed_mps, to.speed_mps) {
            (Some(from_mps), Some(to_mps)) => (to_mps - from_mps) * 3.6,
            _ => speed_kmh - prev_speed_kmh,
        };
        let (ascent_m, descent_m) = altitude.step(to.alt_m);

        let cond = Conditions {
            temp_c,
            headwind_ms: 0.0,
            altitude_m: from.alt_m.unwrap_or(0.0),
        };
        let input = EdgeInput {
            distance_m: seg_distance_m,
            speed_kmh,
            delta_v_kmh,
            ascent_m,
            descent_m,
            road_class: 0,
        };
        predicted_wh += edge_energy_wh(vehicle, calib, &cond, &input);
        distance_m += seg_distance_m;
        prev_speed_kmh = speed_kmh;
    }

    ReplaySummary {
        predicted_wh,
        distance_m,
    }
}

/// Hysteresis accumulator turning a noisy altitude series into committed
/// ascent/descent deltas (ticket): holds a reference altitude fixed until
/// the latest reading drifts more than 3.0 m away from it, then commits
/// that whole drift as one segment's ascent or descent and re-anchors the
/// reference there. A missing reading contributes nothing and leaves the
/// reference untouched, rather than resetting it.
struct AltitudeHysteresis {
    reference_m: Option<f64>,
}

impl AltitudeHysteresis {
    fn new(initial_m: Option<f64>) -> Self {
        Self {
            reference_m: initial_m,
        }
    }

    /// Advances by one sample's altitude, returning `(ascent_m, descent_m)`
    /// committed at this step (at most one of the two is non-zero).
    fn step(&mut self, alt_m: Option<f64>) -> (f64, f64) {
        let Some(alt_m) = alt_m else {
            return (0.0, 0.0);
        };
        let Some(reference_m) = self.reference_m else {
            self.reference_m = Some(alt_m);
            return (0.0, 0.0);
        };

        let pending = alt_m - reference_m;
        // Strictly greater-than: noise that lands exactly on the deadband
        // edge (e.g. a symmetric +-1.5 m oscillation around a fixed
        // reference produces exactly 3.0 m step-to-step swings) must not
        // commit, or the deadband stops filtering the case it exists for.
        if pending.abs() <= ALTITUDE_HYSTERESIS_M {
            return (0.0, 0.0);
        }
        self.reference_m = Some(alt_m);
        if pending > 0.0 {
            (pending, 0.0)
        } else {
            (0.0, -pending)
        }
    }
}

/// One trip's implied ratio of actual to predicted energy (ADR 0009 point
/// 3).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TripFit {
    pub distance_m: f64,
    pub actual_wh: f64,
    pub predicted_wh: f64,
    pub ratio: f64,
}

/// The model's own Reference Consumption prediction (110 km/h, 1 km, flat,
/// no wind, 23 °C -- the same constants `Calibration::from_reference_consumption`
/// solves against).
pub fn reference_consumption_wh_per_km(vehicle: &VehicleModel, calib: &Calibration) -> f64 {
    let (cond, input) = reference_conditions_and_input();
    let wh = edge_energy_wh(vehicle, calib, &cond, &input);
    wh / (input.distance_m / 1000.0)
}

/// Energy-weighted median of `(value, weight)` pairs (ADR 0009 point 3):
/// sorts by value and returns the first value whose cumulative weight
/// reaches half the total weight. `None` when `pairs` is empty or every
/// weight is non-positive.
pub fn weighted_median(pairs: &mut [(f64, f64)]) -> Option<f64> {
    if pairs.is_empty() {
        return None;
    }
    pairs.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("NaN value in weighted_median"));
    let total_weight: f64 = pairs.iter().map(|&(_, w)| w).sum();
    if total_weight <= 0.0 {
        return None;
    }
    let half = total_weight / 2.0;
    let mut cumulative = 0.0;
    for &(value, weight) in pairs.iter() {
        cumulative += weight;
        if cumulative >= half {
            return Some(value);
        }
    }
    pairs.last().map(|&(value, _)| value)
}

/// SoC-points acceptance over an already-selected window of absolute errors
/// (ADR 0009 point 4: "the last up-to-10 qualifying trips" -- selecting that
/// window is the caller's job, since it needs each trip's `start_unix`,
/// which this crate doesn't know about). Both `max` and `mean` are read
/// over that window alone, not all history.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Acceptance {
    pub accepted: bool,
    pub mae_points: Option<f64>,
    pub max_error_points: Option<f64>,
}

pub fn acceptance_of(window_abs_error_points: &[f64]) -> Acceptance {
    if window_abs_error_points.is_empty() {
        return Acceptance {
            accepted: false,
            mae_points: None,
            max_error_points: None,
        };
    }
    let max = window_abs_error_points
        .iter()
        .cloned()
        .fold(f64::MIN, f64::max);
    let mean = window_abs_error_points.iter().sum::<f64>() / window_abs_error_points.len() as f64;
    Acceptance {
        accepted: max <= ACCEPTANCE_MAX_ERROR_POINTS && mean <= ACCEPTANCE_MAE_POINTS,
        mae_points: Some(mean),
        max_error_points: Some(max),
    }
}

/// Trips only gate acceptance past this distance (ADR 0009 point 4);
/// exposed so `ffi::triplog` doesn't duplicate the constant.
pub fn is_qualifying_distance(distance_m: f64) -> bool {
    distance_m >= QUALIFYING_DISTANCE_M
}

/// The acceptance window is the last this-many qualifying trips (ADR 0009
/// point 4).
pub const ACCEPTANCE_WINDOW: usize = 10;

/// Truncates an already `start_unix`-ordered slice to its last
/// [`ACCEPTANCE_WINDOW`] elements (ADR 0009 point 4). Ordering by
/// `start_unix` is the caller's job -- this crate has no notion of a trip's
/// timestamp, only its trace and SoC readings.
pub fn acceptance_window<T: Copy>(ordered_by_start_unix: &[T]) -> &[T] {
    let start = ordered_by_start_unix
        .len()
        .saturating_sub(ACCEPTANCE_WINDOW);
    &ordered_by_start_unix[start..]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_trip(n: usize, speed_kmh: f64, dt_s: f64) -> Vec<TraceSample> {
        // Straight line north at constant ground speed; no altitude.
        let speed_ms = speed_kmh / 3.6;
        let deg_per_m = 1.0 / 111_320.0; // ~1 degree latitude in meters
        (0..n)
            .map(|i| {
                let dist_m = speed_ms * dt_s * i as f64;
                TraceSample {
                    t_s: i as f64 * dt_s,
                    lat: dist_m * deg_per_m,
                    lon: 0.0,
                    speed_mps: Some(speed_ms),
                    alt_m: None,
                }
            })
            .collect()
    }

    /// Deterministic LCG step (issue #85 tests): no `rand` dependency, just
    /// enough pseudo-randomness to jitter positions reproducibly.
    fn lcg_next(state: &mut u64) -> u64 {
        *state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *state
    }

    /// Applies pseudo-random +-5 m jitter to each sample's lat/lon (issue
    /// #85: simulating GPS position noise), leaving `t_s`, `speed_mps` and
    /// `alt_m` untouched. Two components, both from the same deterministic
    /// LCG: a tiny every-sample wobble (so consecutive samples are never
    /// at the exact same point -- `replay_trace_wh` charges aux time via
    /// `t_s = distance/speed`, which only recovers `dt` when that segment's
    /// speed is nonzero) plus a `hold`-period burst that jumps to a fresh
    /// +-5 m offset and holds it, modelling an occasional multipath outlier
    /// rather than full-scale noise every single fix.
    fn jitter_positions(samples: &[TraceSample], seed: u64, hold: usize) -> Vec<TraceSample> {
        let deg_per_m = 1.0 / 111_320.0;
        let mut state = seed;
        let (mut burst_lat_m, mut burst_lon_m) = (0.0, 0.0);
        samples
            .iter()
            .enumerate()
            .map(|(i, s)| {
                if i % hold == 0 {
                    burst_lat_m =
                        (lcg_next(&mut state) >> 40) as f64 / (1u64 << 24) as f64 * 10.0 - 5.0;
                    burst_lon_m =
                        (lcg_next(&mut state) >> 40) as f64 / (1u64 << 24) as f64 * 10.0 - 5.0;
                }
                let wobble_lat_m =
                    (lcg_next(&mut state) >> 40) as f64 / (1u64 << 24) as f64 * 0.04 - 0.02;
                let wobble_lon_m =
                    (lcg_next(&mut state) >> 40) as f64 / (1u64 << 24) as f64 * 0.04 - 0.02;
                TraceSample {
                    lat: s.lat + (burst_lat_m + wobble_lat_m) * deg_per_m,
                    lon: s.lon + (burst_lon_m + wobble_lon_m) * deg_per_m,
                    ..*s
                }
            })
            .collect()
    }

    // ---- weighted_median ----

    #[test]
    fn weighted_median_empty_is_none() {
        assert_eq!(weighted_median(&mut []), None);
    }

    #[test]
    fn weighted_median_single_value() {
        let mut pairs = [(1.2, 5.0)];
        assert_eq!(weighted_median(&mut pairs), Some(1.2));
    }

    #[test]
    fn weighted_median_odd_count_equal_weights() {
        let mut pairs = [(3.0, 1.0), (1.0, 1.0), (2.0, 1.0)];
        assert_eq!(weighted_median(&mut pairs), Some(2.0));
    }

    #[test]
    fn weighted_median_even_count_equal_weights_takes_upper() {
        // Cumulative weight reaches half exactly at the 2nd of 4 equal
        // weights (half = 2.0, cumulative after 2 = 2.0).
        let mut pairs = [(1.0, 1.0), (2.0, 1.0), (3.0, 1.0), (4.0, 1.0)];
        assert_eq!(weighted_median(&mut pairs), Some(2.0));
    }

    #[test]
    fn weighted_median_heavy_weight_dominates_position() {
        let mut pairs = [(1.0, 1.0), (2.0, 100.0), (3.0, 1.0)];
        assert_eq!(weighted_median(&mut pairs), Some(2.0));
    }

    #[test]
    fn weighted_median_zero_total_weight_is_none() {
        let mut pairs = [(1.0, 0.0), (2.0, 0.0)];
        assert_eq!(weighted_median(&mut pairs), None);
    }

    /// The median's reason to exist (ticket): dash SoC quantization scatters
    /// per-trip ratios around a true value, and the weighted median should
    /// still land close to it.
    #[test]
    fn weighted_median_recovers_true_ratio_under_quantization_noise() {
        let vehicle = VehicleModel::ioniq5_lr_2wd();
        let calib = Calibration::default();
        let true_ratio = 1.10;
        let mut pairs = Vec::new();
        for &(speed_kmh, minutes) in [
            (60.0, 8.0),
            (70.0, 10.0),
            (80.0, 12.0),
            (90.0, 15.0),
            (100.0, 9.0),
            (110.0, 11.0),
            (65.0, 14.0),
            (75.0, 9.0),
            (85.0, 13.0),
            (95.0, 16.0),
        ]
        .iter()
        {
            let n = (minutes * 60.0) as usize + 1;
            let samples = flat_trip(n, speed_kmh, 1.0);
            let replay = replay_trace_wh(&vehicle, &calib, Some(15.0), &samples);
            let true_actual_wh = replay.predicted_wh * true_ratio;
            let true_delta_soc_pct =
                true_actual_wh / (vehicle.usable_capacity_kwh * 1000.0) * 100.0;
            // Dash quantization: SoC is only observed as an integer.
            let quantized_delta_soc_pct = true_delta_soc_pct.round();
            let actual_wh = quantized_delta_soc_pct / 100.0 * vehicle.usable_capacity_kwh * 1000.0;
            let ratio = actual_wh / replay.predicted_wh;
            pairs.push((ratio, replay.predicted_wh));
        }
        let median = weighted_median(&mut pairs).expect("non-empty");
        let err_pct = (median - true_ratio).abs() / true_ratio * 100.0;
        assert!(
            err_pct <= 3.0,
            "median {median} vs true ratio {true_ratio} (err {err_pct}%)"
        );
    }

    /// A roof-box trip at ratio ~2.0 should not drag the median away from
    /// the other five trips clustered at ~1.0 (ADR 0009 point 3: "a median
    /// shrugs off outliers where least squares chases them").
    #[test]
    fn weighted_median_shrugs_off_a_single_outlier() {
        let mut pairs = vec![
            (1.0, 50_000.0),
            (1.02, 60_000.0),
            (0.98, 55_000.0),
            (1.01, 45_000.0),
            (0.99, 52_000.0),
            (2.0, 58_000.0),
        ];
        let median = weighted_median(&mut pairs).expect("non-empty");
        assert!(
            (median - 1.0).abs() < 0.05,
            "median {median} should stay near 1.0 despite the 2.0 outlier"
        );
    }

    // ---- replay_trace_wh ----

    /// A constant-speed flat evenly-spaced trace should replay to
    /// approximately one big edge at that speed with `delta_v` 0 throughout:
    /// `flat_trip` reports `speed_mps` from the very first sample (issue
    /// #85: real GPS speed, not distance/dt, now drives `delta_v_kmh` when
    /// available), so there is no "starts at rest" ramp-up to pay -- the
    /// trace says the vehicle was already at cruising speed at t=0.
    #[test]
    fn replay_matches_closed_form_single_edge_within_ramp_tolerance() {
        let vehicle = VehicleModel::ioniq5_lr_2wd();
        let calib = Calibration::default();
        let speed_kmh = 100.0;
        let dt_s = 1.0;
        let n = 601; // 600 one-second segments
        let samples = flat_trip(n, speed_kmh, dt_s);
        let replay = replay_trace_wh(&vehicle, &calib, Some(15.0), &samples);

        let cond = Conditions {
            temp_c: 15.0,
            headwind_ms: 0.0,
            altitude_m: 0.0,
        };
        let one_edge = edge_energy_wh(
            &vehicle,
            &calib,
            &cond,
            &EdgeInput {
                distance_m: replay.distance_m,
                speed_kmh,
                delta_v_kmh: 0.0,
                ascent_m: 0.0,
                descent_m: 0.0,
                road_class: 0,
            },
        );

        let err_pct = (replay.predicted_wh - one_edge).abs() / one_edge * 100.0;
        assert!(
            err_pct <= 0.5,
            "replay {} vs one-edge {} (err {err_pct}%)",
            replay.predicted_wh,
            one_edge
        );
    }

    /// GPS jitter on a parked car must not be read as speed (issue #85):
    /// with `speed_mps` reporting true rest, the kinetic term should stay
    /// at zero and the only cost is aux draw over elapsed time.
    #[test]
    fn replay_stationary_jitter_costs_only_aux() {
        let vehicle = VehicleModel::ioniq5_lr_2wd();
        let calib = Calibration::default();
        let dt_s = 1.0;
        let n = 601; // 600 one-second segments
        let stationary: Vec<TraceSample> = (0..n)
            .map(|i| TraceSample {
                t_s: i as f64 * dt_s,
                lat: 0.0,
                lon: 0.0,
                speed_mps: Some(0.0),
                alt_m: None,
            })
            .collect();
        let samples = jitter_positions(&stationary, 1, 20);
        let replay = replay_trace_wh(&vehicle, &calib, Some(23.0), &samples);

        let total_seconds = (n - 1) as f64 * dt_s;
        // Full aux draw, not just p_aux_w: at 23 degC (flat HVAC baseline,
        // 18-24 degC per `p_hvac_w`'s knots) the HVAC term is a fixed
        // additional load, same as p_aux_w's -- both are "aux draw over
        // elapsed time", distinct from the mechanical terms this test is
        // isolating.
        let expected_aux_wh =
            (vehicle.p_aux_w + calib.k_hvac * crate::p_hvac_w(23.0)) * total_seconds / 3600.0;
        let err_pct = (replay.predicted_wh - expected_aux_wh).abs() / expected_aux_wh * 100.0;
        assert!(
            err_pct <= 15.0,
            "predicted {} vs aux-only {} (err {err_pct}%)",
            replay.predicted_wh,
            expected_aux_wh
        );
    }

    /// With `speed_mps` available, position jitter (phantom distance/speed
    /// noise) must not leak into the kinetic term (issue #85): a jittered
    /// constant-speed trip should cost close to the same as the clean one.
    #[test]
    fn replay_with_gps_speed_ignores_position_jitter() {
        let vehicle = VehicleModel::ioniq5_lr_2wd();
        let calib = Calibration::default();
        let speed_kmh = 100.0;
        let dt_s = 1.0;
        let n = 601; // 600 one-second segments
        let clean = flat_trip(n, speed_kmh, dt_s);
        let jittered = jitter_positions(&clean, 42, 20);

        let clean_replay = replay_trace_wh(&vehicle, &calib, Some(23.0), &clean);
        let jittered_replay = replay_trace_wh(&vehicle, &calib, Some(23.0), &jittered);

        let err_pct = (jittered_replay.predicted_wh - clean_replay.predicted_wh).abs()
            / clean_replay.predicted_wh
            * 100.0;
        assert!(
            err_pct <= 3.0,
            "jittered {} vs clean {} (err {err_pct}%)",
            jittered_replay.predicted_wh,
            clean_replay.predicted_wh
        );
    }

    /// Without `speed_mps`, the fallback to distance/dt speed (and its
    /// delta_v) must be untouched by the issue #85 fix -- same closed-form
    /// comparison as `replay_matches_closed_form_single_edge_within_ramp_tolerance`.
    #[test]
    fn replay_without_gps_speed_falls_back_to_distance_speed() {
        let vehicle = VehicleModel::ioniq5_lr_2wd();
        let calib = Calibration::default();
        let speed_kmh = 100.0;
        let dt_s = 1.0;
        let n = 601; // 600 one-second segments
        let mut samples = flat_trip(n, speed_kmh, dt_s);
        for s in &mut samples {
            s.speed_mps = None;
        }
        let replay = replay_trace_wh(&vehicle, &calib, Some(15.0), &samples);

        let cond = Conditions {
            temp_c: 15.0,
            headwind_ms: 0.0,
            altitude_m: 0.0,
        };
        let one_edge = edge_energy_wh(
            &vehicle,
            &calib,
            &cond,
            &EdgeInput {
                distance_m: replay.distance_m,
                speed_kmh,
                delta_v_kmh: 0.0,
                ascent_m: 0.0,
                descent_m: 0.0,
                road_class: 0,
            },
        );

        let first_seg_distance_m = haversine_m(
            samples[0].lat,
            samples[0].lon,
            samples[1].lat,
            samples[1].lon,
        );
        let with_ramp = edge_energy_wh(
            &vehicle,
            &calib,
            &cond,
            &EdgeInput {
                distance_m: first_seg_distance_m,
                speed_kmh,
                delta_v_kmh: speed_kmh,
                ascent_m: 0.0,
                descent_m: 0.0,
                road_class: 0,
            },
        );
        let without_ramp = edge_energy_wh(
            &vehicle,
            &calib,
            &cond,
            &EdgeInput {
                distance_m: first_seg_distance_m,
                speed_kmh,
                delta_v_kmh: 0.0,
                ascent_m: 0.0,
                descent_m: 0.0,
                road_class: 0,
            },
        );
        let expected = one_edge + (with_ramp - without_ramp);

        let err_pct = (replay.predicted_wh - expected).abs() / expected * 100.0;
        assert!(
            err_pct <= 0.5,
            "replay {} vs one-edge-plus-analytic-ramp {} (err {err_pct}%)",
            replay.predicted_wh,
            expected
        );
    }

    // ---- altitude hysteresis ----

    #[test]
    fn altitude_noise_within_deadband_yields_zero_ascent_and_descent() {
        let vehicle = VehicleModel::ioniq5_lr_2wd();
        let calib = Calibration::default();
        let mut samples = flat_trip(120, 50.0, 1.0);
        // +-1.5m alternating noise: adjacent readings swing by exactly the
        // 3.0m deadband, so this also exercises the strict/non-strict edge
        // case (must NOT commit -- see the deadband check's comment).
        for (i, s) in samples.iter_mut().enumerate() {
            s.alt_m = Some(if i % 2 == 0 { 100.0 + 1.5 } else { 100.0 - 1.5 });
        }
        // Sanity: replaying should not panic and should produce energy.
        let replay = replay_trace_wh(&vehicle, &calib, Some(15.0), &samples);
        assert!(replay.predicted_wh > 0.0);

        // Directly probe the accumulator to assert zero committed
        // ascent/descent across the whole series.
        let mut acc = AltitudeHysteresis::new(samples[0].alt_m);
        let mut total_ascent = 0.0;
        let mut total_descent = 0.0;
        for s in &samples[1..] {
            let (a, d) = acc.step(s.alt_m);
            total_ascent += a;
            total_descent += d;
        }
        assert_eq!(total_ascent, 0.0);
        assert_eq!(total_descent, 0.0);
    }

    #[test]
    fn altitude_monotone_climb_commits_close_to_the_true_total() {
        let mut acc = AltitudeHysteresis::new(Some(0.0));
        let mut total_ascent = 0.0;
        let mut total_descent = 0.0;
        // 100 one-meter steps up.
        for i in 1..=100 {
            let (a, d) = acc.step(Some(i as f64));
            total_ascent += a;
            total_descent += d;
        }
        assert!(
            (total_ascent - 100.0).abs() <= 3.0,
            "total ascent {total_ascent} should be within a couple of meters of 100"
        );
        assert_eq!(total_descent, 0.0);
    }

    // ---- reference_consumption_wh_per_km round-trip ----

    #[test]
    fn reference_consumption_round_trips_through_from_reference_consumption() {
        let vehicle = VehicleModel::ioniq5_lr_2wd();
        let default_calib = Calibration::default();
        let wh_per_km = reference_consumption_wh_per_km(&vehicle, &default_calib);
        let round_tripped = Calibration::from_reference_consumption(&vehicle, wh_per_km);
        assert!((round_tripped.k_aero - 1.0).abs() < 1e-6);
        assert!((round_tripped.k_roll - 1.0).abs() < 1e-6);
        assert!((round_tripped.k_hvac - 1.0).abs() < 1e-6);
    }

    // ---- acceptance ----

    // The max-error boundary (3.0 points) is isolated from the mean
    // boundary (2.0 points) with a 3-trip window whose mean stays well
    // under 2.0 regardless -- a single-trip window can't test this in
    // isolation, since with one trip max == mean and the mean gate (2.0)
    // is stricter than the max gate (3.0).
    #[test]
    fn acceptance_max_boundary_at_exactly_three_is_accepted() {
        let a = acceptance_of(&[3.0, 1.0, 1.0]);
        assert!(a.accepted);
        assert_eq!(a.max_error_points, Some(3.0));
    }

    #[test]
    fn acceptance_max_just_over_three_is_rejected_even_with_a_low_mean() {
        let a = acceptance_of(&[3.1, 1.0, 1.0]);
        assert!(!a.accepted);
        assert_eq!(a.max_error_points, Some(3.1));
    }

    #[test]
    fn acceptance_ten_trips_at_exactly_the_mae_boundary_is_accepted() {
        let errors = vec![2.0; 10];
        let a = acceptance_of(&errors);
        assert!(a.accepted);
        assert_eq!(a.mae_points, Some(2.0));
    }

    #[test]
    fn acceptance_empty_window_is_never_accepted() {
        let a = acceptance_of(&[]);
        assert!(!a.accepted);
        assert_eq!(a.mae_points, None);
        assert_eq!(a.max_error_points, None);
    }

    #[test]
    fn is_qualifying_distance_boundary() {
        assert!(is_qualifying_distance(100_000.0));
        assert!(!is_qualifying_distance(99_999.0));
    }

    #[test]
    fn acceptance_window_truncates_to_the_last_ten() {
        let errors: Vec<f64> = (1..=15).map(|i| i as f64).collect();
        let window = acceptance_window(&errors);
        assert_eq!(window.len(), 10);
        assert_eq!(window, &errors[5..]);
    }

    #[test]
    fn acceptance_window_passes_through_when_under_ten() {
        let errors = vec![1.0, 2.0, 3.0];
        assert_eq!(acceptance_window(&errors), &errors[..]);
    }
}
