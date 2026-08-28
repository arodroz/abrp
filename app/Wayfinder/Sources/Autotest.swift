// Launch-argument autotest (wayfinder #39): `--autotest plan-golden` runs the
// same LU -> Amsterdam golden request as
// PlannerKitTests/GoldenPlanTests.swift, off the main thread, against the
// pack sideloaded into Documents, and exits with the pass/fail result. This
// proves a plan() round-trip through the real xcframework on the simulator,
// independent of the (placeholder) UI. `--autotest editor-smoke` (wayfinder
// #40) extends this to drive the route editor's store mutations directly --
// there's no UI-event injection on the sim, so the editor UI itself is
// verified separately, by screenshot.
import CoreLocation
import Foundation
import MapLibre
import PlannerKit

enum Autotest {
    static func runIfRequested(store: PlanStore) {
        let args = ProcessInfo.processInfo.arguments
        guard let flagIndex = args.firstIndex(of: "--autotest"),
              flagIndex + 1 < args.count
        else { return }

        switch args[flagIndex + 1] {
        case "plan-golden":
            Task.detached(priority: .userInitiated) {
                await runPlanGolden()
            }
        case "map-smoke":
            Task.detached(priority: .userInitiated) {
                await runMapSmoke(store: store)
            }
        case "editor-smoke":
            Task.detached(priority: .userInitiated) {
                await runEditorSmoke(store: store)
            }
        default:
            break
        }
    }

    // Copied from PlannerKitTests/GoldenPlanTests.swift's goldenRequest().
    private static func goldenRequest() -> FfiPlanRequest {
        FfiPlanRequest(
            originLat: 49.6116, originLon: 6.1319,
            destLat: 52.3702, destLon: 4.8952,
            waypoints: [],
            departSoc: 0.90,
            arrivalMinSoc: 0.10,
            chargerArrivalMinSoc: 0.10,
            chargerMaxSoc: 0.80,
            stopsBias: 1.0,
            tempC: 20.0,
            headwindMs: 0.0,
            batteryWarmth: 1.0,
            offerStopFreeAlternative: false,
            vehicle: .ioniq5Lr2wd,
            referenceConsumptionWhPerKm: nil
        )
    }

    private static func report(_ name: String, _ ok: Bool, _ detail: String = "") {
        let status = ok ? "PASS" : "FAIL"
        let suffix = detail.isEmpty ? "" : "(\(detail))"
        print("WAYFINDER-AUTOTEST: \(name) \(status)\(suffix)")
    }

    private static func isClose(_ actual: Double, _ expected: Double, tol: Double) -> Bool {
        abs(actual - expected) <= tol
    }

    /// `pct` as a fraction (0.05 == 5%).
    private static func isClosePct(_ actual: Double, _ expected: Double, pct: Double) -> Bool {
        abs(actual - expected) <= abs(expected) * pct
    }

    /// `sleepSeconds` is the screenshot window for editor-smoke: the external screenshot of
    /// the post-DONE UI state is taken while this runs.
    private static func finish(ok: Bool, sleepSeconds: Double = 1.0) async -> Never {
        print("WAYFINDER-AUTOTEST: DONE ok=\(ok)")
        try? await Task.sleep(nanoseconds: UInt64(sleepSeconds * 1_000_000_000))
        exit(ok ? 0 : 1)
    }

    private static func runPlanGolden() async {
        guard let located = Packs.locate(region: "corridor") else {
            report("pack-present", false, "Documents/corridor.rpack or corridor-chargers.json missing")
            await finish(ok: false)
        }
        report("pack-present", true)

        var ok = true
        do {
            let client = try PlannerClient(regionPackPath: located.rpackURL.path)
            let chargerBytes = try Data(contentsOf: located.chargersURL)
            try client.loadChargers(bytes: chargerBytes, format: "cpack-1")
            report("chargers-loaded", true)

            let start = DispatchTime.now()
            let plan = try await client.plan(goldenRequest())
            let elapsedMs = Double(DispatchTime.now().uptimeNanoseconds - start.uptimeNanoseconds) / 1_000_000
            print("WAYFINDER-AUTOTEST: cold plan ms=\(String(format: "%.1f", elapsedMs))")

            let stopCountOK = plan.stops.count == 1
            report("stop-count", stopCountOK, "expected 1, got \(plan.stops.count)")
            ok = ok && stopCountOK

            let expectedName = "Hyperfast charge laadpalen Nossegem Zaventem"
            let stopName = plan.stops.first?.name ?? "<none>"
            let nameOK = stopName == expectedName
            report("stop-name", nameOK, "expected \"\(expectedName)\", got \"\(stopName)\"")
            ok = ok && nameOK

            let arrivalSoc = plan.socCurve.last?.soc ?? -1
            let arrivalOK = isClose(arrivalSoc, 0.150, tol: 0.005)
            report("arrival-soc", arrivalOK, "expected 0.150 +/- 0.005, got \(arrivalSoc)")
            ok = ok && arrivalOK

            let totalOK = isClose(plan.totalTimeS, 14830, tol: 60)
            report("total-time-s", totalOK, "expected 14830 +/- 60, got \(plan.totalTimeS)")
            ok = ok && totalOK
        } catch {
            report("plan", false, "\(error)")
            ok = false
        }

        await finish(ok: ok)
    }

    // MARK: map-smoke

    /// Brings up the real map surface (via the app's shared PlanStore, same instance RootView
    /// renders) and checks: the style finishes loading, the pmtiles vector source is present,
    /// the Chargers layer is built from all 1,549 sideloaded chargers, and running the golden
    /// LU -> Amsterdam plan (same request as `runPlanGolden`) adds the route + stops layers.
    @MainActor
    private static func runMapSmoke(store: PlanStore) async {
        store.load()

        let styleLoaded = await waitWithTimeout(seconds: 20) { store.isStyleLoaded }
        report("style-loaded", styleLoaded)
        guard styleLoaded else { await finish(ok: false) }

        let pmtilesSourcePresent = store.mapView.style?.source(withIdentifier: "protomaps") != nil
        report("pmtiles-source-present", pmtilesSourcePresent)

        let chargersReady = await waitWithTimeout(seconds: 15) { store.chargerCount > 0 }
        let chargersOK = chargersReady && store.chargerCount == 1549
        report("chargers-count", chargersOK, "expected 1549, got \(store.chargerCount)")

        var ok = styleLoaded && pmtilesSourcePresent && chargersOK

        do {
            let plan = try await store.runPlan(goldenRequest())

            let routePresent = store.mapView.style?.layer(withIdentifier: RouteLayer.routeLineId) != nil
            report("route-layer-present", routePresent)
            ok = ok && routePresent

            let stopsPresent = store.mapView.style?.layer(withIdentifier: RouteLayer.stopsCirclesId) != nil
            let stopCountOK = plan.stops.count == 1
            report("stops-layer-present", stopsPresent && stopCountOK, "expected 1 stop, got \(plan.stops.count)")
            ok = ok && stopsPresent && stopCountOK
        } catch {
            report("route-layer-present", false, "\(error)")
            report("stops-layer-present", false)
            ok = false
        }

        await finish(ok: ok)
    }

    // MARK: editor-smoke

    /// Drives the route editor's store mutations directly (no UI-event injection exists on
    /// the sim; the UI itself is verified by screenshot). Proves: the LU -> Amsterdam golden
    /// via the editor's replan path, a Waypoint insertion (ADR 0010 point 4) against its own
    /// golden, that racing edits with no awaits between them land only the last replan (the
    /// generation guard in PlanStore.replan()), and that removing the Waypoint replans back
    /// to the LU -> Amsterdam golden.
    @MainActor
    private static func runEditorSmoke(store: PlanStore) async {
        // Pin the origin before anything else: setOrigin marks the origin overridden, so a
        // CoreLocation fix inside the corridor can never adopt it mid-test -- this is what
        // makes the goldens deterministic regardless of the host machine's location.
        store.setOrigin(CLLocationCoordinate2D(latitude: 49.6116, longitude: 6.1319))
        store.load()

        let ready = await waitWithTimeout(seconds: 30) { store.plannerStatus == .ready }
        report("planner-ready", ready)
        guard ready else { await finish(ok: false, sleepSeconds: 8) }

        var ok = true

        // Step 1: LU -> Amsterdam golden, via the editor's first destination selection.
        store.setDestination(name: "Amsterdam", coordinate: CLLocationCoordinate2D(latitude: 52.3702, longitude: 4.8952))
        let firstPlanLanded = await waitWithTimeout(seconds: 30) { store.planVersion == 1 }
        let (luAmsOk, luAmsDetail) = store.plan.map(assertLuAmsterdamGolden) ?? (false, "no plan")
        report("golden-lu-ams", firstPlanLanded && luAmsOk, firstPlanLanded ? luAmsDetail : "planVersion never reached 1")
        ok = ok && firstPlanLanded && luAmsOk

        // Step 2: race three replans with no awaits between them -- add Antwerp as a
        // Waypoint (starts a new, cold corridor assembly, ~1s), remove it, add it again.
        // Only the generation guard in PlanStore.replan() should let the last one land.
        let antwerp = CLLocationCoordinate2D(latitude: 51.2194, longitude: 4.4025)
        store.addWaypoint(name: "Antwerp", coordinate: antwerp)
        if let racedAwayId = store.waypoints.last?.id {
            store.removeWaypoint(id: racedAwayId)
        }
        store.addWaypoint(name: "Antwerp", coordinate: antwerp)

        let racesLanded = await waitWithTimeout(seconds: 60) {
            !store.isPlanning && (store.plan.map { assertWaypointGolden($0).0 } ?? false)
        }
        let (waypointGoldenOk, waypointGoldenDetail) = store.plan.map(assertWaypointGolden) ?? (false, "no plan")
        let waypointCountOk = store.waypoints.count == 1
        report(
            "golden-waypoint-after-races", racesLanded && waypointGoldenOk && waypointCountOk,
            racesLanded
                ? (waypointCountOk ? "" : "waypoints.count=\(store.waypoints.count), want 1")
                : "isPlanning=\(store.isPlanning): \(waypointGoldenDetail)"
        )
        ok = ok && racesLanded && waypointGoldenOk && waypointCountOk

        // Step 3: remove the Waypoint; the editor replans back to the LU -> Amsterdam golden.
        let versionBeforeRemoval = store.planVersion
        if let waypointId = store.waypoints.first?.id {
            store.removeWaypoint(id: waypointId)
        }
        let removalLanded = await waitWithTimeout(seconds: 30) {
            store.planVersion > versionBeforeRemoval && !store.isPlanning
        }
        let (removalGoldenOk, removalDetail) = store.plan.map(assertLuAmsterdamGolden) ?? (false, "no plan")
        report(
            "waypoint-removed-replans", removalLanded && removalGoldenOk,
            removalLanded ? removalDetail : "plan never landed after removal"
        )
        ok = ok && removalLanded && removalGoldenOk

        // The 8s post-DONE window is when the external screenshot of the planned-route
        // editor state (LU -> Amsterdam, Waypoint removed) is taken.
        await finish(ok: ok, sleepSeconds: 8)
    }

    /// Pinned observed values, LU -> Amsterdam, depart 0.90 (same golden as plan-golden).
    private static func assertLuAmsterdamGolden(_ plan: FfiPlan) -> (Bool, String) {
        var failures: [String] = []
        if plan.stops.count != 1 { failures.append("stops.count=\(plan.stops.count), want 1") }
        let stopName = plan.stops.first?.name ?? "<none>"
        let expectedName = "Hyperfast charge laadpalen Nossegem Zaventem"
        if stopName != expectedName { failures.append("stop name=\"\(stopName)\"") }
        if !isClose(plan.totalTimeS, 14830, tol: 60) { failures.append("totalTimeS=\(plan.totalTimeS)") }
        let arrivalSoc = plan.socCurve.last?.soc ?? -1
        if !isClose(arrivalSoc, 0.150, tol: 0.005) { failures.append("arrivalSoc=\(arrivalSoc)") }
        return (failures.isEmpty, failures.joined(separator: "; "))
    }

    /// Pinned observed values, LU -> Antwerp Waypoint -> Amsterdam, depart 0.90 (matches
    /// core/optimiser/tests/golden_corridor.rs's `golden_waypoint`).
    private static func assertWaypointGolden(_ plan: FfiPlan) -> (Bool, String) {
        var failures: [String] = []
        if plan.stops.count != 1 { failures.append("stops.count=\(plan.stops.count), want 1") }
        let stopName = plan.stops.first?.name ?? "<none>"
        if !stopName.contains("Nossegem") { failures.append("stop name=\"\(stopName)\"") }
        let chargeS = plan.stops.first?.chargeS ?? -1
        if !isClosePct(chargeS, 1033, pct: 0.10) { failures.append("chargeS=\(chargeS)") }
        if !isClosePct(plan.totalTimeS, 15449, pct: 0.05) { failures.append("totalTimeS=\(plan.totalTimeS)") }
        let arrivalSoc = plan.socCurve.last?.soc ?? -1
        if !isClose(arrivalSoc, 0.174, tol: 0.005) { failures.append("arrivalSoc=\(arrivalSoc)") }
        return (failures.isEmpty, failures.joined(separator: "; "))
    }

    /// Polls `condition` every 100ms until it's true or `seconds` elapses.
    @MainActor
    private static func waitWithTimeout(seconds: Double, until condition: () -> Bool) async -> Bool {
        let deadline = DispatchTime.now() + seconds
        while !condition() {
            if DispatchTime.now() >= deadline { return condition() }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        return true
    }
}
