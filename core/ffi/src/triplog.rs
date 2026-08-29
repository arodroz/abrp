//! tlog-1 parsing and `Planner::calibrate`'s logic (ADR 0009 points 3-5).
//! `calibrate_of` is a free function, independent of `Planner`/`Rpack` (ADR
//! 0009 point 5: "the pack plays no role" in the fit), so the whole
//! calibration is unit-testable without a Region Pack -- matching
//! `mapping.rs`'s existing pattern.

use energy::fit::{
    acceptance_of, acceptance_window, is_qualifying_distance, reference_consumption_wh_per_km,
    replay_trace_wh, weighted_median, TraceSample, TripFit,
};
use energy::{Calibration, VehicleModel};
use serde::Deserialize;

use crate::error::PlannerError;
use crate::mapping::vehicle_of;
use crate::types::{FfiCalibrationResult, FfiTripFit, FfiVehicle};

const TLOG_FORMAT: &str = "tlog-1";

fn vehicle_tag(v: FfiVehicle) -> &'static str {
    match v {
        FfiVehicle::Ioniq5Lr2wd => "ioniq5_lr_2wd",
        FfiVehicle::Ioniq5LrAwd => "ioniq5_lr_awd",
    }
}

#[derive(Debug, Deserialize)]
struct TlogFile {
    format: String,
    id: String,
    vehicle: String,
    start_unix: i64,
    #[allow(dead_code)]
    end_unix: i64,
    start_soc_pct: u32,
    end_soc_pct: u32,
    ambient_temp_c: Option<f64>,
    samples: Vec<TlogSample>,
}

#[derive(Debug, Deserialize)]
struct TlogSample {
    t: f64,
    lat: f64,
    lon: f64,
    speed_mps: Option<f64>,
    alt_m: Option<f64>,
    #[allow(dead_code)]
    hacc_m: Option<f64>,
}

#[derive(Debug)]
pub enum TriplogError {
    Json(serde_json::Error),
    UnsupportedFormat(String),
}

impl std::fmt::Display for TriplogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TriplogError::Json(e) => write!(f, "trip log json error: {e}"),
            TriplogError::UnsupportedFormat(got) => {
                write!(f, "unsupported trip log format: {got}")
            }
        }
    }
}

impl std::error::Error for TriplogError {}

impl From<serde_json::Error> for TriplogError {
    fn from(e: serde_json::Error) -> Self {
        TriplogError::Json(e)
    }
}

/// One parsed Trip Log (ADR 0009 point 1).
#[derive(Debug)]
pub struct TripLog {
    pub id: String,
    pub vehicle: String,
    pub start_unix: i64,
    pub start_soc_pct: u32,
    pub end_soc_pct: u32,
    pub ambient_temp_c: Option<f64>,
    pub samples: Vec<TraceSample>,
}

/// Parses one tlog-1 file's full JSON text (Swift reads the files, ADR 0004
/// division).
pub fn parse_triplog(json: &str) -> Result<TripLog, TriplogError> {
    let file: TlogFile = serde_json::from_str(json)?;
    if file.format != TLOG_FORMAT {
        return Err(TriplogError::UnsupportedFormat(file.format));
    }
    let samples = file
        .samples
        .into_iter()
        .map(|s| TraceSample {
            t_s: s.t,
            lat: s.lat,
            lon: s.lon,
            speed_mps: s.speed_mps,
            alt_m: s.alt_m,
        })
        .collect();
    Ok(TripLog {
        id: file.id,
        vehicle: file.vehicle,
        start_unix: file.start_unix,
        start_soc_pct: file.start_soc_pct,
        end_soc_pct: file.end_soc_pct,
        ambient_temp_c: file.ambient_temp_c,
        samples,
    })
}

/// One trip's outcome after the eligibility pass: either it's usable (its
/// current-calibration `TripFit`), or it's excluded with a reason. The
/// exclusion list (wrong vehicle, too few samples, no net discharge,
/// non-positive predicted energy) is this ticket's guard set -- ADR 0009
/// itself only names the >= 100 km acceptance rule.
enum Eligibility {
    Used(TripFit),
    Excluded(String),
}

fn eligibility_of(
    trip: &TripLog,
    vehicle: &VehicleModel,
    expected_tag: &str,
    current_calib: &Calibration,
) -> Eligibility {
    if trip.vehicle != expected_tag {
        return Eligibility::Excluded(format!(
            "vehicle mismatch: trip log is for {}, requested {expected_tag}",
            trip.vehicle
        ));
    }
    if trip.samples.len() < 2 {
        return Eligibility::Excluded("fewer than 2 samples".to_string());
    }
    if trip.start_soc_pct <= trip.end_soc_pct {
        return Eligibility::Excluded("no net discharge".to_string());
    }

    let replay = replay_trace_wh(vehicle, current_calib, trip.ambient_temp_c, &trip.samples);
    if replay.predicted_wh <= 0.0 {
        return Eligibility::Excluded("predicted energy is not positive".to_string());
    }

    let actual_wh = (trip.start_soc_pct as f64 - trip.end_soc_pct as f64) / 100.0
        * vehicle.usable_capacity_kwh
        * 1000.0;
    Eligibility::Used(TripFit {
        distance_m: replay.distance_m,
        actual_wh,
        predicted_wh: replay.predicted_wh,
        ratio: actual_wh / replay.predicted_wh,
    })
}

/// `Planner::calibrate`'s logic (ADR 0009 points 3-5): replays every usable
/// Trip Log under the current calibration to get a median-ratio refit, then
/// replays again under the refit calibration to measure SoC-points
/// acceptance. See `Planner::calibrate`'s doc comment for the semantics
/// this implements; kept as a free function so it's testable without a
/// `Planner`/`Rpack`.
pub fn calibrate_of(
    logs: &[String],
    vehicle: FfiVehicle,
    reference_consumption_wh_per_km_override: Option<f64>,
) -> Result<FfiCalibrationResult, PlannerError> {
    let veh = vehicle_of(vehicle);
    let expected_tag = vehicle_tag(vehicle);

    let mut trips = Vec::with_capacity(logs.len());
    for (i, log) in logs.iter().enumerate() {
        let trip = parse_triplog(log).map_err(|e| PlannerError::InvalidRequest {
            message: format!("trip log at index {i}: {e}"),
        })?;
        trips.push(trip);
    }
    trips.sort_by_key(|t| t.start_unix);

    let current_ref = reference_consumption_wh_per_km_override
        .unwrap_or_else(|| reference_consumption_wh_per_km(&veh, &Calibration::default()));
    let current_calib = Calibration::from_reference_consumption(&veh, current_ref);

    let eligibility: Vec<Eligibility> = trips
        .iter()
        .map(|t| eligibility_of(t, &veh, expected_tag, &current_calib))
        .collect();

    let mut ratio_weights: Vec<(f64, f64)> = eligibility
        .iter()
        .filter_map(|e| match e {
            Eligibility::Used(fit) => Some((fit.ratio, fit.predicted_wh)),
            Eligibility::Excluded(_) => None,
        })
        .collect();
    let median_ratio =
        weighted_median(&mut ratio_weights).ok_or_else(|| PlannerError::InvalidRequest {
            message: "no usable trips: every trip log was excluded".to_string(),
        })?;

    let refit_reference = current_ref * median_ratio;
    let refit_calib = Calibration::from_reference_consumption(&veh, refit_reference);
    let usable_wh = veh.usable_capacity_kwh * 1000.0;

    let mut trip_fits = Vec::with_capacity(trips.len());
    let mut qualifying_errors: Vec<f64> = Vec::new();

    for (trip, elig) in trips.into_iter().zip(eligibility) {
        let fit = match elig {
            Eligibility::Excluded(reason) => FfiTripFit {
                id: trip.id,
                start_unix: trip.start_unix,
                distance_m: 0.0,
                actual_wh: 0.0,
                predicted_wh: 0.0,
                ratio: 0.0,
                used: false,
                qualifying: false,
                error_points: None,
                excluded_reason: Some(reason),
            },
            Eligibility::Used(current) => {
                let refit_replay =
                    replay_trace_wh(&veh, &refit_calib, trip.ambient_temp_c, &trip.samples);
                let predicted_end_soc_pct =
                    trip.start_soc_pct as f64 - refit_replay.predicted_wh / usable_wh * 100.0;
                let error_points = (predicted_end_soc_pct - trip.end_soc_pct as f64).abs();
                let qualifying = is_qualifying_distance(current.distance_m);
                if qualifying {
                    qualifying_errors.push(error_points);
                }
                FfiTripFit {
                    id: trip.id,
                    start_unix: trip.start_unix,
                    distance_m: current.distance_m,
                    actual_wh: current.actual_wh,
                    predicted_wh: current.predicted_wh,
                    ratio: current.ratio,
                    used: true,
                    qualifying,
                    error_points: Some(error_points),
                    excluded_reason: None,
                }
            }
        };
        trip_fits.push(fit);
    }

    // `trips` (and so `qualifying_errors`, built in the same order) is
    // already sorted ascending by `start_unix`.
    let acceptance = acceptance_of(acceptance_window(&qualifying_errors));

    Ok(FfiCalibrationResult {
        reference_consumption_wh_per_km: refit_reference,
        median_ratio,
        accepted: acceptance.accepted,
        mae_points: acceptance.mae_points,
        max_error_points: acceptance.max_error_points,
        trips: trip_fits,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(t: f64, lat: f64, lon: f64) -> String {
        format!(
            r#"{{"t":{t},"lat":{lat},"lon":{lon},"speed_mps":null,"alt_m":300.0,"hacc_m":5.0}}"#
        )
    }

    /// A short, roughly flat, constant-speed trace: enough samples to be
    /// eligible, short enough not to be qualifying (< 100 km).
    fn short_trip_json(id: &str, start_unix: i64, start_soc: u32, end_soc: u32) -> String {
        let n = 200;
        let deg_per_m = 1.0 / 111_320.0;
        let speed_ms = 30.0; // ~108 km/h
        let samples: Vec<String> = (0..n)
            .map(|i| {
                let dist_m = speed_ms * i as f64;
                sample(i as f64, dist_m * deg_per_m, 0.0)
            })
            .collect();
        format!(
            r#"{{"format":"tlog-1","id":"{id}","vehicle":"ioniq5_lr_2wd","start_unix":{start_unix},"end_unix":{end_unix},"start_soc_pct":{start_soc},"end_soc_pct":{end_soc},"ambient_temp_c":15.0,"samples":[{samples}]}}"#,
            end_unix = start_unix + n as i64,
            samples = samples.join(",")
        )
    }

    #[test]
    fn parse_triplog_malformed_json_is_an_error() {
        assert!(parse_triplog("not json").is_err());
    }

    #[test]
    fn parse_triplog_rejects_unknown_format() {
        let json = r#"{"format":"tlog-2","id":"x","vehicle":"ioniq5_lr_2wd","start_unix":0,"end_unix":1,"start_soc_pct":90,"end_soc_pct":80,"ambient_temp_c":null,"samples":[]}"#;
        assert!(matches!(
            parse_triplog(json),
            Err(TriplogError::UnsupportedFormat(_))
        ));
    }

    #[test]
    fn parse_triplog_reads_the_tlog1_shape() {
        let json = short_trip_json("trip-1", 1_000, 90, 80);
        let trip = parse_triplog(&json).expect("valid tlog-1");
        assert_eq!(trip.id, "trip-1");
        assert_eq!(trip.vehicle, "ioniq5_lr_2wd");
        assert_eq!(trip.start_soc_pct, 90);
        assert_eq!(trip.end_soc_pct, 80);
        assert_eq!(trip.samples.len(), 200);
    }

    #[test]
    fn calibrate_of_hard_fails_on_malformed_json_naming_the_index() {
        let logs = vec!["not json".to_string()];
        let err = calibrate_of(&logs, FfiVehicle::Ioniq5Lr2wd, None).unwrap_err();
        match err {
            PlannerError::InvalidRequest { message } => {
                assert!(message.contains("index 0"), "message: {message}");
            }
            other => panic!("expected InvalidRequest, got {other:?}"),
        }
    }

    #[test]
    fn calibrate_of_excludes_wrong_vehicle_trips() {
        let json =
            short_trip_json("trip-1", 1_000, 90, 80).replace("ioniq5_lr_2wd", "ioniq5_lr_awd");
        let logs = vec![json];
        let result = calibrate_of(&logs, FfiVehicle::Ioniq5Lr2wd, None);
        // The one trip is excluded, so there are zero usable trips.
        assert!(matches!(result, Err(PlannerError::InvalidRequest { .. })));
    }

    #[test]
    fn calibrate_of_excludes_no_net_discharge_trips() {
        let logs = vec![short_trip_json("trip-1", 1_000, 50, 60)];
        let result = calibrate_of(&logs, FfiVehicle::Ioniq5Lr2wd, None);
        assert!(matches!(result, Err(PlannerError::InvalidRequest { .. })));
    }

    #[test]
    fn calibrate_of_zero_used_trips_is_an_error() {
        // Both trips ineligible: one wrong vehicle, one no-discharge.
        let logs = vec![
            short_trip_json("trip-1", 1_000, 90, 80).replace("ioniq5_lr_2wd", "ioniq5_lr_awd"),
            short_trip_json("trip-2", 2_000, 50, 60),
        ];
        let result = calibrate_of(&logs, FfiVehicle::Ioniq5Lr2wd, None);
        assert!(matches!(result, Err(PlannerError::InvalidRequest { .. })));
    }

    #[test]
    fn calibrate_of_two_trip_end_to_end() {
        let logs = vec![
            short_trip_json("trip-1", 1_000, 90, 80),
            short_trip_json("trip-2", 2_000, 80, 70),
        ];
        let result = calibrate_of(&logs, FfiVehicle::Ioniq5Lr2wd, None).expect("usable trips");

        assert_eq!(result.trips.len(), 2);
        assert!(result.trips.iter().all(|t| t.used));
        assert!(result.trips.iter().all(|t| !t.qualifying)); // short trips, < 100km
        assert!(result.trips.iter().all(|t| t.error_points.is_some()));
        // Sorted by start_unix.
        assert_eq!(result.trips[0].id, "trip-1");
        assert_eq!(result.trips[1].id, "trip-2");
        // No qualifying trips -> no acceptance window.
        assert!(!result.accepted);
        assert_eq!(result.mae_points, None);
    }

    /// End-to-end against the checked-in 130 km Ardennes corridor fixture
    /// (4064 samples, 90->41 %, 14.0 °C). Kept as a unit test rather than a
    /// `tests/` integration test (ticket: "your call") since `calibrate_of`
    /// and `parse_triplog` are crate-private, and an integration test would
    /// need to expose more surface than the ticket otherwise calls for.
    #[test]
    fn calibrate_of_matches_the_corridor_fixture() {
        let fixture_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../fixtures/tlog-corridor.json"
        );
        let json = std::fs::read_to_string(fixture_path).expect("fixture readable");

        let default_calib = Calibration::default();
        let veh = vehicle_of(FfiVehicle::Ioniq5Lr2wd);
        let default_ref = reference_consumption_wh_per_km(&veh, &default_calib);
        let trip = parse_triplog(&json).expect("fixture parses as tlog-1");
        let default_replay =
            replay_trace_wh(&veh, &default_calib, trip.ambient_temp_c, &trip.samples);

        let logs = vec![json];
        let result =
            calibrate_of(&logs, FfiVehicle::Ioniq5Lr2wd, None).expect("fixture trip is usable");

        assert_eq!(result.trips.len(), 1);
        let fit = &result.trips[0];
        assert!(fit.used, "fixture trip should be used");
        assert!(
            fit.qualifying,
            "fixture trip should be qualifying (>=100km)"
        );
        assert!(
            (125_000.0..=135_000.0).contains(&fit.distance_m),
            "distance_m {} out of [125_000, 135_000] (ground truth ~129_964)",
            fit.distance_m
        );
        assert!(
            (0.90..=1.10).contains(&fit.ratio),
            "ratio {} out of [0.90, 1.10]",
            fit.ratio
        );
        let error_points = fit.error_points.expect("used trip has error_points");
        assert!(
            error_points < 1.0,
            "post-refit error_points {error_points} should be < 1.0 (a single trip's refit \
             nearly zeroes its own error); investigate before loosening"
        );
        assert!(
            result.accepted,
            "single near-self-consistent trip should be accepted"
        );

        eprintln!(
            "corridor fixture: predicted_wh@default={:.1}, ratio={:.4}, refit_reference={:.2} \
             Wh/km (default {:.2}), post-refit error_points={:.4}",
            default_replay.predicted_wh,
            fit.ratio,
            result.reference_consumption_wh_per_km,
            default_ref,
            error_points
        );
    }
}
