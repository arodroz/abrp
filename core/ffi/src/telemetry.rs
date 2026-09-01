//! UniFFI surface for the profile-driven telemetry decoder (wayfinder #77,
//! ADR 0004 point 3): one coarse `TelemetrySession` object wrapping the
//! `telemetry` crate's dialogue engine and profile decoder end-to-end
//! (construct from profile JSON + a pack-variant id; feed adapter bytes in,
//! drain decoded readings out), plus a standalone `loadTelemetryProfile`
//! summary for a future profile-picker UI. See `docs/adr/
//! 0013-telemetry-profile-format.md` for the Telemetry Profile format this
//! wraps, and `telemetry::profile`/`telemetry::decode` for the format and
//! runtime this is a thin, non-chatty FFI shell over.

use std::sync::{Arc, Mutex};

use telemetry::{
    decode, requests_for, CanonicalSignal, DecodedValue, EcuTarget, Engine, Event, PackVariant,
    TelemetryProfile, ValidationTier,
};

use crate::error::PlannerError;

/// Mirrors `telemetry::ValidationTier` 1:1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiValidationTier {
    CarValidated,
    VectorValidated,
    Paper,
}

fn ffi_tier_of(tier: ValidationTier) -> FfiValidationTier {
    match tier {
        ValidationTier::CarValidated => FfiValidationTier::CarValidated,
        ValidationTier::VectorValidated => FfiValidationTier::VectorValidated,
        ValidationTier::Paper => FfiValidationTier::Paper,
    }
}

/// Mirrors `telemetry::CanonicalSignal` 1:1 (`CONTEXT.md` "Canonical
/// Signal").
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FfiCanonicalSignal {
    BmsSoc,
    DisplaySoc,
    PackCurrent,
    PackVoltage,
    CellVoltage,
    ModuleTemperature,
    Soh,
    RemainingEnergy,
    CumulativeChargeEnergy,
    CumulativeDischargeEnergy,
    CumulativeChargeAh,
    CumulativeDischargeAh,
    Aux12vVoltage,
    Odometer,
}

fn ffi_signal_of(signal: CanonicalSignal) -> FfiCanonicalSignal {
    match signal {
        CanonicalSignal::BmsSoc => FfiCanonicalSignal::BmsSoc,
        CanonicalSignal::DisplaySoc => FfiCanonicalSignal::DisplaySoc,
        CanonicalSignal::PackCurrent => FfiCanonicalSignal::PackCurrent,
        CanonicalSignal::PackVoltage => FfiCanonicalSignal::PackVoltage,
        CanonicalSignal::CellVoltage => FfiCanonicalSignal::CellVoltage,
        CanonicalSignal::ModuleTemperature => FfiCanonicalSignal::ModuleTemperature,
        CanonicalSignal::Soh => FfiCanonicalSignal::Soh,
        CanonicalSignal::RemainingEnergy => FfiCanonicalSignal::RemainingEnergy,
        CanonicalSignal::CumulativeChargeEnergy => FfiCanonicalSignal::CumulativeChargeEnergy,
        CanonicalSignal::CumulativeDischargeEnergy => FfiCanonicalSignal::CumulativeDischargeEnergy,
        CanonicalSignal::CumulativeChargeAh => FfiCanonicalSignal::CumulativeChargeAh,
        CanonicalSignal::CumulativeDischargeAh => FfiCanonicalSignal::CumulativeDischargeAh,
        CanonicalSignal::Aux12vVoltage => FfiCanonicalSignal::Aux12vVoltage,
        CanonicalSignal::Odometer => FfiCanonicalSignal::Odometer,
    }
}

/// A loaded profile's summary (id/name/tier/variants), for a future
/// profile-picker UI -- no engine or decoder constructed.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiTelemetryProfile {
    pub id: String,
    pub name: String,
    pub tier: FfiValidationTier,
    pub variant_ids: Vec<String>,
}

/// One entry out of `TelemetrySession::drainReadings`, batched (never a
/// per-signal call): a canonical-signal reading (`canonical_signal` set),
/// a raw named extra otherwise (`name` is the profile's native signal id;
/// `text_value` carries an OBDb `map`-decoded label when the extra isn't
/// numeric -- ADR 0013 point 3: unmapped native signals are never
/// dropped), or a failed command (`failure_reason` set; the other fields
/// are placeholders on that entry).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FfiTelemetryReading {
    pub canonical_signal: Option<FfiCanonicalSignal>,
    /// 1-based cell/module number for `CellVoltage`/`ModuleTemperature`.
    pub index: Option<u32>,
    pub name: String,
    pub value: Option<f64>,
    pub text_value: Option<String>,
    pub unit: String,
    pub failure_reason: Option<String>,
}

fn failure_reading(target: EcuTarget, uds: &[u8], reason: String) -> FfiTelemetryReading {
    let request_hex: String = uds.iter().map(|b| format!("{b:02X}")).collect();
    FfiTelemetryReading {
        canonical_signal: None,
        index: None,
        name: format!(
            "{:03X}->{:03X} {request_hex}",
            target.tx_header, target.rx_header
        ),
        value: None,
        text_value: None,
        unit: String::new(),
        failure_reason: Some(reason),
    }
}

/// Reuses `PlannerError::InvalidRequest` (error.rs idiom, part G) rather
/// than add a distinct variant: this is already the established
/// "caller-supplied JSON/format was malformed" case elsewhere in this
/// crate (`triplog::parse_triplog`'s tlog-1 errors, `mapping::
/// validate_cpack_format`), and adding a *new* `PlannerError` case would
/// ripple into every exhaustive Swift `switch` over it -- including
/// `app/Wayfinder/Sources/PlanStore.swift`, which this ticket must not
/// touch (a parallel agent owns that lane). See wayfinder #77's report.
fn profile_error(e: telemetry::ProfileError) -> PlannerError {
    PlannerError::InvalidRequest {
        message: e.to_string(),
    }
}

/// Parses and validates a `tprof-1` document, returning its summary.
#[uniffi::export]
pub fn load_telemetry_profile(json: String) -> Result<FfiTelemetryProfile, PlannerError> {
    let profile = TelemetryProfile::load(&json).map_err(profile_error)?;
    Ok(FfiTelemetryProfile {
        id: profile.id,
        name: profile.name,
        tier: ffi_tier_of(profile.tier),
        variant_ids: profile.pack_variants.into_iter().map(|v| v.id).collect(),
    })
}

/// One profile-driven telemetry poll session (ADR 0004 point 3): the
/// engine plus the decoder, end to end, behind the same coarse call shape
/// as `Planner`. Interior mutability for the engine only -- `profile` and
/// `variant` never change after construction.
#[derive(uniffi::Object)]
pub struct TelemetrySession {
    profile: TelemetryProfile,
    variant: PackVariant,
    engine: Mutex<Engine>,
}

#[uniffi::export]
impl TelemetrySession {
    /// Loads `profile_json`, resolves `variant_id` against it, and builds
    /// the engine's poll list (`telemetry::requests_for`) -- headers,
    /// session prerequisites and all.
    #[uniffi::constructor]
    pub fn new(profile_json: String, variant_id: String) -> Result<Arc<Self>, PlannerError> {
        let profile = TelemetryProfile::load(&profile_json).map_err(profile_error)?;
        let variant = profile.variant(&variant_id).map_err(profile_error)?.clone();
        let requests = requests_for(&profile, &variant);
        let engine = Engine::new(requests);
        Ok(Arc::new(Self {
            profile,
            variant,
            engine: Mutex::new(engine),
        }))
    }

    /// Bytes to write to the adapter next, or `None` if nothing is ready
    /// yet (or the session has finished).
    pub fn outgoing(&self) -> Option<Vec<u8>> {
        self.engine().take_outgoing()
    }

    /// Bytes read from the adapter.
    pub fn feed(&self, bytes: Vec<u8>) {
        self.engine().feed(&bytes);
    }

    pub fn on_timeout(&self) {
        self.engine().on_timeout();
    }

    pub fn is_finished(&self) -> bool {
        self.engine().is_finished()
    }

    /// Drains every event completed since the last call, decoded into
    /// canonical/extra readings, plus one entry per failed command --
    /// batched, never per-signal (ADR 0004 point 3).
    pub fn drain_readings(&self) -> Vec<FfiTelemetryReading> {
        let mut engine = self.engine();
        let mut out = Vec::new();
        while let Some(event) = engine.poll_event() {
            match event {
                Event::AdapterReady => {}
                Event::Failed {
                    target,
                    uds,
                    reason,
                } => {
                    out.push(failure_reading(target, &uds, format!("{reason:?}")));
                }
                Event::Payload { target, uds } => {
                    match decode(&self.profile, &self.variant, target, &uds) {
                        Ok(outcome) => {
                            for r in outcome.canonical {
                                out.push(FfiTelemetryReading {
                                    canonical_signal: Some(ffi_signal_of(r.signal)),
                                    index: r.index,
                                    name: String::new(),
                                    value: Some(r.value),
                                    text_value: None,
                                    unit: r.unit,
                                    failure_reason: None,
                                });
                            }
                            for r in outcome.extras {
                                let (value, text_value) = match r.value {
                                    DecodedValue::Number(v) => (Some(v), None),
                                    DecodedValue::Text(t) => (None, Some(t)),
                                };
                                out.push(FfiTelemetryReading {
                                    canonical_signal: None,
                                    index: r.index,
                                    name: r.name,
                                    value,
                                    text_value,
                                    unit: r.unit,
                                    failure_reason: None,
                                });
                            }
                        }
                        Err(e) => out.push(failure_reading(target, &uds, e.to_string())),
                    }
                }
            }
        }
        out
    }
}

impl TelemetrySession {
    fn engine(&self) -> std::sync::MutexGuard<'_, Engine> {
        self.engine.lock().expect("telemetry engine mutex poisoned")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL_PROFILE: &str = r#"{
      "format": "tprof-1",
      "id": "test-profile",
      "name": "Test Profile",
      "vehicle_tags": ["test_car"],
      "tier": "paper",
      "commands": [
        {
          "tx_header": "7E4",
          "rx_header": "7EC",
          "request_hex": "220101",
          "freq_s": 1.0,
          "signals": [
            {"id": "soc", "name": "SoC", "canonical": "display_soc", "bix": 0, "len": 8, "div": 2, "max": 100, "unit": "percent"}
          ]
        }
      ],
      "pack_variants": [
        {"id": "v1", "name": "V1", "populated_cells": 10, "usable_capacity_kwh": 10.0, "nominal_capacity_kwh": 10.0, "remaining_energy_ceiling_kwh": 10.0}
      ]
    }"#;

    #[test]
    fn load_telemetry_profile_summarizes_a_valid_profile() {
        let summary = load_telemetry_profile(MINIMAL_PROFILE.to_string()).expect("valid profile");
        assert_eq!(summary.id, "test-profile");
        assert_eq!(summary.tier, FfiValidationTier::Paper);
        assert_eq!(summary.variant_ids, vec!["v1".to_string()]);
    }

    #[test]
    fn load_telemetry_profile_errors_on_malformed_json() {
        let err = load_telemetry_profile("not json".to_string()).unwrap_err();
        assert!(matches!(err, PlannerError::InvalidRequest { .. }));
    }

    #[test]
    fn telemetry_session_new_errors_on_unknown_variant() {
        // `Arc<TelemetrySession>` isn't `Debug` (it wraps the untouched
        // #76 `Engine`, which derives nothing), so this can't use
        // `unwrap_err()` -- match instead.
        match TelemetrySession::new(MINIMAL_PROFILE.to_string(), "no_such_variant".to_string()) {
            Err(PlannerError::InvalidRequest { .. }) => {}
            Err(other) => panic!("expected InvalidRequest, got {other}"),
            Ok(_) => panic!("expected an error for an unknown variant id"),
        }
    }

    #[test]
    fn telemetry_session_drives_the_engine_and_decodes_a_payload() {
        let session = TelemetrySession::new(MINIMAL_PROFILE.to_string(), "v1".to_string())
            .expect("valid profile and variant");

        // Init handshake: 7 commands, each awaiting its own `>` prompt.
        for _ in 0..7 {
            let cmd = session.outgoing().expect("init command pending");
            assert!(!cmd.is_empty());
            session.feed(b">".to_vec());
        }
        // Target setup: 5 AT commands for the one command's target.
        for _ in 0..5 {
            session.outgoing().expect("setup command pending");
            session.feed(b"OK\r\r>".to_vec());
        }
        // The one data request: single-frame `22 01 01`, single-frame reply
        // `62 01 01 0E` (PCI 0x04 = 4 payload bytes, zero-padded to 8).
        let data_cmd = session.outgoing().expect("data request pending");
        assert_eq!(data_cmd, b"0322010100000000\r");
        session.feed(b"7EC046201010E000000\r\r>".to_vec());

        assert!(session.is_finished());
        let readings = session.drain_readings();
        assert_eq!(readings.len(), 1);
        assert_eq!(
            readings[0].canonical_signal,
            Some(FfiCanonicalSignal::DisplaySoc)
        );
        assert_eq!(readings[0].value, Some(7.0)); // raw 0x0E = 14, /2 = 7 %.
        assert_eq!(readings[0].failure_reason, None);
    }
}
