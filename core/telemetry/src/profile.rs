//! Telemetry Profile format `tprof-1` (wayfinder #77, ADR 0013): loads and
//! validates the JSON data file describing how to read one vehicle's live
//! telemetry over OBD -- ECUs/commands to poll, per-signal decode rules
//! (OBDb-near-verbatim keys, ADR 0013 point 2), pack-variant constants, and
//! the mapping of native signals onto [`CanonicalSignal`]s. `format:
//! "tprof-1"` mirrors the existing `tlog-1` convention (`core/ffi/src/
//! triplog.rs`).
//!
//! # The bit-index origin (ADR 0013 point 2, resolved empirically here)
//!
//! OBDb's `bix`/`len` decode keys count *bits*, MSB-first within each byte,
//! big-endian across bytes -- but from what origin? Our imported guide
//! (`docs/research/ioniq5-obd-telemetry.md`) placed DID `22 01 01`'s SoC
//! byte at absolute payload byte 6 (counting the leading `0x62` positive-
//! response byte as byte 0); OBDb's own recorded Ioniq 5 test vector for
//! that exact command decodes `IONIQ5_HVBAT_SOC` (`bix: 32, len: 8, div:
//! 2`) to 71 % from raw byte `0x8E` at absolute byte 7 of that same
//! reassembled payload.
//!
//! Working the vectors (this crate's `tests/ioniq5_vectors.rs`) settles it:
//! **`bix` counts bits from zero starting immediately *after* the echoed
//! UDS request prefix** -- for a `22 <DID_hi> <DID_lo>` request, that's the
//! 3 bytes `62 <DID_hi> <DID_lo>`; more generally, `skip = request.len()`
//! bytes (the positive-response SID plus the echoed remainder of whatever
//! was requested), since the positive response always echoes the request
//! verbatim except for the SID's `+0x40`. `bix: 32` is therefore byte 4 of
//! the *post-prefix* data, i.e. absolute byte 7 -- matching OBDb's vector,
//! not the guide's byte 6 (the guide undercounted the prefix by one byte,
//! apparently modeling a 1-byte DID instead of `22`'s real 2-byte DID).
//! This was cross-checked against four independent fields in the same
//! payload (SoC, pack current, 12 V aux voltage, cumulative charge Ah) and
//! against every field of the ICCU's `22 E0 11` command on a different ECU
//! -- all match OBDb's recorded expected values exactly under this origin.
//!
//! One further, separate finding travels with the byte-offset one: OBDb's
//! own field name for that `bix: 32` SoC signal is "HV battery charge,
//! **dashboard**" (`suggestedMetric: stateOfCharge`) -- i.e. OBDb's own
//! contributors consider this signal **Display SoC**, not the "true/raw
//! BMS SoC" the guide's table claimed for the same DID. Neither DID `22 01
//! 01` nor `22 01 05` exposes a field OBDb itself labels as a true/BMS
//! reading (the `22 01 05` SoC-shaped field is even named
//! `..._SOC_DISP`). Lacking a vector-backed field for it, the first-party
//! Ioniq 5 profile does not map anything to [`CanonicalSignal::BmsSoc`] --
//! see the crate-level report for this ticket rather than fabricate one.
//!
//! And a boundary of what vector validation *can* settle: it proves bit
//! **extraction** (raw integers match the recording), never a field's
//! **unit scale** -- any profile that mirrors OBDb's own scale reproduces
//! their `expected_values`, wrong or right. Where OBDb's signalset carries
//! no scale but independent triangulated sources confirm one (the four
//! cumulative Ah/kWh counters: OVMS + EVNotify + Torque all divide the raw
//! u32 by 10, `docs/research/ioniq5-obd-telemetry.md` §3), the first-party
//! profiles decode the physical unit and `tests/ioniq5_vectors.rs`
//! compares those fields against `expected / 10` -- extraction stays
//! vector-exact; the unit claim is attributed to its real source.
//!
//! # `omin`/`omax`/`oval` (adopted key names, app-original semantics)
//!
//! OBDb's own reference decoder (`OBDb/.schemas` `python/can/signals.py`)
//! parses these keys but never reads them back -- only `min`/`max`
//! (post-scale clamp) are implemented there. No signal in either shipped
//! profile needs them, so this crate defines its own consistent semantic
//! for the adopted names rather than guess at an undocumented one: when a
//! rule sets `omin`/`omax`/`oval` together, a decoded value falling
//! outside `[omin, omax]` is replaced with `oval`. `nullmin`/`nullmax`
//! (also unimplemented upstream) are treated as "the decoded value in this
//! range means the reading is absent" -- both checks run on the fully
//! scaled value, before the final `min`/`max` clamp.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer};

pub const TPROF_FORMAT: &str = "tprof-1";

/// The engine's small fixed live-telemetry vocabulary (ADR 0013 point 3;
/// `CONTEXT.md` "Canonical Signal"). Every profile maps whichever of its
/// native signals it can onto these; the app consumes canonical signals
/// only. [`CanonicalSignal::CellVoltage`] and
/// [`CanonicalSignal::ModuleTemperature`] are inherently multi-instance --
/// their readings carry a 1-based cell/module `index` alongside the value
/// (see `decode::CanonicalReading`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CanonicalSignal {
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

impl CanonicalSignal {
    /// Parses the `snake_case` name used in a profile's `canonical` field.
    pub fn parse(name: &str) -> Option<CanonicalSignal> {
        use CanonicalSignal::*;
        Some(match name {
            "bms_soc" => BmsSoc,
            "display_soc" => DisplaySoc,
            "pack_current" => PackCurrent,
            "pack_voltage" => PackVoltage,
            "cell_voltage" => CellVoltage,
            "module_temperature" => ModuleTemperature,
            "soh" => Soh,
            "remaining_energy" => RemainingEnergy,
            "cumulative_charge_energy" => CumulativeChargeEnergy,
            "cumulative_discharge_energy" => CumulativeDischargeEnergy,
            "cumulative_charge_ah" => CumulativeChargeAh,
            "cumulative_discharge_ah" => CumulativeDischargeAh,
            "aux_12v_voltage" => Aux12vVoltage,
            "odometer" => Odometer,
            _ => return None,
        })
    }

    /// `true` for the two signals whose readings carry a cell/module index
    /// (used to decide which readings pack-variant truncation applies to).
    pub fn is_indexed(self) -> bool {
        matches!(
            self,
            CanonicalSignal::CellVoltage | CanonicalSignal::ModuleTemperature
        )
    }
}

/// A profile's declared confidence (ADR 0013 point 1 / `CONTEXT.md`
/// "Telemetry Profile"). Ordered loosest-to-strongest is `Paper` <
/// `VectorValidated` < `CarValidated`, but this type doesn't encode an
/// ordering -- callers compare the tier they need directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ValidationTier {
    CarValidated,
    VectorValidated,
    Paper,
}

/// Per-pack-variant constants (ADR 0013 point 1): the facts OBDb's own
/// schema cannot express (`docs/research/obd-datasets.md` §2c) but that a
/// decoder needs -- how many cell-voltage slots are real, and the capacity/
/// remaining-energy numbers a future energy or UI feature would want.
#[derive(Debug, Clone, Deserialize)]
pub struct PackVariant {
    pub id: String,
    pub name: String,
    /// Cell-voltage slots beyond this index read as unpopulated wiring and
    /// are dropped, not just left at 0 V (e.g. 180 on the Ioniq 5's 72.6
    /// kWh pack, out of 192 wire slots the shared E-GMP commands expose).
    pub populated_cells: u32,
    pub usable_capacity_kwh: f64,
    pub nominal_capacity_kwh: f64,
    pub remaining_energy_ceiling_kwh: f64,
    #[serde(default)]
    pub notes: String,
}

/// One native signal's decode rule, OBDb-near-verbatim (ADR 0013 point 2).
/// `bix`/`len` are bit offset/width, origin per the module docs above.
/// `count` > 1 repeats this rule `count` times, each subsequent reading
/// `len` bits after the previous, numbered from `first_index` (cell
/// voltages, module temperatures); absent, this is a single scalar
/// reading.
#[derive(Debug, Clone, Deserialize)]
pub struct DecodeRule {
    /// Native signal id, unique within its command (e.g.
    /// `"hvbat_soc_dashboard"`).
    pub id: String,
    pub name: String,
    /// This signal's [`CanonicalSignal`], by `snake_case` name; `None`
    /// leaves it a raw named extra (never dropped).
    #[serde(default)]
    pub canonical: Option<String>,
    pub bix: u32,
    pub len: u32,
    /// Reverses the covered byte range before extraction, for a
    /// little-endian multi-byte field (OBDb `blsb`); no shipped signal
    /// needs this, but the key is adopted per ADR 0013 point 2.
    #[serde(default)]
    pub blsb: bool,
    #[serde(default)]
    pub sign: bool,
    #[serde(default)]
    pub add: f64,
    #[serde(default = "one")]
    pub mul: f64,
    #[serde(default = "one")]
    pub div: f64,
    #[serde(default)]
    pub min: f64,
    pub max: f64,
    pub unit: String,
    #[serde(default)]
    pub nullmin: Option<f64>,
    #[serde(default)]
    pub nullmax: Option<f64>,
    #[serde(default)]
    pub omin: Option<f64>,
    #[serde(default)]
    pub omax: Option<f64>,
    #[serde(default)]
    pub oval: Option<f64>,
    /// Enumeration lookup (OBDb `map`): raw integer, as a string, to label.
    /// When set, this rule decodes to text instead of a scaled number, and
    /// `sign`/`add`/`mul`/`div`/`min`/`max`/null/override are ignored.
    #[serde(default)]
    pub map: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub count: Option<u32>,
    #[serde(default = "one_u32")]
    pub first_index: u32,
}

fn one() -> f64 {
    1.0
}
fn one_u32() -> u32 {
    1
}

/// One ECU command to poll (ADR 0013 point 1): headers, the UDS request
/// bytes, an optional session prerequisite, poll cadence, and the signals
/// its response carries.
#[derive(Debug, Clone, Deserialize)]
pub struct CommandDef {
    #[serde(deserialize_with = "hex_u16")]
    pub tx_header: u16,
    #[serde(deserialize_with = "hex_u16")]
    pub rx_header: u16,
    /// The UDS request bytes, hex (e.g. `"220101"` for `22 01 01`).
    #[serde(deserialize_with = "hex_bytes", rename = "request_hex")]
    pub request: Vec<u8>,
    /// A UDS request to send once, ahead of this command, whenever this
    /// target hasn't already had it sent (e.g. `"1003"` --
    /// `DiagnosticSessionControl(extendedSession)` -- before the Ioniq 5's
    /// ICCU reads; `docs/research/ioniq5-obd-telemetry.md` §2/§5).
    #[serde(
        deserialize_with = "hex_bytes_opt",
        rename = "session_prerequisite_hex",
        default
    )]
    pub session_prerequisite: Option<Vec<u8>>,
    /// Poll cadence in seconds (OBDb `freq`); metadata for a future
    /// scheduler, not consumed by `requests_for` in this ticket.
    pub freq_s: f64,
    /// Overrides the engine's default flow-control data bytes (hardcoded
    /// `30 00 00` -- clear-to-send, block size 0, STmin 0 -- in
    /// `dialogue::setup_commands`, untouched by this ticket) for this
    /// command's target. Round-tripped as profile metadata only: wiring an
    /// override into the engine would mean changing `dialogue.rs`, which
    /// this ticket deliberately leaves alone. No shipped signal needs one.
    #[serde(
        deserialize_with = "hex_bytes_opt",
        rename = "flow_control_override_hex",
        default
    )]
    pub flow_control_override: Option<Vec<u8>>,
    /// A per-command read-timeout hint in milliseconds, for whatever
    /// drives the transport (the untouched engine's `on_timeout` is called
    /// by its caller, not scheduled by the engine itself). Metadata only,
    /// same status as `flow_control_override`.
    #[serde(default)]
    pub timeout_ms: Option<u32>,
    pub signals: Vec<DecodeRule>,
}

fn hex_u16<'de, D: Deserializer<'de>>(d: D) -> Result<u16, D::Error> {
    let s = String::deserialize(d)?;
    u16::from_str_radix(&s, 16).map_err(|e| D::Error::custom(format!("bad hex header {s:?}: {e}")))
}

fn parse_hex_bytes(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err(format!("hex string {s:?} has odd length"));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| format!("bad hex byte in {s:?}: {e}"))
        })
        .collect()
}

fn hex_bytes<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let s = String::deserialize(d)?;
    parse_hex_bytes(&s).map_err(D::Error::custom)
}

fn hex_bytes_opt<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<u8>>, D::Error> {
    let s: Option<String> = Option::deserialize(d)?;
    s.map(|s| parse_hex_bytes(&s).map_err(D::Error::custom))
        .transpose()
}

/// A loaded, validated `tprof-1` file.
#[derive(Debug, Clone, Deserialize)]
pub struct TelemetryProfile {
    pub format: String,
    pub id: String,
    pub name: String,
    /// Vehicle(s) this profile serves (free-form tags for a future
    /// profile-picker UI; not the Vehicle Model's drivetrain tags).
    pub vehicle_tags: Vec<String>,
    pub tier: ValidationTier,
    #[serde(default)]
    pub provenance: String,
    pub commands: Vec<CommandDef>,
    pub pack_variants: Vec<PackVariant>,
}

#[derive(Debug)]
pub enum ProfileError {
    Json(serde_json::Error),
    UnsupportedFormat(String),
    UnknownCanonicalSignal {
        command_id: String,
        signal_id: String,
        name: String,
    },
    OverlappingBitRange {
        command_id: String,
        a: String,
        b: String,
    },
    /// A rule maps a `map`-decoded (text) signal onto a canonical signal;
    /// canonical readings are always numeric (`decode::CanonicalReading`),
    /// so this would silently drop every reading at decode time -- rejected
    /// at load instead.
    CanonicalMapDecoded {
        command_id: String,
        signal_id: String,
    },
    MalformedBitRange {
        command_id: String,
        signal_id: String,
        reason: String,
    },
    DuplicatePackVariant(String),
    UnknownVariant(String),
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileError::Json(e) => write!(f, "telemetry profile json error: {e}"),
            ProfileError::UnsupportedFormat(got) => {
                write!(f, "unsupported telemetry profile format: {got}")
            }
            ProfileError::UnknownCanonicalSignal {
                command_id,
                signal_id,
                name,
            } => write!(
                f,
                "command {command_id}, signal {signal_id}: unknown canonical signal {name:?}"
            ),
            ProfileError::OverlappingBitRange { command_id, a, b } => write!(
                f,
                "command {command_id}: signals {a} and {b} have overlapping bit ranges"
            ),
            ProfileError::CanonicalMapDecoded {
                command_id,
                signal_id,
            } => write!(
                f,
                "command {command_id}, signal {signal_id}: a map-decoded signal cannot be canonical (canonical readings are always numeric)"
            ),
            ProfileError::MalformedBitRange {
                command_id,
                signal_id,
                reason,
            } => write!(f, "command {command_id}, signal {signal_id}: {reason}"),
            ProfileError::DuplicatePackVariant(id) => {
                write!(f, "duplicate pack variant id {id:?}")
            }
            ProfileError::UnknownVariant(id) => write!(f, "unknown pack variant {id:?}"),
        }
    }
}

impl std::error::Error for ProfileError {}

impl From<serde_json::Error> for ProfileError {
    fn from(e: serde_json::Error) -> Self {
        ProfileError::Json(e)
    }
}

impl TelemetryProfile {
    /// Parses and validates a `tprof-1` JSON document. Validation covers:
    /// unknown canonical signal names, overlapping or malformed bit ranges
    /// within a command, and duplicate pack-variant ids.
    pub fn load(json: &str) -> Result<TelemetryProfile, ProfileError> {
        let profile: TelemetryProfile = serde_json::from_str(json)?;
        profile.validate()?;
        Ok(profile)
    }

    /// Looks up a pack variant by id (ADR 0013's "unknown pack variant
    /// references" load-adjacent error: resolved once here, by whichever
    /// variant id the caller is about to poll/decode with).
    pub fn variant(&self, id: &str) -> Result<&PackVariant, ProfileError> {
        self.pack_variants
            .iter()
            .find(|v| v.id == id)
            .ok_or_else(|| ProfileError::UnknownVariant(id.to_string()))
    }

    fn command_id(cmd: &CommandDef) -> String {
        format!(
            "{:03X}.{:03X}.{}",
            cmd.tx_header,
            cmd.rx_header,
            cmd.request
                .iter()
                .map(|b| format!("{b:02X}"))
                .collect::<String>()
        )
    }

    fn validate(&self) -> Result<(), ProfileError> {
        if self.format != TPROF_FORMAT {
            return Err(ProfileError::UnsupportedFormat(self.format.clone()));
        }

        let mut seen_variants = std::collections::HashSet::new();
        for variant in &self.pack_variants {
            if !seen_variants.insert(variant.id.as_str()) {
                return Err(ProfileError::DuplicatePackVariant(variant.id.clone()));
            }
        }

        for cmd in &self.commands {
            let command_id = Self::command_id(cmd);
            let mut ranges: Vec<(u32, u32, &str)> = Vec::new();
            for rule in &cmd.signals {
                if let Some(name) = &rule.canonical {
                    if CanonicalSignal::parse(name).is_none() {
                        return Err(ProfileError::UnknownCanonicalSignal {
                            command_id,
                            signal_id: rule.id.clone(),
                            name: name.clone(),
                        });
                    }
                    if rule.map.is_some() {
                        return Err(ProfileError::CanonicalMapDecoded {
                            command_id,
                            signal_id: rule.id.clone(),
                        });
                    }
                }
                if rule.len == 0 || rule.len > 64 {
                    return Err(ProfileError::MalformedBitRange {
                        command_id,
                        signal_id: rule.id.clone(),
                        reason: format!("len must be 1..=64, got {}", rule.len),
                    });
                }
                if rule.div == 0.0 {
                    return Err(ProfileError::MalformedBitRange {
                        command_id,
                        signal_id: rule.id.clone(),
                        reason: "div must not be zero".to_string(),
                    });
                }
                let count = rule.count.unwrap_or(1);
                if count == 0 {
                    return Err(ProfileError::MalformedBitRange {
                        command_id,
                        signal_id: rule.id.clone(),
                        reason: "count must not be zero".to_string(),
                    });
                }
                // Checked, not wrapping: `load` is reachable from the FFI
                // with arbitrary JSON, and a wrapped `end` would panic in
                // debug builds / alias a valid-looking range in release.
                let Some(end) = rule
                    .len
                    .checked_mul(count)
                    .and_then(|span| rule.bix.checked_add(span))
                else {
                    return Err(ProfileError::MalformedBitRange {
                        command_id,
                        signal_id: rule.id.clone(),
                        reason: "bit range overflows".to_string(),
                    });
                };
                for &(other_start, other_end, other_id) in &ranges {
                    if rule.bix < other_end && other_start < end {
                        return Err(ProfileError::OverlappingBitRange {
                            command_id,
                            a: other_id.to_string(),
                            b: rule.id.clone(),
                        });
                    }
                }
                ranges.push((rule.bix, end, &rule.id));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal_json(extra_signal: &str) -> String {
        format!(
            r#"{{
  "format": "tprof-1",
  "id": "test-profile",
  "name": "Test Profile",
  "vehicle_tags": ["test_car"],
  "tier": "paper",
  "commands": [
    {{
      "tx_header": "7E4",
      "rx_header": "7EC",
      "request_hex": "220101",
      "freq_s": 1.0,
      "signals": [
        {{"id": "soc", "name": "SoC", "canonical": "display_soc", "bix": 32, "len": 8, "div": 2, "max": 100, "unit": "percent"}}
        {extra_signal}
      ]
    }}
  ],
  "pack_variants": [
    {{"id": "v1", "name": "V1", "populated_cells": 10, "usable_capacity_kwh": 10.0, "nominal_capacity_kwh": 10.0, "remaining_energy_ceiling_kwh": 10.0}}
  ]
}}"#
        )
    }

    #[test]
    fn engine_hints_default_to_absent_and_parse_when_present() {
        let profile = TelemetryProfile::load(&minimal_json("")).expect("valid profile");
        assert_eq!(profile.commands[0].flow_control_override, None);
        assert_eq!(profile.commands[0].timeout_ms, None);

        let json = minimal_json("").replace(
            "\"freq_s\": 1.0,",
            "\"freq_s\": 1.0, \"flow_control_override_hex\": \"300500\", \"timeout_ms\": 250,",
        );
        let profile = TelemetryProfile::load(&json).expect("valid profile with engine hints");
        assert_eq!(
            profile.commands[0].flow_control_override,
            Some(vec![0x30, 0x05, 0x00])
        );
        assert_eq!(profile.commands[0].timeout_ms, Some(250));
    }

    #[test]
    fn loads_a_minimal_valid_profile() {
        let profile = TelemetryProfile::load(&minimal_json("")).expect("valid profile");
        assert_eq!(profile.id, "test-profile");
        assert_eq!(profile.tier, ValidationTier::Paper);
        assert_eq!(profile.commands[0].tx_header, 0x7E4);
        assert_eq!(profile.commands[0].rx_header, 0x7EC);
        assert_eq!(profile.commands[0].request, vec![0x22, 0x01, 0x01]);
    }

    #[test]
    fn rejects_wrong_format() {
        let json = minimal_json("").replace("tprof-1", "tprof-2");
        assert!(matches!(
            TelemetryProfile::load(&json),
            Err(ProfileError::UnsupportedFormat(_))
        ));
    }

    #[test]
    fn rejects_unknown_canonical_signal() {
        let json = minimal_json("").replace("display_soc", "warp_factor");
        assert!(matches!(
            TelemetryProfile::load(&json),
            Err(ProfileError::UnknownCanonicalSignal { .. })
        ));
    }

    #[test]
    fn rejects_overlapping_bit_ranges() {
        let json = minimal_json(
            r#", {"id": "overlap", "name": "Overlap", "bix": 36, "len": 8, "max": 1, "unit": "scalar"}"#,
        );
        assert!(matches!(
            TelemetryProfile::load(&json),
            Err(ProfileError::OverlappingBitRange { .. })
        ));
    }

    #[test]
    fn adjacent_bit_ranges_do_not_overlap() {
        let json = minimal_json(
            r#", {"id": "adjacent", "name": "Adjacent", "bix": 40, "len": 8, "max": 1, "unit": "scalar"}"#,
        );
        assert!(TelemetryProfile::load(&json).is_ok());
    }

    #[test]
    fn rejects_zero_len() {
        let json = minimal_json("").replace("\"len\": 8", "\"len\": 0");
        assert!(matches!(
            TelemetryProfile::load(&json),
            Err(ProfileError::MalformedBitRange { .. })
        ));
    }

    #[test]
    fn rejects_a_canonical_map_decoded_signal() {
        let json = minimal_json(
            r#", {"id": "relay", "name": "Relay", "canonical": "soh", "bix": 64, "len": 1, "max": 1, "unit": "scalar", "map": {"1": "closed"}}"#,
        );
        assert!(matches!(
            TelemetryProfile::load(&json),
            Err(ProfileError::CanonicalMapDecoded { .. })
        ));
    }

    #[test]
    fn rejects_a_bit_range_that_overflows_u32() {
        // len 64 * count 2^26 = 2^32, one past u32::MAX.
        let json = minimal_json(
            r#", {"id": "huge", "name": "Huge", "bix": 64, "len": 64, "count": 67108864, "max": 1, "unit": "scalar"}"#,
        );
        assert!(matches!(
            TelemetryProfile::load(&json),
            Err(ProfileError::MalformedBitRange { .. })
        ));
    }

    #[test]
    fn rejects_zero_div() {
        let json = minimal_json(
            r#", {"id": "baddiv", "name": "Bad div", "bix": 64, "len": 8, "div": 0, "max": 1, "unit": "scalar"}"#,
        );
        assert!(matches!(
            TelemetryProfile::load(&json),
            Err(ProfileError::MalformedBitRange { .. })
        ));
    }

    #[test]
    fn rejects_duplicate_pack_variant_ids() {
        let json = minimal_json("").replace(
            r#""pack_variants": [
    {"id": "v1", "name": "V1", "populated_cells": 10, "usable_capacity_kwh": 10.0, "nominal_capacity_kwh": 10.0, "remaining_energy_ceiling_kwh": 10.0}
  ]"#,
            r#""pack_variants": [
    {"id": "v1", "name": "V1", "populated_cells": 10, "usable_capacity_kwh": 10.0, "nominal_capacity_kwh": 10.0, "remaining_energy_ceiling_kwh": 10.0},
    {"id": "v1", "name": "V1 dup", "populated_cells": 20, "usable_capacity_kwh": 20.0, "nominal_capacity_kwh": 20.0, "remaining_energy_ceiling_kwh": 20.0}
  ]"#,
        );
        assert!(matches!(
            TelemetryProfile::load(&json),
            Err(ProfileError::DuplicatePackVariant(_))
        ));
    }

    #[test]
    fn variant_lookup_errors_on_unknown_id() {
        let profile = TelemetryProfile::load(&minimal_json("")).expect("valid profile");
        assert!(matches!(
            profile.variant("does_not_exist"),
            Err(ProfileError::UnknownVariant(_))
        ));
        assert!(profile.variant("v1").is_ok());
    }

    #[test]
    fn hex_parsing_rejects_odd_length() {
        assert!(parse_hex_bytes("123").is_err());
    }

    #[test]
    fn hex_parsing_accepts_even_length_including_empty() {
        assert_eq!(parse_hex_bytes("").unwrap(), Vec::<u8>::new());
        assert_eq!(parse_hex_bytes("2201").unwrap(), vec![0x22, 0x01]);
    }
}
