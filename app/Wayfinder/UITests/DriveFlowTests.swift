// Drive Mode core UI e2e (wayfinder #59, ADR 0012 points 1-3 and 8): Go button gating both
// ways (current-location origin shows it, a long-press/remote origin hides it), entering drive
// hides the planning UI in favor of the drive-controls bar, free-look kicks in on a map gesture
// (Re-center button appears) and recentering removes it again, and End returns to planning with
// the Plan intact. Camera framing itself isn't assertable from XCUITest -- the overview button
// tap only proves it's tappable. Every launch here passes `-simulatedLocationFix "lat,lon"`
// (WayfinderUITestCase.launchApp's extraLaunchArguments): XCUITest cannot inject CoreLocation
// fixes, so PlanStore.load() reads this launch argument and adopts it as the current-location
// origin directly (see PlanStore.swift's e2e seam comment) -- the real CLLocationManager
// adoption path is covered by drive-smoke and the M4-gate device drives instead.
import XCTest

final class DriveFlowTests: WayfinderUITestCase {
    func testGoEntersAndEndExitsDrive() throws {
        launchApp(extraLaunchArguments: ["-simulatedLocationFix", "49.6116,6.1319"])
        XCTAssertTrue(planToRecent(named: "Antwerp"), "shared plan-to-recent flow failed setting up this test's plan")

        XCTAssertTrue(
            waitForExistence(el("go-button"), timeout: 15), "go button never appeared for a current-location-origin plan"
        )
        el("go-button").tap()

        XCTAssertTrue(waitForExistence(el("drive-end-button"), timeout: 10), "drive-end-button never appeared after tapping Go")
        XCTAssertTrue(waitForNonexistence(el("search-pill"), timeout: 5), "search-pill still present after entering drive")
        XCTAssertTrue(waitForNonexistence(el("settings-button"), timeout: 5), "settings-button still present after entering drive")
        XCTAssertTrue(waitForNonexistence(el("result-card"), timeout: 5), "result-card still present after entering drive")

        // Free-look: any map gesture (a coordinate drag, like MapGestureTests) should reveal
        // the Re-center button.
        let window = app.windows.firstMatch
        window.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.6))
            .press(forDuration: 0.1, thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: 0.7, dy: 0.4)))
        XCTAssertTrue(
            waitForExistence(el("drive-recenter-button"), timeout: 10), "drive-recenter-button never appeared after a map gesture"
        )

        el("drive-recenter-button").tap()
        XCTAssertTrue(
            waitForNonexistence(el("drive-recenter-button"), timeout: 10), "drive-recenter-button still present after tapping it"
        )

        // Just proves it's tappable -- camera framing isn't assertable from XCUITest.
        el("drive-overview-button").tap()

        el("drive-end-button").tap()
        XCTAssertTrue(waitForExistence(el("search-pill"), timeout: 10), "search-pill never returned after End")
        XCTAssertTrue(waitForExistence(el("result-card"), timeout: 10), "result-card never returned after End")
        XCTAssertTrue(waitForExistence(el("go-button"), timeout: 10), "go-button never returned after End")
    }

    /// The Go gate is provenance-based (ADR 0012 point 2): a long-press origin is by definition
    /// not the current location, so it must close the gate even though a plan still exists.
    func testNoGoOnRemoteOriginPlan() throws {
        launchApp(extraLaunchArguments: ["-simulatedLocationFix", "49.6116,6.1319"])
        XCTAssertTrue(planToRecent(named: "Antwerp"), "shared plan-to-recent flow failed setting up this test's plan")
        XCTAssertTrue(
            waitForExistence(el("go-button"), timeout: 15), "go button never appeared for a current-location-origin plan"
        )

        // Long-press the map, away from both the route editor card (top) and the result card
        // (bottom) -- drops an origin pin and triggers a replan (RootView.onLongPress ->
        // PlanStore.setOrigin).
        let window = app.windows.firstMatch
        window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).press(forDuration: 0.8)

        XCTAssertTrue(
            waitForNonexistence(el("go-button"), timeout: 10), "go-button still present after a long-press (remote) origin override"
        )
    }
}
