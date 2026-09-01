// `--autotest triplog-telemetry-smoke` (wayfinder #80): drives the real FFI telemetry engine
// (crib of AutotestLiveSoc.swift's drive dance, builders shared via
// AutotestTelemetryScripting.swift) through TWO full poll sweeps whose `220101` responses carry
// different Display SoC / cumulative-counter values -- sweep 1 the trip's start state, the final
// sweep its end state -- and proves: TripLogStore's lazy/eager telemetry snapshot lands the
// sweep-1 values at `confirmStartSoc` and the final-sweep values at `stopTapped` (via the poll
// loop, never test-injected), the saved tlog-1's `telemetry` block carries exactly those raw
// snapshots (BMS SoC fields null -- unmapped until #81), Rust's `calibrate()` consumes the
// measured net-of-regen energy from the counters (not a display-SoC inference) with
// `measured == true`, and a drive with no telemetry at all still saves a tlog with no
// `telemetry` block (the no-dongle fallback stays intact).
//
// A SEPARATE FILE, same reasoning as AutotestObdSmoke.swift's header: appending this to
// Autotest.swift reproducibly breaks the Swift 6 strict-concurrency checker once that file grows
// past some size.
import CoreLocation
import Foundation
import PlannerKit

extension Autotest {
    @MainActor
    static func runTriplogTelemetrySmoke() async {
        var ok = true

        let planStore = PlanStore()
        let tripStore = TripLogStore()
        tripStore.authorizationStatus = { .authorizedWhenInUse }
        tripStore.fetchTemperature = { _, _, _ in 15.0 }

        let stub = StubTelemetryLink()
        stub.script = twoSweepScript()
        let telemetryStore = TelemetryLinkStore(link: stub)
        telemetryStore.makeDialogue = {
            guard let json = Ioniq5Profile.loadJson(),
                  let session = try? TelemetrySession(profileJson: json, variantId: Ioniq5Profile.variantId)
            else { return nil }
            return TelemetrySessionDialogue(session: session)
        }
        // wayfinder #80: wiring only -- the values below come from the poll loop draining the
        // scripted sweeps into `latestReadings`, never from this test directly.
        tripStore.telemetrySnapshot = { [telemetryStore] in
            guard let lastReadingAt = telemetryStore.lastReadingAt,
                  Date().timeIntervalSince(lastReadingAt) <= TelemetryLinkStore.snapshotFreshnessS
            else { return nil }
            let readings = telemetryStore.latestReadings
            return TripTelemetrySnapshot(
                displaySocPct: readings[.displaySoc], bmsSocPct: readings[.bmsSoc],
                cumulativeChargeKwh: readings[.cumulativeChargeEnergy],
                cumulativeDischargeKwh: readings[.cumulativeDischargeEnergy]
            )
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

        // Go opens the 12V gate at intent-to-drive (wayfinder #80); wait for sweep 1 to land
        // BEFORE confirming start SoC, so `confirmStartSoc`'s "already fresh" snapshot path --
        // not the lazy `ingest` fill -- is what's under test here.
        driveStore.go()
        let firstSweepOk = await waitWithTimeout(seconds: 10) {
            telemetryStore.liveDisplaySoc == 71.5 && telemetryStore.lastReadingAt != nil
        }
        report(
            "first-sweep-landed", firstSweepOk,
            "liveDisplaySoc=\(String(describing: telemetryStore.liveDisplaySoc))"
        )
        ok = ok && firstSweepOk
        guard firstSweepOk else { await finish(ok: false) }

        tripStore.confirmStartSoc(71)
        driveStore.resolvePendingGo()
        let enteredOk = driveStore.phase == .driving && tripStore.phase == .recording
        report("entered-drive", enteredOk, "drivePhase=\(driveStore.phase) tripPhase=\(tripStore.phase)")
        ok = ok && enteredOk
        guard enteredOk, let tripStartDate = tripStore.tripStartDate else { await finish(ok: false) }

        // A short synthetic trace -- this smoke is about the telemetry block/measured energy,
        // not the trace itself (calibrate-smoke/drive-smoke already cover trace shape).
        for i in 0..<200 {
            tripStore.ingest(CLLocation(
                coordinate: CLLocationCoordinate2D(latitude: 49.0 + 0.00025 * Double(i), longitude: 6.0),
                altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5, course: -1, speed: 27.8,
                timestamp: tripStartDate.addingTimeInterval(Double(i))
            ))
        }

        // Wait for the LAST scripted sweep (the end state) to land, then end the drive
        // immediately -- the script is exhausted after this, and CAN-quiet backoff may fire on a
        // further poll; ending stops the poll loop before that can affect anything below.
        let finalSweepOk = await waitWithTimeout(seconds: 15) { telemetryStore.liveDisplaySoc == 43.0 }
        report(
            "final-sweep-landed", finalSweepOk,
            "liveDisplaySoc=\(String(describing: telemetryStore.liveDisplaySoc))"
        )
        ok = ok && finalSweepOk

        // `end()` -> `stopTapped()` (end snapshot, gate still open) -> gate closes, in that
        // order (wayfinder #79/#80's DriveStore ordering guarantee).
        driveStore.end()
        let savedBefore = tripStore.lastSavedURL
        tripStore.confirmEndSoc(43)
        let savedOk = await waitWithTimeout(seconds: 15) { tripStore.lastSavedURL != savedBefore }
        report("saved", savedOk)
        ok = ok && savedOk
        guard savedOk, let url = tripStore.lastSavedURL else { await finish(ok: false) }

        var savedLog: TripLog?
        do {
            let data = try Data(contentsOf: url)
            savedLog = try JSONDecoder().decode(TripLog.self, from: data)
        } catch {
            report("telemetry-block-saved", false, "\(error)")
            ok = false
        }

        if let telemetry = savedLog?.telemetry {
            let blockOk = telemetry.startDisplaySocPct == 71.5 && telemetry.endDisplaySocPct == 43.0
                && telemetry.startBmsSocPct == nil && telemetry.endBmsSocPct == nil
                && telemetry.startCumulativeChargeKwh == 100.0 && telemetry.endCumulativeChargeKwh == 100.2
                && telemetry.startCumulativeDischargeKwh == 200.0 && telemetry.endCumulativeDischargeKwh == 224.8
            report("telemetry-block-saved", blockOk, "telemetry=\(telemetry)")
            ok = ok && blockOk
        } else {
            report("telemetry-block-saved", false, "no telemetry block in the saved log")
            ok = false
        }

        // Rust-side: calibrate() over the saved log consumes the measured net-of-regen energy
        // from the counters, not a display-SoC inference.
        planStore.refreshCalibration(logURLs: [url])
        let calibratedOk = await waitWithTimeout(seconds: 30) { planStore.calibrationResult != nil }
        if calibratedOk, let fit = planStore.calibrationResult?.trips.first {
            // (224.8 - 200.0) - (100.2 - 100.0) = 24.6 kWh = 24_600 Wh, net of the charge
            // counter (regen), exactly what the two scripted sweeps above imply.
            let measuredOk = fit.measured && abs(fit.actualWh - 24_600.0) < 1e-6
            report("measured-energy", measuredOk, "measured=\(fit.measured) actualWh=\(fit.actualWh)")
            ok = ok && measuredOk
        } else {
            report(
                "measured-energy", false,
                "calibrationErrorMessage=\(planStore.calibrationErrorMessage ?? "none")"
            )
            ok = false
        }

        try? TripLogStorage.delete(url: url)

        // Fallback: a trip with NO telemetry (a standalone store whose `telemetrySnapshot`
        // stays the default `{ nil }`) saves a tlog with no `telemetry` block at all.
        let noTelemetryStore = TripLogStore()
        noTelemetryStore.authorizationStatus = { .authorizedWhenInUse }
        noTelemetryStore.fetchTemperature = { _, _, _ in 15.0 }
        noTelemetryStore.startTapped()
        noTelemetryStore.confirmStartSoc(70)
        noTelemetryStore.ingest(CLLocation(
            coordinate: CLLocationCoordinate2D(latitude: 49.6116, longitude: 6.1319),
            altitude: 300, horizontalAccuracy: 5, verticalAccuracy: 5, course: -1, speed: 15, timestamp: Date()
        ))
        noTelemetryStore.stopTapped()
        noTelemetryStore.confirmEndSoc(60)
        let noTelemetrySavedOk = await waitWithTimeout(seconds: 15) { noTelemetryStore.lastSavedURL != nil }
        if noTelemetrySavedOk, let noTelemetryUrl = noTelemetryStore.lastSavedURL {
            do {
                let data = try Data(contentsOf: noTelemetryUrl)
                let log = try JSONDecoder().decode(TripLog.self, from: data)
                let fallbackOk = log.telemetry == nil
                report("no-telemetry-fallback", fallbackOk, "telemetry=\(String(describing: log.telemetry))")
                ok = ok && fallbackOk
            } catch {
                report("no-telemetry-fallback", false, "\(error)")
                ok = false
            }
            try? TripLogStorage.delete(url: noTelemetryUrl)
        } else {
            report("no-telemetry-fallback", false, "no-telemetry log never saved")
            ok = false
        }

        await finish(ok: ok)
    }

    // MARK: Scripted two-sweep dialogue

    /// Two full sweeps of the bundled Ioniq 5 profile's `77_4_kwh` variant, back to back --
    /// sweep 1 the trip's start telemetry, sweep 2 (the last one scripted) its end telemetry.
    /// Every other command answers `NO DATA` in both sweeps, same as AutotestLiveSoc's script.
    private static func twoSweepScript() -> [ScriptedExchange] {
        let sweep1 = sweepScript(command220101Exchange(
            displaySocRaw: 143, chargeAhRaw: 2_600, dischargeAhRaw: 5_200,
            chargeEnergyRaw: 1_000, dischargeEnergyRaw: 2_000
        ))
        let sweep2 = sweepScript(command220101Exchange(
            displaySocRaw: 86, chargeAhRaw: 2_605, dischargeAhRaw: 5_248,
            chargeEnergyRaw: 1_002, dischargeEnergyRaw: 2_248
        ))
        return sweep1 + sweep2
    }

    /// One full sweep's exchange list, `command220101` supplying the one command this smoke
    /// cares about -- structurally identical to AutotestLiveSoc's `liveSocScript`.
    private static func sweepScript(_ command220101: ScriptedExchange) -> [ScriptedExchange] {
        var script = telemetryInitExchanges
        script += telemetrySetupExchanges(tx: "7E4", rx: "7EC")
        script.append(command220101)
        for hex in ["220102", "220103", "220104", "22010A", "22010B", "22010C", "220105", "220111"] {
            script.append(telemetryNoDataExchange(hex))
        }
        script += telemetrySetupExchanges(tx: "7C6", rx: "7CE")
        script.append(telemetryNoDataExchange("22B002"))
        script += telemetrySetupExchanges(tx: "7E5", rx: "7ED")
        script.append(telemetryNoDataExchange("1003"))
        script.append(telemetryNoDataExchange("22E011"))
        return script
    }

    /// Builds `220101`'s scripted response: Display SoC (bix 32, byte 7) plus the profile's four
    /// cumulative Ah/kWh counters (bix 240/272/304/336 -> payload bytes 33/37/41/45 -- offset
    /// `3 + bix/8`, after the echoed `620101`), each len-32/div-10/big-endian.
    private static func command220101Exchange(
        displaySocRaw: UInt8, chargeAhRaw: UInt32, dischargeAhRaw: UInt32,
        chargeEnergyRaw: UInt32, dischargeEnergyRaw: UInt32
    ) -> ScriptedExchange {
        var payload = [UInt8](repeating: 0, count: 62)
        payload[0] = 0x62
        payload[1] = 0x01
        payload[2] = 0x01
        payload[7] = displaySocRaw // raw -> /2 (profile's div)
        setBigEndianU32(chargeAhRaw, in: &payload, at: 33)
        setBigEndianU32(dischargeAhRaw, in: &payload, at: 37)
        setBigEndianU32(chargeEnergyRaw, in: &payload, at: 41)
        setBigEndianU32(dischargeEnergyRaw, in: &payload, at: 45)

        var incoming = Data()
        for frame in isoTpFrames(payload: payload) {
            incoming.append(Data("7EC".utf8))
            incoming.append(Data(frame.map { String(format: "%02X", $0) }.joined().utf8))
            incoming.append(Data("\r".utf8))
        }
        incoming.append(Data(">".utf8))
        return ScriptedExchange(outgoing: singleFrameCommand("220101"), incoming: incoming)
    }

    private static func setBigEndianU32(_ value: UInt32, in payload: inout [UInt8], at offset: Int) {
        payload[offset] = UInt8((value >> 24) & 0xFF)
        payload[offset + 1] = UInt8((value >> 16) & 0xFF)
        payload[offset + 2] = UInt8((value >> 8) & 0xFF)
        payload[offset + 3] = UInt8(value & 0xFF)
    }
}
