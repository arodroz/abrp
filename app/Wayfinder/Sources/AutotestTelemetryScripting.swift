// Shared ELM/ISO-TP dialogue-scripting helpers for `StubTelemetryLink` (wayfinder #79's
// AutotestLiveSoc, reused as-is by wayfinder #80's AutotestTriplogTelemetry): the fixed init
// handshake, per-target setup commands, NO-DATA responses, and the ISO-TP frame/single-frame-
// command byte builders every scripted engine dialogue needs. Extracted here (widened from
// AutotestLiveSoc.swift's originally-private copies) so a second autotest scripting a full
// profile sweep doesn't duplicate ISO-TP frame-building by hand -- see that file's header for
// the wire-level provenance (core/telemetry/src/dialogue.rs's `Engine`).
import Foundation

extension Autotest {
    /// Verbatim `dialogue.rs::INIT_COMMANDS` sequence (also core/telemetry/tests/common/mod.rs's
    /// `INIT` fixture): headers on, auto-formatting off, protocol 6 (ISO 15765-4 CAN 11/500).
    static let telemetryInitExchanges: [ScriptedExchange] = [
        ScriptedExchange(outgoing: Data("ATZ\r".utf8), incoming: Data("ATZ\r\rELM327 v1.5\r\r>".utf8)),
        ScriptedExchange(outgoing: Data("ATE0\r".utf8), incoming: Data("OK\r\r>".utf8)),
        ScriptedExchange(outgoing: Data("ATL0\r".utf8), incoming: Data("OK\r\r>".utf8)),
        ScriptedExchange(outgoing: Data("ATS0\r".utf8), incoming: Data("OK\r\r>".utf8)),
        ScriptedExchange(outgoing: Data("ATH1\r".utf8), incoming: Data("OK\r\r>".utf8)),
        ScriptedExchange(outgoing: Data("ATCAF0\r".utf8), incoming: Data("OK\r\r>".utf8)),
        ScriptedExchange(outgoing: Data("ATSP6\r".utf8), incoming: Data("OK\r\r>".utf8)),
    ]

    /// Verbatim `dialogue.rs::setup_commands` for one ECU target (also mirrored by
    /// core/telemetry/tests/common/mod.rs's `BMS_SETUP` fixture, for 7E4/7EC).
    static func telemetrySetupExchanges(tx: String, rx: String) -> [ScriptedExchange] {
        [
            ScriptedExchange(outgoing: Data("ATSH\(tx)\r".utf8), incoming: Data("OK\r\r>".utf8)),
            ScriptedExchange(outgoing: Data("ATCRA\(rx)\r".utf8), incoming: Data("OK\r\r>".utf8)),
            ScriptedExchange(outgoing: Data("ATFCSH\(tx)\r".utf8), incoming: Data("OK\r\r>".utf8)),
            ScriptedExchange(outgoing: Data("ATFCSD300000\r".utf8), incoming: Data("OK\r\r>".utf8)),
            ScriptedExchange(outgoing: Data("ATFCSM1\r".utf8), incoming: Data("OK\r\r>".utf8)),
        ]
    }

    static func telemetryNoDataExchange(_ requestHex: String) -> ScriptedExchange {
        ScriptedExchange(outgoing: singleFrameCommand(requestHex), incoming: Data("NO DATA\r\r>".utf8))
    }

    /// ISO-TP First Frame + Consecutive Frames for `payload`, mirroring
    /// core/telemetry/src/isotp.rs's `Reassembler` on the wire. The ELM adapter itself emits
    /// Flow Control (`ATFCSH`/`ATFCSD`/`ATFCSM1` configure it to) -- the engine never sends one,
    /// so a stub never needs to simulate one either.
    static func isoTpFrames(payload: [UInt8]) -> [[UInt8]] {
        let len = payload.count
        var first = [UInt8](repeating: 0, count: 8)
        first[0] = 0x10 | UInt8((len >> 8) & 0x0F)
        first[1] = UInt8(len & 0xFF)
        for (i, b) in payload.prefix(6).enumerated() { first[2 + i] = b }
        var frames = [first]

        var offset = 6
        var seq = 1
        while offset < len {
            var cf = [UInt8](repeating: 0, count: 8)
            cf[0] = 0x20 | UInt8(seq & 0x0F)
            for (i, b) in payload[offset..<min(offset + 7, len)].enumerated() { cf[1 + i] = b }
            frames.append(cf)
            offset += 7
            seq += 1
        }
        return frames
    }

    /// ASCII text of one ISO-TP Single Frame ELM command for `requestHex` (e.g. `"220101"`),
    /// mirroring core/telemetry/src/dialogue.rs's `take_outgoing` (`encode_single_frame` +
    /// `to_hex_upper`) plus its trailing CR.
    static func singleFrameCommand(_ requestHex: String) -> Data {
        var frame = [UInt8](repeating: 0, count: 8)
        let uds = hexBytes(requestHex)
        frame[0] = UInt8(uds.count)
        for (i, b) in uds.enumerated() { frame[1 + i] = b }
        let hex = frame.map { String(format: "%02X", $0) }.joined()
        return Data((hex + "\r").utf8)
    }

    static func hexBytes(_ hex: String) -> [UInt8] {
        var bytes: [UInt8] = []
        var idx = hex.startIndex
        while idx < hex.endIndex {
            let next = hex.index(idx, offsetBy: 2)
            bytes.append(UInt8(hex[idx..<next], radix: 16)!)
            idx = next
        }
        return bytes
    }
}
