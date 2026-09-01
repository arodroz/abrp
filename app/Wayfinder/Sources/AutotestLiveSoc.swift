// `--autotest live-soc-smoke` (wayfinder #79): drives the real FFI telemetry engine end to end --
// a `TelemetrySessionDialogue` wrapping a real `TelemetrySession` built from the bundled Ioniq 5
// profile, against a `StubTelemetryLink` scripted with the exact ELM/ISO-TP dialogue
// core/telemetry/src/dialogue.rs's `Engine` sends for that profile's `77_4_kwh` variant: the fixed
// init handshake, then per-target setup (`ATSH`/`ATCRA`/`ATFCSH`/`ATFCSD`/`ATFCSM1`) before each
// of its three ECU targets' commands -- all exactly as proven in
// core/telemetry/tests/replay_scenarios.rs and tests/common/mod.rs's shared fixtures. 11 commands
// total, one of them (`22 E0 11`) preceded by its `10 03` session prerequisite (decode.rs's
// `requests_for`). Only `22 01 01` (Display SoC's command) gets a real answer: a synthetic
// multi-frame ISO-TP response (First Frame + 8 Consecutive Frames, 62 bytes total like
// core/telemetry's own replay fixtures) encoding raw 143 at bix 32 -- byte 7 of the reassembled
// payload, 4 bytes after the echoed `62 01 01` prefix -- which the profile's `div: 2` decodes to
// 71.5%. Every other command gets `NO DATA`, so the engine records a failed command and moves on
// (proven already by replay_scenarios.rs's `no_data_then_success_fixture`) -- decoded readings
// still land for `22 01 01`.
//
// A SEPARATE FILE, same reasoning as AutotestObdSmoke.swift's header: appending this to
// Autotest.swift reproducibly breaks the Swift 6 strict-concurrency checker once that file grows
// past some size.
import CoreLocation
import Foundation
import PlannerKit

extension Autotest {
    @MainActor
    static func runLiveSocSmoke() async {
        var ok = true

        // 1-3: LiveSocPresentation is a pure function -- no store needed.
        let hiddenOk = LiveSocPresentation.compute(soc: nil, age: nil) == .hidden
        report("presentation-hidden", hiddenOk)
        ok = ok && hiddenOk

        let freshOk = LiveSocPresentation.compute(soc: 71.5, age: 3) == .fresh(71.5)
        report("presentation-fresh", freshOk)
        ok = ok && freshOk

        let staleOk = LiveSocPresentation.compute(soc: 71.5, age: 12) == .stale(71.5)
        report("presentation-stale", staleOk)
        ok = ok && staleOk

        // 4-8: the real engine end to end, over a scripted stub link, wired into a fresh
        // DriveStore exactly the way the app wires the real one (crib of drive-smoke's own
        // plan+go+ingest dance) -- local stores throughout, not the shared ones drive-smoke uses.
        let planStore = PlanStore()
        let tripStore = TripLogStore()
        tripStore.authorizationStatus = { .authorizedWhenInUse }
        tripStore.fetchTemperature = { _, _, _ in 15.0 }

        let stub = StubTelemetryLink()
        stub.script = liveSocScript()
        let telemetryStore = TelemetryLinkStore(link: stub)
        telemetryStore.makeDialogue = {
            guard let json = Ioniq5Profile.loadJson(),
                  let session = try? TelemetrySession(profileJson: json, variantId: Ioniq5Profile.variantId)
            else { return nil }
            return TelemetrySessionDialogue(session: session)
        }
        let driveStore = DriveStore(planStore: planStore, tripStore: tripStore, telemetryStore: telemetryStore)

        planStore.load()
        let plannerReadyOk = await waitWithTimeout(seconds: 120) { planStore.plannerStatus == .ready }
        report("planner-ready", plannerReadyOk)
        ok = ok && plannerReadyOk
        guard plannerReadyOk else { await finish(ok: false) }

        planStore.locationManager(
            CLLocationManager(), didUpdateLocations: [CLLocation(latitude: 49.6116, longitude: 6.1319)]
        )
        planStore.setDestination(name: "Amsterdam", coordinate: CLLocationCoordinate2D(latitude: 52.3702, longitude: 4.8952))
        let planLandedOk = await waitWithTimeout(seconds: 30) { planStore.plan != nil }
        report("plan-landed", planLandedOk)
        ok = ok && planLandedOk
        guard planLandedOk else { await finish(ok: false) }

        // 4: entering the drive is what flips the 12V-safety gate -- not this test directly.
        driveStore.go()
        tripStore.confirmStartSoc(80)
        driveStore.resolvePendingGo()
        let gateOpenedOk = driveStore.phase == .driving && telemetryStore.gateOpen
        report("gate-opened-by-enter-drive", gateOpenedOk, "phase=\(driveStore.phase) gateOpen=\(telemetryStore.gateOpen)")
        ok = ok && gateOpenedOk

        // 5: the automatic poll loop (TelemetryLinkStore.runPollLoop) lands the scripted sweep.
        let readingLandedOk = await waitWithTimeout(seconds: 10) {
            telemetryStore.liveDisplaySoc == 71.5 && telemetryStore.lastReadingAt != nil
        }
        report(
            "display-soc-landed", readingLandedOk,
            "liveDisplaySoc=\(String(describing: telemetryStore.liveDisplaySoc)) lastReadingAt=\(String(describing: telemetryStore.lastReadingAt))"
        )
        ok = ok && readingLandedOk

        // 6: End closes the gate and clears the readings.
        driveStore.end()
        let teardownOk = telemetryStore.gateOpen == false && telemetryStore.latestReadings.isEmpty
            && telemetryStore.lastReadingAt == nil
        report(
            "gate-closes-and-clears", teardownOk,
            "gateOpen=\(telemetryStore.gateOpen) readings=\(telemetryStore.latestReadings.count) "
                + "lastReadingAt=\(String(describing: telemetryStore.lastReadingAt))"
        )
        ok = ok && teardownOk

        await finish(ok: ok)
    }

    // MARK: Scripted engine dialogue

    /// The full 34-exchange script for one sweep of the bundled Ioniq 5 profile's `77_4_kwh`
    /// variant: 7 init commands, then per-target setup + requests for each of the profile's three
    /// ECU targets, in the profile's own command order.
    private static func liveSocScript() -> [ScriptedExchange] {
        var script = initExchanges
        script += setupExchanges(tx: "7E4", rx: "7EC")
        script.append(displaySocExchange())
        for hex in ["220102", "220103", "220104", "22010A", "22010B", "22010C", "220105", "220111"] {
            script.append(noDataExchange(hex))
        }
        script += setupExchanges(tx: "7C6", rx: "7CE")
        script.append(noDataExchange("22B002"))
        script += setupExchanges(tx: "7E5", rx: "7ED")
        script.append(noDataExchange("1003"))
        script.append(noDataExchange("22E011"))
        return script
    }

    /// Verbatim `dialogue.rs::INIT_COMMANDS` sequence (also core/telemetry/tests/common/mod.rs's
    /// `INIT` fixture): headers on, auto-formatting off, protocol 6 (ISO 15765-4 CAN 11/500).
    private static let initExchanges: [ScriptedExchange] = [
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
    private static func setupExchanges(tx: String, rx: String) -> [ScriptedExchange] {
        [
            ScriptedExchange(outgoing: Data("ATSH\(tx)\r".utf8), incoming: Data("OK\r\r>".utf8)),
            ScriptedExchange(outgoing: Data("ATCRA\(rx)\r".utf8), incoming: Data("OK\r\r>".utf8)),
            ScriptedExchange(outgoing: Data("ATFCSH\(tx)\r".utf8), incoming: Data("OK\r\r>".utf8)),
            ScriptedExchange(outgoing: Data("ATFCSD300000\r".utf8), incoming: Data("OK\r\r>".utf8)),
            ScriptedExchange(outgoing: Data("ATFCSM1\r".utf8), incoming: Data("OK\r\r>".utf8)),
        ]
    }

    private static func noDataExchange(_ requestHex: String) -> ScriptedExchange {
        ScriptedExchange(outgoing: singleFrameCommand(requestHex), incoming: Data("NO DATA\r\r>".utf8))
    }

    private static func displaySocExchange() -> ScriptedExchange {
        var payload = [UInt8](repeating: 0, count: 62)
        payload[0] = 0x62
        payload[1] = 0x01
        payload[2] = 0x01
        payload[7] = 143 // raw -> /2 (profile's div) -> 71.5%
        var incoming = Data()
        for frame in isoTpFrames(payload: payload) {
            incoming.append(Data("7EC".utf8))
            incoming.append(Data(frame.map { String(format: "%02X", $0) }.joined().utf8))
            incoming.append(Data("\r".utf8))
        }
        incoming.append(Data(">".utf8))
        return ScriptedExchange(outgoing: singleFrameCommand("220101"), incoming: incoming)
    }

    /// ISO-TP First Frame + Consecutive Frames for `payload` (this ticket's case: 62 bytes, so 1
    /// FF + 8 CFs), mirroring core/telemetry/src/isotp.rs's `Reassembler` on the wire. The ELM
    /// adapter itself emits Flow Control (`ATFCSH`/`ATFCSD`/`ATFCSM1` configure it to) -- the
    /// engine never sends one, so this stub never needs to simulate one either.
    private static func isoTpFrames(payload: [UInt8]) -> [[UInt8]] {
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
    private static func singleFrameCommand(_ requestHex: String) -> Data {
        var frame = [UInt8](repeating: 0, count: 8)
        let uds = hexBytes(requestHex)
        frame[0] = UInt8(uds.count)
        for (i, b) in uds.enumerated() { frame[1 + i] = b }
        let hex = frame.map { String(format: "%02X", $0) }.joined()
        return Data((hex + "\r").utf8)
    }

    private static func hexBytes(_ hex: String) -> [UInt8] {
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
