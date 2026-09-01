//! The runtime path over a loaded [`TelemetryProfile`] (wayfinder #77): (1)
//! [`requests_for`] turns a profile + pack variant into the engine's poll
//! list; (2) [`decode`] takes one completed [`Event::Payload`](crate::Event)
//! and extracts every signal its command defines, applying scaling/sign/
//! map/null rules and pack-variant cell truncation, into canonical
//! readings plus raw named extras. See `profile.rs` for the resolved
//! bit-index origin convention these primitives implement.

use std::fmt;

use crate::dialogue::{EcuTarget, Request};
use crate::profile::{CanonicalSignal, CommandDef, DecodeRule, PackVariant, TelemetryProfile};

/// A decoded value: numeric (the common case) or text (an OBDb `map`
/// enumeration lookup).
#[derive(Debug, Clone, PartialEq)]
pub enum DecodedValue {
    Number(f64),
    Text(String),
}

/// One canonical-signal reading. `index` is the 1-based cell/module number
/// for [`CanonicalSignal::CellVoltage`]/[`CanonicalSignal::ModuleTemperature`],
/// `None` for every other (scalar) signal. Always numeric: no shipped
/// canonical signal is `map`-decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct CanonicalReading {
    pub signal: CanonicalSignal,
    pub index: Option<u32>,
    pub value: f64,
    pub unit: String,
}

/// One raw native-signal reading that isn't mapped onto a
/// [`CanonicalSignal`] -- never dropped (ADR 0013 point 3).
#[derive(Debug, Clone, PartialEq)]
pub struct ExtraReading {
    pub name: String,
    pub index: Option<u32>,
    pub value: DecodedValue,
    pub unit: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DecodeOutcome {
    pub canonical: Vec<CanonicalReading>,
    pub extras: Vec<ExtraReading>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The response's target/echoed prefix doesn't match any command this
    /// profile defines.
    UnknownCommand { tx_header: u16, rx_header: u16 },
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::UnknownCommand {
                tx_header,
                rx_header,
            } => write!(
                f,
                "no command defined for target {tx_header:03X}/{rx_header:03X} matching this response"
            ),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Turns `profile` into the engine's poll list for `variant`: one
/// [`Request`] per relevant command, in profile order, with each target's
/// session-prerequisite request (if any) inserted once before that
/// target's first command that needs it. A command whose *every* signal is
/// a [`CanonicalSignal::CellVoltage`] rule entirely beyond `variant`'s
/// `populated_cells` is skipped outright (no point spending bus time/12V
/// budget on slots the pack doesn't have -- `docs/research/
/// ioniq5-obd-telemetry.md` §6's 12V-drain warning).
pub fn requests_for(profile: &TelemetryProfile, variant: &PackVariant) -> Vec<Request> {
    // A plain `Vec` rather than a `HashSet`: `EcuTarget` (`dialogue.rs`,
    // untouched by this ticket) doesn't derive `Hash`, and the handful of
    // distinct ECU targets in any real profile makes linear lookup here
    // immaterial.
    let mut session_sent: Vec<EcuTarget> = Vec::new();
    let mut out = Vec::new();
    for cmd in &profile.commands {
        if !command_relevant(cmd, variant) {
            continue;
        }
        let target = EcuTarget {
            tx_header: cmd.tx_header,
            rx_header: cmd.rx_header,
        };
        if let Some(session) = &cmd.session_prerequisite {
            if !session_sent.contains(&target) {
                session_sent.push(target);
                out.push(Request {
                    target,
                    uds: session.clone(),
                });
            }
        }
        out.push(Request {
            target,
            uds: cmd.request.clone(),
        });
    }
    out
}

fn command_relevant(cmd: &CommandDef, variant: &PackVariant) -> bool {
    if cmd.signals.is_empty() {
        return true;
    }
    cmd.signals.iter().any(|rule| {
        let is_cell_voltage = rule.canonical.as_deref().and_then(CanonicalSignal::parse)
            == Some(CanonicalSignal::CellVoltage);
        !is_cell_voltage || rule.first_index <= variant.populated_cells
    })
}

/// Decodes one completed `Payload{target, uds}` (`uds` is the full
/// positive-response payload, `0x62`/echoed-DID prefix included). Returns
/// an empty [`DecodeOutcome`] (not an error) for a session-prerequisite
/// acknowledgement -- nothing to decode there, but it isn't a failure
/// either. `variant` gates [`CanonicalSignal::CellVoltage`] truncation:
/// readings beyond `variant.populated_cells` are dropped rather than
/// reported at 0 V.
pub fn decode(
    profile: &TelemetryProfile,
    variant: &PackVariant,
    target: EcuTarget,
    uds: &[u8],
) -> Result<DecodeOutcome, DecodeError> {
    if is_session_ack(profile, target, uds) {
        return Ok(DecodeOutcome::default());
    }
    let cmd = find_command(profile, target, uds).ok_or(DecodeError::UnknownCommand {
        tx_header: target.tx_header,
        rx_header: target.rx_header,
    })?;
    let data = &uds[cmd.request.len()..];

    let mut outcome = DecodeOutcome::default();
    for rule in &cmd.signals {
        for (index, value) in decode_rule(data, rule) {
            match &rule.canonical {
                Some(name) => {
                    // Already checked against `CanonicalSignal::parse` at
                    // profile load time (`TelemetryProfile::validate`).
                    let signal = CanonicalSignal::parse(name).expect("validated canonical name");
                    if signal == CanonicalSignal::CellVoltage
                        && index.is_some_and(|i| i > variant.populated_cells)
                    {
                        continue;
                    }
                    if let DecodedValue::Number(value) = value {
                        outcome.canonical.push(CanonicalReading {
                            signal,
                            index,
                            value,
                            unit: rule.unit.clone(),
                        });
                    }
                }
                None => outcome.extras.push(ExtraReading {
                    name: rule.id.clone(),
                    index,
                    value,
                    unit: rule.unit.clone(),
                }),
            }
        }
    }
    Ok(outcome)
}

fn echoes(response: &[u8], request: &[u8]) -> bool {
    !request.is_empty()
        && response.len() >= request.len()
        && response[0] == request[0].wrapping_add(0x40)
        && response[1..request.len()] == request[1..]
}

fn find_command<'a>(
    profile: &'a TelemetryProfile,
    target: EcuTarget,
    uds: &[u8],
) -> Option<&'a CommandDef> {
    profile.commands.iter().find(|cmd| {
        cmd.tx_header == target.tx_header
            && cmd.rx_header == target.rx_header
            && echoes(uds, &cmd.request)
    })
}

fn is_session_ack(profile: &TelemetryProfile, target: EcuTarget, uds: &[u8]) -> bool {
    profile.commands.iter().any(|cmd| {
        cmd.tx_header == target.tx_header
            && cmd.rx_header == target.rx_header
            && cmd
                .session_prerequisite
                .as_ref()
                .is_some_and(|session| echoes(uds, session))
    })
}

/// Produces every reading `rule` yields from `data` (the payload with the
/// echoed request prefix already stripped -- bit 0 of `rule.bix` is bit 0
/// of `data`). A `count > 1` rule yields up to `count` readings, indexed
/// from `first_index`; a reading whose bit range runs past the end of
/// `data` is silently omitted (a short/truncated response drops that one
/// signal, not the whole command). A `map` rule with no matching key is
/// likewise omitted, mirroring OBDb's own reference decoder. A value
/// falling inside `[nullmin, nullmax]` is omitted (the signal reads as
/// absent); one outside `[omin, omax]` (when all three of `omin`/`omax`/
/// `oval` are set) is reported as `oval` instead.
fn decode_rule(data: &[u8], rule: &DecodeRule) -> Vec<(Option<u32>, DecodedValue)> {
    let count = rule.count.unwrap_or(1);
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let bix = rule.bix + i * rule.len;
        let Some(raw) = extract_bits(data, bix, rule.len, rule.blsb) else {
            continue;
        };
        let index = rule.count.map(|_| rule.first_index + i);

        if let Some(map) = &rule.map {
            if let Some(label) = map.get(&raw.to_string()) {
                out.push((index, DecodedValue::Text(label.clone())));
            }
            continue;
        }

        let signed = if rule.sign {
            sign_extend(raw, rule.len) as f64
        } else {
            raw as f64
        };
        let scaled = signed * rule.mul / rule.div + rule.add;

        if let (Some(lo), Some(hi)) = (rule.nullmin, rule.nullmax) {
            if scaled >= lo && scaled <= hi {
                continue;
            }
        }
        let overridden = match (rule.omin, rule.omax, rule.oval) {
            (Some(lo), Some(hi), Some(oval)) if scaled < lo || scaled > hi => oval,
            _ => scaled,
        };
        let value = if rule.max > rule.min {
            overridden.clamp(rule.min, rule.max)
        } else {
            overridden
        };
        out.push((index, DecodedValue::Number(value)));
    }
    out
}

/// Extracts `len` bits starting at bit `bix` (MSB-first within each byte,
/// big-endian across bytes -- OBDb's convention), `None` if `data` is too
/// short. `blsb` reverses the covered byte span first (a little-endian
/// multi-byte field); mirrors `.schemas/python/can/signals.py`'s
/// `_extract_bits`.
fn extract_bits(data: &[u8], bix: u32, len: u32, blsb: bool) -> Option<u64> {
    let start = bix as usize;
    let end = start + len as usize;
    if len == 0 || len > 64 || end > data.len() * 8 {
        return None;
    }

    let reversed;
    let bytes: &[u8] = if blsb && len > 8 {
        let start_byte = start / 8;
        let byte_count = (len as usize).div_ceil(8);
        let end_byte = (start_byte + byte_count).min(data.len());
        let mut buf = data.to_vec();
        buf[start_byte..end_byte].reverse();
        reversed = buf;
        &reversed
    } else {
        data
    };

    let mut result: u64 = 0;
    for i in start..end {
        let byte_idx = i / 8;
        let bit_idx = 7 - (i % 8);
        let bit = (bytes[byte_idx] >> bit_idx) & 1;
        result = (result << 1) | u64::from(bit);
    }
    Some(result)
}

/// Two's-complement sign extension of the low `len` bits of `raw` into an
/// `i64`.
fn sign_extend(raw: u64, len: u32) -> i64 {
    let shift = 64 - len;
    ((raw << shift) as i64) >> shift
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{PackVariant, ValidationTier};
    use std::collections::BTreeMap;

    fn rule(id: &str, bix: u32, len: u32) -> DecodeRule {
        DecodeRule {
            id: id.to_string(),
            name: id.to_string(),
            canonical: None,
            bix,
            len,
            blsb: false,
            sign: false,
            add: 0.0,
            mul: 1.0,
            div: 1.0,
            min: 0.0,
            max: 0.0,
            unit: "scalar".to_string(),
            nullmin: None,
            nullmax: None,
            omin: None,
            omax: None,
            oval: None,
            map: None,
            count: None,
            first_index: 1,
        }
    }

    // --- extract_bits / bix/len edge cases ---

    #[test]
    fn extract_bits_reads_msb_first_within_a_byte() {
        // 0b1011_0000 -> top 4 bits = 0b1011 = 11.
        assert_eq!(extract_bits(&[0b1011_0000], 0, 4, false), Some(11));
    }

    #[test]
    fn extract_bits_spans_a_byte_boundary() {
        // bits 4..12 of [0xF0, 0x0F] = low nibble of byte0 (0) ++ high nibble of byte1 (0) = 0.
        // bits 4..12 of [0x0F, 0xF0] = 0xFF.
        assert_eq!(extract_bits(&[0x0F, 0xF0], 4, 8, false), Some(0xFF));
    }

    #[test]
    fn extract_bits_none_when_range_exceeds_data() {
        assert_eq!(extract_bits(&[0x00], 4, 8, false), None);
    }

    #[test]
    fn extract_bits_none_for_zero_or_over_64_len() {
        assert_eq!(extract_bits(&[0xFF; 9], 0, 0, false), None);
        assert_eq!(extract_bits(&[0xFF; 9], 0, 65, false), None);
    }

    #[test]
    fn extract_bits_exact_64_bit_width() {
        let data = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        assert_eq!(
            extract_bits(&data, 0, 64, false),
            Some(0x0102_0304_0506_0708)
        );
    }

    #[test]
    fn extract_bits_blsb_reverses_the_covered_bytes_only() {
        // Two u16 fields back to back; blsb reverses only the first one's bytes.
        let data = [0x01, 0x02, 0xAA, 0xBB];
        assert_eq!(extract_bits(&data, 0, 16, true), Some(0x0201));
        assert_eq!(extract_bits(&data, 16, 16, false), Some(0xAABB));
    }

    // --- signedness ---

    #[test]
    fn sign_extend_negative_8_bit() {
        assert_eq!(sign_extend(0xFF, 8), -1);
        assert_eq!(sign_extend(0x80, 8), -128);
        assert_eq!(sign_extend(0x7F, 8), 127);
    }

    #[test]
    fn sign_extend_negative_16_bit_matches_confirmed_vector() {
        // 0xFF64 as a signed 16-bit value, /10 -> -15.6 A (wayfinder #77
        // adjudication: this exact raw value came from a real OBDb Ioniq 5
        // vector for IONIQ5_BATTERY_CURRENT).
        assert_eq!(sign_extend(0xFF64, 16), -156);
    }

    #[test]
    fn decode_rule_applies_sign_before_scaling() {
        let mut r = rule("current", 0, 16);
        r.sign = true;
        r.div = 10.0;
        r.min = -1000.0;
        r.max = 1000.0;
        let out = decode_rule(&[0xFF, 0x64], &r);
        assert_eq!(out, vec![(None, DecodedValue::Number(-15.6))]);
    }

    // --- scaling composition order: (raw * mul / div) + add, then clamp ---

    #[test]
    fn decode_rule_scaling_order_is_mul_div_then_add_then_clamp() {
        let mut r = rule("t", 0, 8);
        r.mul = 2.0;
        r.div = 4.0;
        r.add = -40.0;
        r.min = -40.0;
        r.max = 80.0;
        // raw 200: 200*2/4 - 40 = 100 - 40 = 60 (within range, unclamped).
        assert_eq!(
            decode_rule(&[200], &r),
            vec![(None, DecodedValue::Number(60.0))]
        );
    }

    #[test]
    fn decode_rule_clamps_to_max_when_max_greater_than_min() {
        let mut r = rule("v", 0, 16);
        r.min = 268.8;
        r.max = 403.2;
        // No div declared (defaults to 1): a large raw value clamps to max,
        // mirroring OBDb's own reference decoder (`Scaling.decode_value`)
        // and the (broken, unscaled) IONIQ5_BATTERY_DC_VOLTAGE field this
        // ticket's report flags -- see profile.rs module docs.
        assert_eq!(
            decode_rule(&[0x1D, 0x3E], &r),
            vec![(None, DecodedValue::Number(403.2))]
        );
    }

    #[test]
    fn decode_rule_does_not_clamp_when_max_is_not_greater_than_min() {
        let r = rule("counter", 0, 8); // min=max=0.0 default -> no clamp.
        assert_eq!(
            decode_rule(&[255], &r),
            vec![(None, DecodedValue::Number(255.0))]
        );
    }

    // --- null / override ranges ---

    #[test]
    fn decode_rule_null_range_omits_the_reading() {
        let mut r = rule("maybe_absent", 0, 8);
        r.nullmin = Some(254.0);
        r.nullmax = Some(255.0);
        assert_eq!(decode_rule(&[255], &r), vec![]);
        assert_eq!(
            decode_rule(&[10], &r),
            vec![(None, DecodedValue::Number(10.0))]
        );
    }

    #[test]
    fn decode_rule_override_range_substitutes_oval_outside_bounds() {
        let mut r = rule("sensor", 0, 8);
        r.omin = Some(0.0);
        r.omax = Some(100.0);
        r.oval = Some(-1.0);
        assert_eq!(
            decode_rule(&[200], &r),
            vec![(None, DecodedValue::Number(-1.0))]
        );
        assert_eq!(
            decode_rule(&[50], &r),
            vec![(None, DecodedValue::Number(50.0))]
        );
    }

    // --- map lookups ---

    #[test]
    fn decode_rule_map_lookup_hit_and_miss() {
        // Only "1" is mapped, so raw 0 is a genuine miss (not just an
        // unmapped-but-valid-looking value).
        let mut r = rule("relay", 0, 1);
        let mut map = BTreeMap::new();
        map.insert("1".to_string(), "closed".to_string());
        r.map = Some(map);
        assert_eq!(
            decode_rule(&[0b1000_0000], &r),
            vec![(None, DecodedValue::Text("closed".to_string()))]
        );
        r.bix = 1;
        assert_eq!(decode_rule(&[0b1000_0000], &r), vec![]);
    }

    // --- repeated (count/first_index) signals ---

    #[test]
    fn decode_rule_repeats_count_times_indexed_from_first_index() {
        let mut r = rule("cell", 0, 8);
        r.div = 50.0;
        r.max = 4.2;
        r.count = Some(3);
        r.first_index = 97;
        let data = [181u8, 190, 0];
        assert_eq!(
            decode_rule(&data, &r),
            vec![
                (Some(97), DecodedValue::Number(3.62)),
                (Some(98), DecodedValue::Number(3.8)),
                (Some(99), DecodedValue::Number(0.0)),
            ]
        );
    }

    #[test]
    fn decode_rule_short_data_omits_only_the_overrunning_reading() {
        let mut r = rule("cell", 0, 8);
        r.count = Some(2);
        assert_eq!(
            decode_rule(&[42], &r),
            vec![(Some(1), DecodedValue::Number(42.0))]
        );
    }

    // --- pack truncation, requests_for, and end-to-end decode ---

    fn variant(populated_cells: u32) -> PackVariant {
        PackVariant {
            id: "v".to_string(),
            name: "V".to_string(),
            populated_cells,
            usable_capacity_kwh: 72.6,
            nominal_capacity_kwh: 72.6,
            remaining_energy_ceiling_kwh: 74.0,
            notes: String::new(),
        }
    }

    fn cell_command(uds_did: u8, first_index: u32) -> CommandDef {
        let mut r = rule("cell", 32, 8);
        r.canonical = Some("cell_voltage".to_string());
        r.div = 50.0;
        r.max = 4.2;
        r.count = Some(32);
        r.first_index = first_index;
        CommandDef {
            tx_header: 0x7E4,
            rx_header: 0x7EC,
            request: vec![0x22, 0x01, uds_did],
            session_prerequisite: None,
            freq_s: 1.0,
            flow_control_override: None,
            timeout_ms: None,
            signals: vec![r],
        }
    }

    fn iccu_command() -> CommandDef {
        let mut r = rule("pack_voltage", 112, 16);
        r.canonical = Some("pack_voltage".to_string());
        r.div = 10.0;
        r.max = 6553.5;
        CommandDef {
            tx_header: 0x7E5,
            rx_header: 0x7ED,
            request: vec![0x22, 0xE0, 0x11],
            session_prerequisite: Some(vec![0x10, 0x03]),
            freq_s: 1.0,
            flow_control_override: None,
            timeout_ms: None,
            signals: vec![r],
        }
    }

    fn test_profile() -> TelemetryProfile {
        TelemetryProfile {
            format: "tprof-1".to_string(),
            id: "test".to_string(),
            name: "Test".to_string(),
            vehicle_tags: vec!["test_car".to_string()],
            tier: ValidationTier::Paper,
            provenance: String::new(),
            commands: vec![
                cell_command(0x02, 1),
                cell_command(0x0C, 161),
                iccu_command(),
            ],
            pack_variants: vec![variant(180)],
        }
    }

    #[test]
    fn requests_for_orders_session_prerequisite_once_before_its_command() {
        let profile = test_profile();
        let v = variant(180);
        let requests = requests_for(&profile, &v);
        // cell_command(0x02) has no session; iccu_command needs one 10 03
        // exactly once, immediately before its own 22 E0 11.
        let last_two: Vec<_> = requests.iter().rev().take(2).collect();
        assert_eq!(last_two[1].uds, vec![0x10, 0x03]);
        assert_eq!(last_two[0].uds, vec![0x22, 0xE0, 0x11]);
        assert_eq!(requests.iter().filter(|r| r.uds == [0x10, 0x03]).count(), 1);
    }

    #[test]
    fn requests_for_skips_a_command_wholly_beyond_populated_cells() {
        // 144 kWh-58 style variant: cells 161-192 are entirely unpopulated.
        let profile = test_profile();
        let v = variant(144);
        let requests = requests_for(&profile, &v);
        let targets_22_010c = requests
            .iter()
            .filter(|r| r.uds == [0x22, 0x01, 0x0C])
            .count();
        assert_eq!(
            targets_22_010c, 0,
            "the 161-192 cell command should be skipped"
        );
        // The 1-32 cell command is still present (within range for every variant).
        assert!(requests.iter().any(|r| r.uds == [0x22, 0x01, 0x02]));
    }

    #[test]
    fn decode_truncates_cell_voltages_beyond_populated_cells() {
        let profile = test_profile();
        let v = variant(180); // command covers cells 161-192; only 161-180 populated.
        let target = EcuTarget {
            tx_header: 0x7E4,
            rx_header: 0x7EC,
        };
        // bix 32 = byte 4 of the post-prefix data; 32 cells * 1 byte each.
        let mut uds = vec![0x62, 0x01, 0x0C];
        uds.extend(std::iter::repeat_n(0u8, 4));
        uds.extend(std::iter::repeat_n(181u8, 32));
        let outcome = decode(&profile, &v, target, &uds).expect("known command");
        let max_index = outcome.canonical.iter().filter_map(|r| r.index).max();
        assert_eq!(max_index, Some(180));
        assert_eq!(outcome.canonical.len(), 20, "cells 161..=180 inclusive");
    }

    #[test]
    fn decode_session_ack_yields_an_empty_ok_outcome() {
        let profile = test_profile();
        let v = variant(180);
        let target = EcuTarget {
            tx_header: 0x7E5,
            rx_header: 0x7ED,
        };
        let outcome = decode(&profile, &v, target, &[0x50, 0x03]).expect("session ack decodes");
        assert_eq!(outcome, DecodeOutcome::default());
    }

    #[test]
    fn decode_unknown_command_is_an_error() {
        let profile = test_profile();
        let v = variant(180);
        let target = EcuTarget {
            tx_header: 0x999,
            rx_header: 0x998,
        };
        assert!(matches!(
            decode(&profile, &v, target, &[0x62, 0xAB]),
            Err(DecodeError::UnknownCommand { .. })
        ));
    }
}
