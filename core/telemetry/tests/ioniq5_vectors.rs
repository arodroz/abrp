//! Vector validation for the first-party Ioniq 5 profile (wayfinder #77,
//! ADR 0013 point 5): feeds every imported OBDb recorded response through
//! ISO-TP reassembly (`telemetry::isotp`, reusing the untouched #76 engine
//! pieces) and this ticket's decode path, and asserts the decoded values
//! match OBDb's own recorded `expected_values` for the signals this
//! profile implements. This is what backs the profile's `tier:
//! "vector-validated"` claim -- if any assertion here fails, that claim is
//! false and the JSON must say so.
//!
//! Shape of an imported fixture (see `data/imported/obdb/NOTICE.md` and
//! `src/bin/import_obdb_vectors.rs`): one JSON file per OBDb command,
//! holding the source repo/commit and a list of cases, each a **list of
//! raw CAN frame lines** (`"<3-hex-id> <hex data bytes>"`, exactly the
//! shape `crate::elm::classify_line` parses) plus OBDb's own
//! `expected_values` map, keyed by OBDb's native signal names.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

use telemetry::elm::{classify_line, Line};
use telemetry::isotp::Reassembler;
use telemetry::{CanonicalSignal, DecodeOutcome, DecodedValue, EcuTarget, TelemetryProfile};

const IONIQ5_JSON: &str = include_str!("../../../data/profiles/hyundai-ioniq5.tprof.json");

#[derive(Debug, Deserialize)]
struct ImportedFile {
    tx_header: String,
    rx_header: String,
    cases: Vec<ImportedCase>,
}

#[derive(Debug, Deserialize)]
struct ImportedCase {
    response_lines: Vec<String>,
    expected_values: BTreeMap<String, f64>,
}

fn load_imported(command_file: &str) -> ImportedFile {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../data/imported/obdb/hyundai-ioniq5")
        .join(command_file);
    let json = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {path:?}: {e}"));
    serde_json::from_str(&json).unwrap_or_else(|e| panic!("parsing {path:?}: {e}"))
}

/// Reassembles one case's raw frame lines (any foreign-id lines are
/// ignored, exactly as the real engine does) into the completed UDS
/// payload.
fn reassemble(lines: &[String], rx_header: u16) -> Vec<u8> {
    let mut reassembler = Reassembler::new();
    for line in lines {
        if let Line::Frame(frame) = classify_line(line, None) {
            if frame.id == rx_header {
                if let Ok(Some(payload)) = reassembler.feed_frame(&frame.data) {
                    return payload;
                }
            }
        }
    }
    panic!("case's frame lines never reassembled into a complete payload: {lines:?}");
}

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() < 0.01
}

/// OBDb's `expected_values` for the four cumulative counters are raw u32
/// counts -- their signalset carries no scale for these fields, while the
/// guide's triangulated sources (OVMS + EVNotify + Torque, "Confirmed"
/// tier, `docs/research/ioniq5-obd-telemetry.md` §3) all divide by 10 to
/// get Ah/kWh, and the profile decodes that physical unit (see its
/// provenance). Extraction is therefore validated against `expected / 10`
/// for exactly these fields: still vector-exact, with the unit claim
/// attributed to its real source. A vector can prove bit extraction, never
/// a unit scale -- see `profile.rs`'s module docs.
fn obdb_expected_scale(obdb_name: &str) -> f64 {
    match obdb_name {
        "IONIQ5_CUMULATIVE_CHARGE_CURRENT"
        | "IONIQ5_CUMULATIVE_DISCHARGE_CURRENT"
        | "IONIQ5_CUMULATIVE_ENERGY_CHARGED"
        | "IONIQ5_CUMULATIVE_ENERGY_DISCHARGED" => 0.1,
        _ => 1.0,
    }
}

enum Lookup {
    Canonical(CanonicalSignal, Option<u32>),
    Extra(&'static str),
}

struct FieldCheck {
    obdb_name: &'static str,
    lookup: Lookup,
}

fn canonical_value(
    outcome: &DecodeOutcome,
    signal: CanonicalSignal,
    index: Option<u32>,
) -> Option<f64> {
    outcome
        .canonical
        .iter()
        .find(|r| r.signal == signal && r.index == index)
        .map(|r| r.value)
}

fn extra_value(outcome: &DecodeOutcome, name: &str) -> Option<f64> {
    outcome
        .extras
        .iter()
        .find(|r| r.name == name)
        .and_then(|r| match r.value {
            DecodedValue::Number(v) => Some(v),
            DecodedValue::Text(_) => None,
        })
}

/// Runs every case in `command_file` through reassembly + decode and
/// checks every entry in `checks` against OBDb's `expected_values`. Every
/// `checks` entry must be present in every case's `expected_values` --
/// this catches an importer/test drift as loudly as a wrong decoded value.
fn run_checks(command_file: &str, variant_id: &str, checks: &[FieldCheck]) {
    let profile = TelemetryProfile::load(IONIQ5_JSON).expect("shipped Ioniq 5 profile is valid");
    let variant = profile.variant(variant_id).expect("variant exists");
    let imported = load_imported(command_file);
    let target = EcuTarget {
        tx_header: u16::from_str_radix(&imported.tx_header, 16).unwrap(),
        rx_header: u16::from_str_radix(&imported.rx_header, 16).unwrap(),
    };

    for (case_i, case) in imported.cases.iter().enumerate() {
        let payload = reassemble(&case.response_lines, target.rx_header);
        let outcome = telemetry::decode(&profile, variant, target, &payload)
            .unwrap_or_else(|e| panic!("{command_file} case {case_i}: decode error: {e}"));

        for check in checks {
            let expected = *case
                .expected_values
                .get(check.obdb_name)
                .unwrap_or_else(|| {
                    panic!(
                        "{command_file} case {case_i}: fixture has no expected value for {}",
                        check.obdb_name
                    )
                })
                * obdb_expected_scale(check.obdb_name);
            let actual = match check.lookup {
                Lookup::Canonical(signal, index) => canonical_value(&outcome, signal, index),
                Lookup::Extra(name) => extra_value(&outcome, name),
            };
            let actual = actual.unwrap_or_else(|| {
                panic!(
                    "{command_file} case {case_i}: no decoded value for OBDb signal {}",
                    check.obdb_name
                )
            });
            assert!(
                approx_eq(actual, expected),
                "{command_file} case {case_i}: {} expected {expected}, decoded {actual}",
                check.obdb_name
            );
        }
    }
}

#[test]
fn command_220101_bms_workhorse_frame() {
    use CanonicalSignal::*;
    use Lookup::*;
    let checks = [
        FieldCheck {
            obdb_name: "IONIQ5_HVBAT_SOC",
            lookup: Canonical(DisplaySoc, None),
        },
        FieldCheck {
            obdb_name: "IONIQ5_BATTERY_CURRENT",
            lookup: Canonical(PackCurrent, None),
        },
        FieldCheck {
            obdb_name: "IONIQ5_BATTERY_MAX_T",
            lookup: Extra("battery_max_t"),
        },
        FieldCheck {
            obdb_name: "IONIQ5_BATTERY_MIN_T",
            lookup: Extra("battery_min_t"),
        },
        FieldCheck {
            obdb_name: "IONIQ5_BATTERY_PACK_B01",
            lookup: Canonical(ModuleTemperature, Some(1)),
        },
        FieldCheck {
            obdb_name: "IONIQ5_BATTERY_PACK_B02",
            lookup: Canonical(ModuleTemperature, Some(2)),
        },
        FieldCheck {
            obdb_name: "IONIQ5_BATTERY_PACK_B03",
            lookup: Canonical(ModuleTemperature, Some(3)),
        },
        FieldCheck {
            obdb_name: "IONIQ5_BATTERY_PACK_B04",
            lookup: Canonical(ModuleTemperature, Some(4)),
        },
        FieldCheck {
            obdb_name: "IONIQ5_BATTERY_PACK_B05",
            lookup: Canonical(ModuleTemperature, Some(5)),
        },
        FieldCheck {
            obdb_name: "IONIQ5_MAXIMUM_CELL_VOLTAGE",
            lookup: Extra("maximum_cell_voltage"),
        },
        FieldCheck {
            obdb_name: "IONIQ5_MAXIMUM_CELL_VOLTAGE_NO",
            lookup: Extra("maximum_cell_voltage_no"),
        },
        FieldCheck {
            obdb_name: "IONIQ5_MINIMUM_CELL_VOLTAGE",
            lookup: Extra("minimum_cell_voltage"),
        },
        FieldCheck {
            obdb_name: "IONIQ5_MINIMUM_CELL_VOLTAGE_NO",
            lookup: Extra("minimum_cell_voltage_no"),
        },
        FieldCheck {
            obdb_name: "IONIQ5_AUXILLARY_BATTERY_VOLTAGE",
            lookup: Canonical(Aux12vVoltage, None),
        },
        FieldCheck {
            obdb_name: "IONIQ5_CUMULATIVE_CHARGE_CURRENT",
            lookup: Canonical(CumulativeChargeAh, None),
        },
        FieldCheck {
            obdb_name: "IONIQ5_CUMULATIVE_DISCHARGE_CURRENT",
            lookup: Canonical(CumulativeDischargeAh, None),
        },
        FieldCheck {
            obdb_name: "IONIQ5_CUMULATIVE_ENERGY_CHARGED",
            lookup: Canonical(CumulativeChargeEnergy, None),
        },
        FieldCheck {
            obdb_name: "IONIQ5_CUMULATIVE_ENERGY_DISCHARGED",
            lookup: Canonical(CumulativeDischargeEnergy, None),
        },
        FieldCheck {
            obdb_name: "IONIQ5_BATTERY_WORK_TIME_TOTAL_SEC",
            lookup: Extra("battery_work_time_total_sec"),
        },
    ];
    run_checks("7E4.7EC.220101.json", "72_6_kwh", &checks);
}

#[test]
fn command_220105_soh_and_remaining_energy() {
    use CanonicalSignal::*;
    use Lookup::*;
    let checks = [
        FieldCheck {
            obdb_name: "IONIQ5_HVBAT_SOH",
            lookup: Canonical(Soh, None),
        },
        FieldCheck {
            obdb_name: "IONIQ5_HVBAT_WH",
            lookup: Canonical(RemainingEnergy, None),
        },
        FieldCheck {
            obdb_name: "IONIQ5_HVBAT_SOC_DISP",
            lookup: Extra("hvbat_soc_disp"),
        },
    ];
    run_checks("7E4.7EC.220105.json", "72_6_kwh", &checks);
}

#[test]
fn command_220111_high_module_temps() {
    use CanonicalSignal::*;
    use Lookup::*;
    let checks = [
        FieldCheck {
            obdb_name: "IONIQ5_BATTERY_PACK_B17",
            lookup: Canonical(ModuleTemperature, Some(17)),
        },
        FieldCheck {
            obdb_name: "IONIQ5_BATTERY_PACK_B18",
            lookup: Canonical(ModuleTemperature, Some(18)),
        },
    ];
    run_checks("7E4.7EC.220111.json", "72_6_kwh", &checks);
}

#[test]
fn command_b002_odometer() {
    use CanonicalSignal::*;
    use Lookup::*;
    let checks = [
        FieldCheck {
            obdb_name: "IONIQ5_ODO_KM",
            lookup: Canonical(Odometer, None),
        },
        FieldCheck {
            obdb_name: "IONIQ5_ODO_MI",
            lookup: Extra("odo_mi"),
        },
    ];
    run_checks("7C6.7CE.22B002.json", "72_6_kwh", &checks);
}

#[test]
fn command_e011_iccu_and_aux_battery() {
    use CanonicalSignal::*;
    use Lookup::*;
    let checks = [
        FieldCheck {
            obdb_name: "IONIQ5_LDC_T",
            lookup: Extra("ldc_t"),
        },
        FieldCheck {
            obdb_name: "IONIQ5_LDC_OUTPUT_VOLTAGE",
            lookup: Extra("ldc_output_voltage"),
        },
        FieldCheck {
            obdb_name: "IONIQ5_LDC_OUTPUT_CURRENT",
            lookup: Extra("ldc_output_current"),
        },
        FieldCheck {
            obdb_name: "IONIQ5_LDC_INPUT_VOLTAGE",
            lookup: Canonical(PackVoltage, None),
        },
        FieldCheck {
            obdb_name: "IONIQ5_AUX_BATTERY_CURRENT",
            lookup: Extra("aux_battery_current"),
        },
        FieldCheck {
            obdb_name: "IONIQ5_AUX_BATTERY_STATE_OF_CHARGE",
            lookup: Extra("aux_battery_soc"),
        },
        FieldCheck {
            obdb_name: "IONIQ5_AUX_BATTERY_VOLTAGE_7E5",
            lookup: Extra("aux_battery_voltage_iccu"),
        },
        FieldCheck {
            obdb_name: "IONIQ5_AUX_BATTERY_T",
            lookup: Extra("aux_battery_t"),
        },
    ];
    run_checks("7E5.7ED.22E011.json", "72_6_kwh", &checks);
}

/// Cell-voltage commands: OBDb keys every cell as its own
/// `IONIQ5_HVBAT_CMU###_VOLT` signal, so these are checked generically by
/// parsing the cell number out of the key rather than a hand-written
/// table. Run twice: once at the 72.6 kWh variant (the ticket's headline
/// truncation case -- cells beyond 180 must not appear at all) and once
/// at 77.4 kWh (192 populated cells, so every recorded cell in these
/// vectors is checked against the decoder with no truncation in the way).
fn check_cell_command(command_file: &str, variant_id: &str, populated_cells: u32) {
    let profile = TelemetryProfile::load(IONIQ5_JSON).expect("shipped Ioniq 5 profile is valid");
    let variant = profile.variant(variant_id).expect("variant exists");
    let imported = load_imported(command_file);
    let target = EcuTarget {
        tx_header: u16::from_str_radix(&imported.tx_header, 16).unwrap(),
        rx_header: u16::from_str_radix(&imported.rx_header, 16).unwrap(),
    };

    for (case_i, case) in imported.cases.iter().enumerate() {
        let payload = reassemble(&case.response_lines, target.rx_header);
        let outcome = telemetry::decode(&profile, variant, target, &payload)
            .unwrap_or_else(|e| panic!("{command_file} case {case_i}: decode error: {e}"));

        let mut checked = 0;
        for (key, &expected) in &case.expected_values {
            let Some(cell_no) = key
                .strip_prefix("IONIQ5_HVBAT_CMU")
                .and_then(|s| s.strip_suffix("_VOLT"))
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            let actual = canonical_value(&outcome, CanonicalSignal::CellVoltage, Some(cell_no));
            if cell_no > populated_cells {
                assert!(
                    actual.is_none(),
                    "{command_file} case {case_i}: cell {cell_no} should be truncated at {populated_cells} populated cells"
                );
                continue;
            }
            let actual = actual.unwrap_or_else(|| {
                panic!("{command_file} case {case_i}: no decoded value for cell {cell_no}")
            });
            assert!(
                approx_eq(actual, expected),
                "{command_file} case {case_i}: cell {cell_no} expected {expected}, decoded {actual}"
            );
            checked += 1;
        }
        assert!(
            checked > 0,
            "{command_file} case {case_i}: no CMU keys matched"
        );
    }
}

#[test]
fn cell_voltages_truncate_at_180_populated_cells_on_the_72_6_kwh_pack() {
    for file in [
        "7E4.7EC.220102.json",
        "7E4.7EC.220103.json",
        "7E4.7EC.220104.json",
        "7E4.7EC.22010A.json",
        "7E4.7EC.22010B.json",
        "7E4.7EC.22010C.json",
    ] {
        check_cell_command(file, "72_6_kwh", 180);
    }
}

#[test]
fn cell_voltages_decode_the_full_192_slots_on_the_77_4_kwh_pack() {
    for file in [
        "7E4.7EC.220102.json",
        "7E4.7EC.220103.json",
        "7E4.7EC.220104.json",
        "7E4.7EC.22010A.json",
        "7E4.7EC.22010B.json",
        "7E4.7EC.22010C.json",
    ] {
        check_cell_command(file, "77_4_kwh", 192);
    }
}
