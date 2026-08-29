//! Wire records and enums crossing the boundary (ADR 0004 point 3):
//! immutable, `f64`/`String`/`Vec`/`Option` only. One `Plan` per user
//! gesture, never per-edge or per-point calls.

/// Vehicle selection (ADR 0004 point 2 keeps the Vehicle Model in Rust; the
/// caller only picks a variant).
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Enum)]
pub enum FfiVehicle {
    Ioniq5Lr2wd,
    Ioniq5LrAwd,
}

/// A user waypoint (ADR 0010 point 4).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiWaypoint {
    pub lat: f64,
    pub lon: f64,
    pub depart_soc_override: Option<f64>,
}

/// `Planner::plan`'s single request record.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiPlanRequest {
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub dest_lat: f64,
    pub dest_lon: f64,
    pub waypoints: Vec<FfiWaypoint>,
    pub depart_soc: f64,
    pub arrival_min_soc: f64,
    pub charger_arrival_min_soc: f64,
    pub charger_max_soc: f64,
    pub stops_bias: f64,
    pub temp_c: f64,
    pub headwind_ms: f64,
    pub battery_warmth: f64,
    pub offer_stop_free_alternative: bool,
    pub vehicle: FfiVehicle,
    /// When set, calibrates the Vehicle Model via
    /// `Calibration::from_reference_consumption`; when `None`, the
    /// uncalibrated physics core (`Calibration::default()`) is used.
    pub reference_consumption_wh_per_km: Option<f64>,
}

/// One `(lat, lon)` polyline vertex.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct FfiGeoPoint {
    pub lat: f64,
    pub lon: f64,
}

/// One SoC-curve sample: cumulative Plan distance and the SoC fraction
/// there. Consecutive points are monotone-decreasing except for a vertical
/// jump at each Charging Stop (ADR 0004 point 3: "the SoC curve is one
/// array").
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct FfiSocPoint {
    pub dist_m: f64,
    pub soc: f64,
}

/// One driven Leg, endpoints resolved to human-referenceable labels
/// (`"Origin"`/`"Dest"`/a charger's name/`"Waypoint N"`) since Swift never
/// sees the candidate graph's node indices.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiLeg {
    pub from_label: String,
    pub to_label: String,
    pub drive_s: f64,
    pub dist_m: f64,
    pub energy_wh: f64,
    /// The chosen Speed Cap, `None` when uncapped (ADR 0010 point 1).
    pub speed_cap_kmh: Option<f64>,
    pub depart_soc: f64,
    pub arrival_soc: f64,
    pub flags: Vec<String>,
}

/// One Charging Stop.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiStop {
    pub name: String,
    pub charger_id: String,
    pub lat: f64,
    pub lon: f64,
    pub power_kw: f64,
    pub arrival_soc: f64,
    pub depart_soc: f64,
    pub charge_s: f64,
}

/// The opt-in stop-free alternative (ADR 0010 point 5): the same shape as
/// `FfiPlan` minus its own `alternative` field -- UniFFI records can't
/// self-reference, even through `Option<Box<_>>`, so this is a separate,
/// non-recursive copy rather than a flattened `Vec`.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiPlanAlt {
    pub legs: Vec<FfiLeg>,
    pub stops: Vec<FfiStop>,
    pub drive_time_s: f64,
    pub charge_time_s: f64,
    pub total_time_s: f64,
    pub total_dist_m: f64,
    pub flags: Vec<String>,
    pub soc_curve: Vec<FfiSocPoint>,
    pub polyline: Vec<FfiGeoPoint>,
}

/// The winning Plan.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiPlan {
    pub legs: Vec<FfiLeg>,
    pub stops: Vec<FfiStop>,
    pub drive_time_s: f64,
    pub charge_time_s: f64,
    pub total_time_s: f64,
    pub total_dist_m: f64,
    pub flags: Vec<String>,
    pub soc_curve: Vec<FfiSocPoint>,
    pub polyline: Vec<FfiGeoPoint>,
    pub alternative: Option<FfiPlanAlt>,
}

/// One Trip Log's fit result (ADR 0009 points 3-5), whether or not it was
/// used: an excluded trip still gets a row (`used == false`,
/// `excluded_reason` set), so the caller can show why.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiTripFit {
    pub id: String,
    pub start_unix: i64,
    pub distance_m: f64,
    pub actual_wh: f64,
    pub predicted_wh: f64,
    pub ratio: f64,
    pub used: bool,
    /// `used && distance_m >= 100_000.0` -- only qualifying trips gate
    /// acceptance (ADR 0009 point 4).
    pub qualifying: bool,
    /// Post-refit `|predicted arrival SoC - actual arrival SoC|`, set for
    /// every used trip regardless of `qualifying` (cheap, and useful UX
    /// even for a trip too short to gate acceptance).
    pub error_points: Option<f64>,
    pub excluded_reason: Option<String>,
}

/// `Planner::calibrate`'s result (ADR 0009 points 3-5).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiCalibrationResult {
    /// The refit Reference Consumption: `current × median_ratio` (ADR 0009
    /// point 3).
    pub reference_consumption_wh_per_km: f64,
    pub median_ratio: f64,
    /// `true` when the last up-to-10 qualifying trips have max error
    /// `<= 3.0` points and mean absolute error `<= 2.0` points (ADR 0009
    /// point 4).
    pub accepted: bool,
    pub mae_points: Option<f64>,
    pub max_error_points: Option<f64>,
    pub trips: Vec<FfiTripFit>,
}

/// `Planner::energy`'s input: one edge-shaped what-if (ADR 0004 point 3).
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct FfiLegInput {
    pub distance_m: f64,
    pub speed_kmh: f64,
    pub ascent_m: f64,
    pub descent_m: f64,
    pub temp_c: f64,
    pub headwind_ms: f64,
    pub vehicle: FfiVehicle,
}
