//! Replay-driven scenarios for the dialogue engine (wayfinder #76), against
//! the BMS target (`0x7E4`/`0x7EC`) and its two workhorse DIDs from
//! `docs/research/ioniq5-obd-telemetry.md` §2 (`22 01 01`, `22 01 05`).
//! Frame payloads are synthetic but exactly reassembled and asserted --
//! byte 0 is always the real `0x62`/`0x7F` response marker, the rest is
//! arbitrary distinct filler so a reassembly bug (dropped/reordered bytes)
//! can't hide behind repeated values.

mod common;

use common::{BMS, BMS_SETUP, INIT};
use telemetry::isotp::IsoTpError;
use telemetry::replay::{run_replay, run_replay_chunked};
use telemetry::{Engine, Event, FailReason, Request};

fn expected_payload_a() -> Vec<u8> {
    let mut v = vec![0x62, 0x01, 0x01];
    v.extend(3u8..62);
    v
}

fn expected_payload_b() -> Vec<u8> {
    let mut v = vec![0x62, 0x01, 0x05];
    v.extend((3u8..48).map(|i| i + 0x80));
    v
}

fn happy_path_requests() -> Vec<Request> {
    vec![
        Request {
            target: BMS,
            uds: vec![0x22, 0x01, 0x01],
        },
        Request {
            target: BMS,
            uds: vec![0x22, 0x01, 0x05],
        },
    ]
}

/// `22 01 01` -> First Frame `10 3E` + CFs `21`..`28` (6 + 7*8 = 62 bytes),
/// then `22 01 05` -> FF `10 30` + CFs `21`..`26` (6 + 7*6 = 48 bytes), to
/// the same target, so setup happens only once.
fn happy_path_fixture() -> String {
    format!(
        r"{INIT}{BMS_SETUP}tx 0322010100000000
rx 7EC103E620101030405\r
rx 7EC21060708090A0B0C\r
rx 7EC220D0E0F10111213\r
rx 7EC231415161718191A\r
rx 7EC241B1C1D1E1F2021\r
rx 7EC2522232425262728\r
rx 7EC26292A2B2C2D2E2F\r
rx 7EC2730313233343536\r
rx 7EC283738393A3B3C3D\r
rx >
tx 0322010500000000
rx 7EC1030620105838485\r
rx 7EC21868788898A8B8C\r
rx 7EC228D8E8F90919293\r
rx 7EC239495969798999A\r
rx 7EC249B9C9D9E9FA0A1\r
rx 7EC25A2A3A4A5A6A7A8\r
rx 7EC26A9AAABACADAEAF\r
rx >
"
    )
}

#[test]
fn happy_path_two_requests_same_target_setup_once() {
    let mut engine = Engine::new(happy_path_requests());
    let events = run_replay(&mut engine, &happy_path_fixture());
    assert_eq!(events.len(), 3);
    assert_eq!(events[0], Event::AdapterReady);
    assert_eq!(
        events[1],
        Event::Payload {
            target: BMS,
            uds: expected_payload_a(),
        }
    );
    assert_eq!(
        events[2],
        Event::Payload {
            target: BMS,
            uds: expected_payload_b(),
        }
    );
    assert!(engine.is_finished());
}

#[test]
fn happy_path_chunked_matches_unchunked() {
    let expected = {
        let mut engine = Engine::new(happy_path_requests());
        run_replay(&mut engine, &happy_path_fixture())
    };
    for chunk in [1usize, 3, 20] {
        let mut engine = Engine::new(happy_path_requests());
        let events = run_replay_chunked(&mut engine, &happy_path_fixture(), chunk);
        assert_eq!(events, expected, "chunk size {chunk}");
        assert!(engine.is_finished());
    }
}

/// `NO DATA` on the first request, success (a plain single-frame response
/// this time) on the second, to the same target -- no re-setup lines
/// appear in the fixture, so a spurious re-setup attempt would desync the
/// `tx` assertions and fail loudly.
fn no_data_then_success_fixture() -> String {
    format!(
        r"{INIT}{BMS_SETUP}tx 0322010100000000
rx NO DATA\r
rx >
tx 0322010500000000
rx 7EC0362010500000000\r
rx >
"
    )
}

#[test]
fn no_data_then_success_recovers_without_resetup() {
    let mut engine = Engine::new(happy_path_requests());
    let events = run_replay(&mut engine, &no_data_then_success_fixture());
    assert_eq!(events.len(), 3);
    assert_eq!(events[0], Event::AdapterReady);
    assert_eq!(
        events[1],
        Event::Failed {
            target: BMS,
            uds: vec![0x22, 0x01, 0x01],
            reason: FailReason::NoData,
        }
    );
    assert_eq!(
        events[2],
        Event::Payload {
            target: BMS,
            uds: vec![0x62, 0x01, 0x05],
        }
    );
    assert!(engine.is_finished());
}

/// Single Frame `03 7F 22 31`: a negative response to SID `0x22` with
/// NRC `0x31`.
fn negative_response_fixture() -> String {
    format!(
        r"{INIT}{BMS_SETUP}tx 0322010100000000
rx 7EC037F223100000000\r
rx >
"
    )
}

#[test]
fn negative_response_reports_nrc() {
    let mut engine = Engine::new(vec![Request {
        target: BMS,
        uds: vec![0x22, 0x01, 0x01],
    }]);
    let events = run_replay(&mut engine, &negative_response_fixture());
    assert_eq!(events.len(), 2);
    assert_eq!(events[0], Event::AdapterReady);
    assert_eq!(
        events[1],
        Event::Failed {
            target: BMS,
            uds: vec![0x22, 0x01, 0x01],
            reason: FailReason::Negative { nrc: 0x31 },
        }
    );
    assert!(engine.is_finished());
}

/// First Frame (len 20) + CF `21` (accepted, next expected is `22`) + CF
/// `23` (gap) -- the terminal prompt still arrives afterward.
fn sequence_gap_fixture() -> String {
    format!(
        r"{INIT}{BMS_SETUP}tx 0322010100000000
rx 7EC1014000102030405\r
rx 7EC21060708090A0B0C\r
rx 7EC230D0E0F10111213\r
rx >
"
    )
}

#[test]
fn sequence_gap_fails_after_terminal_prompt() {
    let mut engine = Engine::new(vec![Request {
        target: BMS,
        uds: vec![0x22, 0x01, 0x01],
    }]);
    let events = run_replay(&mut engine, &sequence_gap_fixture());
    assert_eq!(events.len(), 2);
    assert_eq!(events[0], Event::AdapterReady);
    assert_eq!(
        events[1],
        Event::Failed {
            target: BMS,
            uds: vec![0x22, 0x01, 0x01],
            reason: FailReason::IsoTp(IsoTpError::SequenceGap {
                expected: 2,
                got: 3,
            }),
        }
    );
    assert!(engine.is_finished());
}

/// `STOPPED` arrives mid-collection (after a First Frame); the next
/// request to the same target still runs.
fn stopped_mid_collection_fixture() -> String {
    format!(
        r"{INIT}{BMS_SETUP}tx 0322010100000000
rx 7EC1014000102030405\r
rx STOPPED\r
rx >
tx 0322010500000000
rx 7EC0362010500000000\r
rx >
"
    )
}

#[test]
fn stopped_mid_collection_then_next_request_proceeds() {
    let mut engine = Engine::new(happy_path_requests());
    let events = run_replay(&mut engine, &stopped_mid_collection_fixture());
    assert_eq!(events.len(), 3);
    assert_eq!(events[0], Event::AdapterReady);
    assert_eq!(
        events[1],
        Event::Failed {
            target: BMS,
            uds: vec![0x22, 0x01, 0x01],
            reason: FailReason::Stopped,
        }
    );
    assert_eq!(
        events[2],
        Event::Payload {
            target: BMS,
            uds: vec![0x62, 0x01, 0x05],
        }
    );
    assert!(engine.is_finished());
}

/// The transport's read times out with no frames at all; the request
/// fails as `Timeout`, and the adapter's late `>` still concludes it
/// cleanly before the next request runs.
fn timeout_fixture() -> String {
    format!(
        r"{INIT}{BMS_SETUP}tx 0322010100000000
timeout
rx >
tx 0322010500000000
rx 7EC0362010500000000\r
rx >
"
    )
}

#[test]
fn timeout_then_late_prompt_lets_next_request_run() {
    let mut engine = Engine::new(happy_path_requests());
    let events = run_replay(&mut engine, &timeout_fixture());
    assert_eq!(events.len(), 3);
    assert_eq!(events[0], Event::AdapterReady);
    assert_eq!(
        events[1],
        Event::Failed {
            target: BMS,
            uds: vec![0x22, 0x01, 0x01],
            reason: FailReason::Timeout,
        }
    );
    assert_eq!(
        events[2],
        Event::Payload {
            target: BMS,
            uds: vec![0x62, 0x01, 0x05],
        }
    );
    assert!(engine.is_finished());
}

/// A `7E8`-id line (well-formed frame shape, foreign id) interleaved
/// inside the `7EC` stream must not perturb reassembly.
fn foreign_frame_fixture() -> String {
    format!(
        r"{INIT}{BMS_SETUP}tx 0322010100000000
rx 7EC103E620101030405\r
rx 7E8DEADBEEFDEADBEEF\r
rx 7EC21060708090A0B0C\r
rx 7EC220D0E0F10111213\r
rx 7EC231415161718191A\r
rx 7EC241B1C1D1E1F2021\r
rx 7EC2522232425262728\r
rx 7EC26292A2B2C2D2E2F\r
rx 7EC2730313233343536\r
rx 7EC283738393A3B3C3D\r
rx >
"
    )
}

#[test]
fn foreign_frame_id_is_ignored() {
    let mut engine = Engine::new(vec![Request {
        target: BMS,
        uds: vec![0x22, 0x01, 0x01],
    }]);
    let events = run_replay(&mut engine, &foreign_frame_fixture());
    assert_eq!(events.len(), 2);
    assert_eq!(events[0], Event::AdapterReady);
    assert_eq!(
        events[1],
        Event::Payload {
            target: BMS,
            uds: expected_payload_a(),
        }
    );
    assert!(engine.is_finished());
}
