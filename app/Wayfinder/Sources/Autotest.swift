// Launch-argument autotest (wayfinder #39): `--autotest plan-golden` runs the
// same LU -> Amsterdam golden request as
// PlannerKitTests/GoldenPlanTests.swift, off the main thread, against the
// pack sideloaded into Documents, and exits with the pass/fail result. This
// proves a plan() round-trip through the real xcframework on the simulator,
// independent of the (placeholder) UI.
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

    private static func finish(ok: Bool) async -> Never {
        print("WAYFINDER-AUTOTEST: DONE ok=\(ok)")
        try? await Task.sleep(nanoseconds: 1_000_000_000)
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
