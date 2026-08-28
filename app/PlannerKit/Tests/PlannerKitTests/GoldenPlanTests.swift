import XCTest
@testable import PlannerKit

/// The golden-plan gate (wayfinder #34), Swift side of ADR 0004's DoD: the
/// same LU->Amsterdam corridor `optimiser`'s own
/// `core/optimiser/tests/golden_corridor.rs::golden_lu_amsterdam` pins,
/// exercised end-to-end through the xcframework boundary. Needs the real
/// `~/abrp-data/dist/corridor` artifacts (like that Rust test); skipped
/// when they aren't present rather than failing CI.
final class GoldenPlanTests: XCTestCase {
    private func distRoot() throws -> URL {
        let home = FileManager.default.homeDirectoryForCurrentUser
        return home.appendingPathComponent("abrp-data/dist/corridor")
    }

    private func makeClient() throws -> PlannerClient? {
        let root = try distRoot()
        let packPath = root.appendingPathComponent("corridor.rpack").path
        guard FileManager.default.fileExists(atPath: packPath) else {
            return nil
        }
        let client = try PlannerClient(regionPackPath: packPath)
        let chargersURL = root.appendingPathComponent("corridor-chargers.json")
        let bytes = try Data(contentsOf: chargersURL)
        try client.loadChargers(bytes: bytes, format: "cpack-1")
        return client
    }

    private func goldenRequest() -> FfiPlanRequest {
        // LU -> Amsterdam, depart 0.90, floors 0.10/0.10, max 0.80, bias
        // 1.0, warmth 1.0, 20C, 2WD -- matches
        // `golden_corridor.rs::golden_lu_amsterdam` exactly.
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

    private func assertClose(_ actual: Double, _ expected: Double, tol: Double, _ what: String, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertLessThanOrEqual(abs(actual - expected), tol, "\(what): expected \(expected) +/- \(tol), got \(actual)", file: file, line: line)
    }

    private func assertPctClose(_ actual: Double, _ expected: Double, pct: Double, _ what: String, file: StaticString = #filePath, line: UInt = #line) {
        assertClose(actual, expected, tol: abs(expected) * pct / 100.0, what, file: file, line: line)
    }

    func testGoldenLuAmsterdam() async throws {
        guard let client = try makeClient() else {
            throw XCTSkip("~/abrp-data/dist/corridor artifacts not present (local-only, like the Rust golden test)")
        }

        let plan = try await client.plan(goldenRequest())

        XCTAssertEqual(plan.stops.count, 1)
        XCTAssertEqual(plan.stops[0].name, "Hyperfast charge laadpalen Nossegem Zaventem")
        assertClose(plan.stops[0].arrivalSoc, 0.129, tol: 0.02, "stop 0 arrival soc")
        assertClose(plan.stops[0].departSoc, 0.765, tol: 0.02, "stop 0 depart soc")
        assertPctClose(plan.stops[0].chargeS, 966.0, pct: 10.0, "stop 0 charge_s")
        assertPctClose(plan.totalTimeS, 14830.0, pct: 5.0, "total_time_s")
        assertPctClose(plan.totalDistM, 414_871.0, pct: 2.0, "total_dist_m")
        XCTAssertTrue(plan.flags.isEmpty, "expected a feasible plan: \(plan.flags)")

        // Polyline: non-empty, starts near LU and ends near Amsterdam.
        XCTAssertFalse(plan.polyline.isEmpty, "polyline should not be empty")
        let first = plan.polyline.first!
        let last = plan.polyline.last!
        XCTAssertLessThan(abs(first.lat - 49.6116), 0.2, "polyline should start near Luxembourg")
        XCTAssertLessThan(abs(first.lon - 6.1319), 0.2, "polyline should start near Luxembourg")
        XCTAssertLessThan(abs(last.lat - 52.3702), 0.2, "polyline should end near Amsterdam")
        XCTAssertLessThan(abs(last.lon - 4.8952), 0.2, "polyline should end near Amsterdam")

        // SoC curve: monotone-decreasing except for exactly one upward jump
        // (the single Charging Stop's post-charge SoC).
        XCTAssertFalse(plan.socCurve.isEmpty, "soc_curve should not be empty")
        var increases = 0
        for i in 1..<plan.socCurve.count {
            if plan.socCurve[i].soc > plan.socCurve[i - 1].soc + 1e-9 {
                increases += 1
            }
        }
        XCTAssertEqual(increases, 1, "expected exactly one upward SoC jump (the Charging Stop)")
    }

    func testCancelBeforePlanThrowsCancelled() async throws {
        guard let client = try makeClient() else {
            throw XCTSkip("~/abrp-data/dist/corridor artifacts not present (local-only, like the Rust golden test)")
        }

        // `Planner::plan` clears the cancel flag as its very first action
        // (see its doc comment / `plan_with_cancel` in
        // `core/optimiser/src/plan_api.rs`), so a single `cancel()` call
        // made before starting `plan()` is NOT deterministic: it just gets
        // cleared immediately. Racing a single `cancel()` against a
        // just-started `plan()` isn't deterministic either (dispatch
        // latency to actually start running the detached call vs. the
        // clear-then-heavy-corridor-assembly work on the Rust side can
        // land either way). Instead, start `plan()` as a child task and
        // spam `cancel()` at it for the whole time it's running: the
        // corridor assembly for this route takes tens of ms of real work
        // after the clear, so one of many repeated calls is virtually
        // guaranteed to land inside that window and be observed by
        // `corridor::assemble`'s per-pair cancellation check.
        //
        // `request` is built ahead of time (rather than calling
        // `goldenRequest()` inline below) so the `async let` closure only
        // captures Sendable values (`client`, `request`), not task-isolated
        // `self` -- otherwise Swift 6 language mode rejects sending `self`
        // into the child task.
        let request = goldenRequest()
        async let result: FfiPlan = client.plan(request)

        let cancelTask = Task {
            for _ in 0..<200 {
                if Task.isCancelled { break }
                client.cancel()
                try? await Task.sleep(nanoseconds: 10_000_000) // 10ms
            }
        }

        do {
            _ = try await result
            cancelTask.cancel()
            XCTFail("expected plan() to throw Cancelled")
        } catch PlannerError.Cancelled(_) {
            cancelTask.cancel()
        } catch {
            cancelTask.cancel()
            throw error
        }
    }
}
