//! Shared replay fixture snippets (wayfinder #76). The init handshake and
//! BMS-target setup sequence are identical across most scenarios, so each
//! test's own fixture literal only needs to cover what it's actually
//! exercising.

use telemetry::EcuTarget;

pub const BMS: EcuTarget = EcuTarget {
    tx_header: 0x7E4,
    rx_header: 0x7EC,
};

/// `tx`/`rx` lines for the fixed init sequence. Ends with a trailing
/// newline so callers can splice it directly in front of their own lines.
pub const INIT: &str = r"tx ATZ
rx ATZ\r\rELM327 v1.5\r\r>
tx ATE0
rx OK\r\r>
tx ATL0
rx OK\r\r>
tx ATS0
rx OK\r\r>
tx ATH1
rx OK\r\r>
tx ATCAF0
rx OK\r\r>
tx ATSP6
rx OK\r\r>
";

/// `tx`/`rx` lines for BMS target setup (`0x7E4`/`0x7EC`).
pub const BMS_SETUP: &str = r"tx ATSH7E4
rx OK\r\r>
tx ATCRA7EC
rx OK\r\r>
tx ATFCSH7E4
rx OK\r\r>
tx ATFCSD300000
rx OK\r\r>
tx ATFCSM1
rx OK\r\r>
";
