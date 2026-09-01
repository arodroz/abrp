//! Structural checks on the shipped first-party profiles (wayfinder #77):
//! both load and validate, the Ioniq 5 profile claims `vector-validated`
//! (backed by `tests/ioniq5_vectors.rs`), the EV6 paper profile is
//! byte-for-byte identical on every command/signal (the modularity proof --
//! it was authored with zero changes to this crate's Rust code), and each
//! profile's own declared pack variants resolve.

use telemetry::{TelemetryProfile, ValidationTier};

const IONIQ5_JSON: &str = include_str!("../../../data/profiles/hyundai-ioniq5.tprof.json");
const EV6_JSON: &str = include_str!("../../../data/profiles/kia-ev6.tprof.json");

#[test]
fn ioniq5_profile_loads_and_claims_vector_validated() {
    let profile = TelemetryProfile::load(IONIQ5_JSON).expect("hyundai-ioniq5.tprof.json is valid");
    assert_eq!(profile.id, "hyundai-ioniq5");
    assert_eq!(profile.tier, ValidationTier::VectorValidated);
    for id in ["58_kwh", "72_6_kwh", "77_4_kwh", "84_kwh"] {
        assert!(profile.variant(id).is_ok(), "missing pack variant {id}");
    }
    assert_eq!(profile.variant("72_6_kwh").unwrap().populated_cells, 180);
}

#[test]
fn ev6_profile_loads_and_is_paper_tier() {
    let profile = TelemetryProfile::load(EV6_JSON).expect("kia-ev6.tprof.json is valid");
    assert_eq!(profile.id, "kia-ev6");
    assert_eq!(profile.tier, ValidationTier::Paper);
    for id in ["58_kwh", "77_4_kwh"] {
        assert!(profile.variant(id).is_ok(), "missing pack variant {id}");
    }
    // The EV6 doesn't offer the Ioniq 5's 72.6/84 kWh trims.
    assert!(profile.variant("72_6_kwh").is_err());
    assert!(profile.variant("84_kwh").is_err());
}

/// The modularity proof (wayfinder #77 ticket): the EV6's commands/signals
/// are the Ioniq 5's shared E-GMP layout, unchanged -- authored as a new
/// data file, zero Rust code touched.
#[test]
fn ev6_shares_the_ioniq5_command_and_signal_layout_verbatim() {
    let ioniq5 = TelemetryProfile::load(IONIQ5_JSON).unwrap();
    let ev6 = TelemetryProfile::load(EV6_JSON).unwrap();

    assert_eq!(ioniq5.commands.len(), ev6.commands.len());
    for (a, b) in ioniq5.commands.iter().zip(ev6.commands.iter()) {
        assert_eq!(a.tx_header, b.tx_header);
        assert_eq!(a.rx_header, b.rx_header);
        assert_eq!(a.request, b.request);
        assert_eq!(a.session_prerequisite, b.session_prerequisite);
        assert_eq!(a.signals.len(), b.signals.len());
        for (sa, sb) in a.signals.iter().zip(b.signals.iter()) {
            assert_eq!(sa.id, sb.id);
            assert_eq!(sa.canonical, sb.canonical);
            assert_eq!(sa.bix, sb.bix);
            assert_eq!(sa.len, sb.len);
            assert_eq!(sa.sign, sb.sign);
            assert_eq!(sa.add, sb.add);
            assert_eq!(sa.mul, sb.mul);
            assert_eq!(sa.div, sb.div);
            assert_eq!(sa.count, sb.count);
            assert_eq!(sa.first_index, sb.first_index);
        }
    }
}
