// Drive Mode core UI e2e (wayfinder #59, ADR 0012 points 1-3 and 8): Go button gating both
// ways (current-location origin shows it, a long-press/remote origin hides it), entering drive
// hides the planning UI in favor of the drive-controls bar, free-look kicks in on a map gesture
// (Re-center button appears) and recentering removes it again, and End returns to planning with
// the Plan intact. Camera framing itself isn't assertable from XCUITest -- the overview button
// tap only proves it's tappable. Every launch here passes `-simulatedLocationFix "lat,lon"`
// (WayfinderUITestCase.launchApp's extraLaunchArguments): XCUITest cannot inject CoreLocation
// fixes, so PlanStore.load() reads this launch argument and adopts it as the current-location
// origin directly (see PlanStore.swift's e2e seam comment) -- the real CLLocationManager
// adoption path is covered by drive-smoke and the M4-gate device drives instead. The drive HUD
// card (wayfinder #60) is only checked for presence and its expand/collapse toggle here -- HUD
// VALUE updates (ETA, remaining distance, SoC) aren't assertable from XCUITest since it can't
// inject CoreLocation fixes; that's drive-smoke's job (its "hud-initial"/"hud-updates" asserts).
// Trip Log coupling (wayfinder #62, ADR 0012 point 7): Go now opens the "Trip start SoC" alert
// before entering, handled with TripLogFlowTests' exact alert idiom; End opens "Trip end SoC",
// which this test Cancels deliberately (see that step's own comment) so no tlog file lands in
// the shared simulator container for other suites to trip over. Manual mid-drive SoC correction
// (wayfinder #63) is covered too, but only as far as tapping the card's SoC number opens and
// Cancels the "Correct SoC" alert -- the replan itself needs a snapped position drive-smoke
// provides.
import XCTest

final class DriveFlowTests: WayfinderUITestCase {
    func testGoEntersAndEndExitsDrive() throws {
        launchApp(extraLaunchArguments: ["-simulatedLocationFix", "49.6116,6.1319"])
        XCTAssertTrue(planToRecent(named: "Antwerp"), "shared plan-to-recent flow failed setting up this test's plan")

        XCTAssertTrue(
            waitForExistence(el("go-button"), timeout: 15), "go button never appeared for a current-location-origin plan"
        )
        el("go-button").tap()

        // Go now opens the Trip Log's start-SoC prompt (wayfinder #62, ADR 0012 point 7) before
        // entering -- same alert-handling idiom as TripLogFlowTests' start prompt.
        let startAlert = app.alerts["Trip start SoC"]
        XCTAssertTrue(waitForExistence(startAlert, timeout: 10), "\"Trip start SoC\" alert never appeared after tapping Go")
        startAlert.textFields.firstMatch.typeText("80")
        startAlert.buttons["OK"].tap()
        XCTAssertTrue(waitForNonexistence(startAlert, timeout: 5), "\"Trip start SoC\" alert is still up after OK")

        XCTAssertTrue(waitForExistence(el("drive-end-button"), timeout: 10), "drive-end-button never appeared after tapping Go")
        XCTAssertTrue(waitForNonexistence(el("search-pill"), timeout: 5), "search-pill still present after entering drive")
        XCTAssertTrue(waitForNonexistence(el("settings-button"), timeout: 5), "settings-button still present after entering drive")
        XCTAssertTrue(waitForNonexistence(el("result-card"), timeout: 5), "result-card still present after entering drive")

        // Drive HUD card (wayfinder #60): present once driving, its chevron expands/collapses
        // the pinned SoC chart.
        XCTAssertTrue(waitForExistence(el("drive-card"), timeout: 10), "drive-card never appeared after entering drive")
        el("drive-card-toggle").tap()
        XCTAssertTrue(waitForExistence(el("soc-chart"), timeout: 10), "soc-chart never appeared after expanding the drive card")

        // Collapse back. Retry the tap a few times before concluding this is a genuine bug --
        // same rationale as PlanFlowTests' result-card-toggle retry: a tap synthesized right as
        // the expand spring animation is still resizing the card can miss the (moving) chevron.
        var chartGone = false
        for _ in 0..<3 {
            el("drive-card-toggle").tap()
            chartGone = waitForNonexistence(el("soc-chart"), timeout: 5)
            if chartGone { break }
        }
        XCTAssertTrue(chartGone, "soc-chart still present after collapsing the drive card, even after 3 retries")

        // Manual SoC correction (wayfinder #63): the card's SoC number opens the dash-SoC alert.
        // Cancel deliberately -- XCUITest can't inject fixes, so there's no snapped position for a
        // real correction to replan from; the replan path itself is drive-smoke's job.
        el("drive-soc-button").tap()
        let socAlert = app.alerts["Correct SoC"]
        XCTAssertTrue(waitForExistence(socAlert, timeout: 10), "\"Correct SoC\" alert never appeared after tapping the SoC number")
        socAlert.buttons["Cancel"].tap()
        XCTAssertTrue(waitForNonexistence(socAlert, timeout: 5), "\"Correct SoC\" alert is still up after Cancel")

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

        // End now closes the shared Trip Log capture with the end-SoC prompt (wayfinder #62).
        // Cancel it deliberately here, not OK: the data-loss guard resumes capture as a
        // standalone recording AND no tlog file is written into the shared simulator container,
        // which other suites' log expectations could otherwise see.
        let endAlert = app.alerts["Trip end SoC"]
        XCTAssertTrue(waitForExistence(endAlert, timeout: 10), "\"Trip end SoC\" alert never appeared after tapping End")
        endAlert.buttons["Cancel"].tap()
        XCTAssertTrue(waitForNonexistence(endAlert, timeout: 5), "\"Trip end SoC\" alert is still up after Cancel")

        XCTAssertTrue(waitForExistence(el("search-pill"), timeout: 10), "search-pill never returned after End")
        XCTAssertTrue(waitForExistence(el("result-card"), timeout: 10), "result-card never returned after End")
        XCTAssertTrue(waitForExistence(el("go-button"), timeout: 10), "go-button never returned after End")
        XCTAssertTrue(waitForExistence(el("trip-record-button"), timeout: 10), "trip-record-button never appeared after End")
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
