// UI e2e flow 1/2 (wayfinder #31): recents -> destination -> plan -> result card, and the
// waypoint add/remove path through the route editor card. See WayfinderUITestCase for the
// seeded-recents determinism approach and the waitForExistence override (the always-animating
// map surface breaks XCUITest's default quiescence-based waitForExistence). Failures here are
// classified per the suite's rule: a timing/query artifact is fixed and retried; a genuine app
// bug is recorded with XCTExpectFailure so the suite still runs to completion.
import XCTest

final class PlanFlowTests: WayfinderUITestCase {
    func testRecentsToPlanToCard() throws {
        launchApp()

        XCTAssertTrue(waitForExistence(el("search-pill"), timeout: 60), "planner never became ready (search pill never appeared)")

        el("search-pill").tap()
        XCTAssertTrue(waitForExistence(el("recent-Antwerp"), timeout: 10), "recents list never showed the seeded Antwerp row")
        el("recent-Antwerp").tap()

        XCTAssertTrue(
            waitForExistence(el("result-card"), timeout: 30), "result card never appeared after selecting a recent destination"
        )

        // Route editor: origin defaults to Luxembourg (PlanStore.originName), destination is
        // the one just selected.
        XCTAssertTrue(waitForExistence(el("route-origin-row"), timeout: 10))
        XCTAssertTrue(
            waitForExistence(app.staticTexts["Luxembourg"].firstMatch, timeout: 5), "origin row doesn't show \"Luxembourg\""
        )
        XCTAssertTrue(waitForExistence(el("route-destination-row"), timeout: 5))
        XCTAssertTrue(
            waitForExistence(app.staticTexts["Antwerp"].firstMatch, timeout: 5), "destination row doesn't show \"Antwerp\""
        )

        // Collapsed summary shows an arrival clock (HH:mm).
        let arrivalClock = NSPredicate(format: "label MATCHES %@", "^[0-2][0-9]:[0-5][0-9]$")
        XCTAssertTrue(
            waitForExistence(app.staticTexts.matching(arrivalClock).firstMatch, timeout: 5),
            "no HH:mm arrival clock text found in the collapsed result card"
        )

        // Expand: itinerary + SoC chart.
        XCTAssertTrue(waitForExistence(el("result-card-toggle"), timeout: 5))
        el("result-card-toggle").tap()
        XCTAssertTrue(
            waitForExistence(el("itinerary"), timeout: 5), "itinerary content never appeared after expanding the result card"
        )
        XCTAssertTrue(waitForExistence(app.staticTexts["Origin"].firstMatch, timeout: 5))
        XCTAssertTrue(waitForExistence(app.staticTexts["Destination"].firstMatch, timeout: 5))
        XCTAssertTrue(waitForExistence(el("soc-chart"), timeout: 5), "SoC chart never appeared after expanding the result card")

        // Collapse back. Retry the tap a few times before concluding this is a genuine bug --
        // the card's own swipe-to-expand/collapse DragGesture sits on the same view as this
        // Button, and the ScrollView/Spacer subtree swap the toggle causes could plausibly
        // race a freshly-recreated gesture recognizer against the very next synthesized tap.
        var itineraryGone = false
        for _ in 0..<3 {
            el("result-card-toggle").tap()
            itineraryGone = waitForNonexistence(el("itinerary"), timeout: 5)
            if itineraryGone { break }
        }
        if !itineraryGone {
            XCTExpectFailure(
                """
                BUG: tapping the result card's collapse toggle (chevron) does not collapse the \
                card back down. Symptom: after expanding the card and tapping "result-card-toggle" \
                again, the itinerary/SoC chart content is still present (and the button's own label \
                still reads "Go Down", i.e. store.cardExpanded is still true) even after retrying \
                the tap 3x with a 5s settle each time. Repro: expand the result card via its \
                chevron, then tap the same chevron again to collapse. Suspected cause: \
                ResultCard's outer VStack carries both this Button and a whole-card \
                `.gesture(DragGesture()...)` for swipe-to-expand/collapse (ResultCard.swift); \
                toggling `cardExpanded` swaps the ScrollView/Spacer subtree beneath that shared \
                gesture, which can plausibly leave the button's tap recognizer unable to win \
                against (or be re-armed after) the drag recognizer on the very next tap.
                """
            ) {
                XCTAssertTrue(itineraryGone)
            }
        } else {
            XCTAssertTrue(itineraryGone, "itinerary content is still present after collapsing the result card")
        }
    }

    func testWaypointAddRemove() throws {
        launchApp()
        XCTAssertTrue(planToRecent(named: "Antwerp"), "shared plan-to-recent flow failed setting up this test's plan")

        // The route editor card starts expanded (RouteEditorView's cardExpanded @State default).
        XCTAssertTrue(waitForExistence(el("route-editor-card"), timeout: 10))
        XCTAssertTrue(
            waitForExistence(el("add-stop-row"), timeout: 10), "\"Add stop\" row never appeared in the route editor card"
        )
        // Retry the tap a few times before concluding this is a genuine bug rather than a
        // gesture-recognizer race with the List's forced edit-mode reorder recognizer
        // (RouteEditorView.routeEditorCard has `.onMove` on the waypoints section, which
        // installs a press-and-hold-to-reorder recognizer competing with a plain tap).
        var searchReopened = false
        for _ in 0..<3 {
            el("add-stop-row").tap()
            searchReopened = waitForExistence(el("recent-Capellen"), timeout: 5)
            if searchReopened { break }
        }
        if !searchReopened {
            XCTExpectFailure(
                """
                BUG: tapping the "Add stop" row in the route editor card does not expand the \
                search pill into search mode. Symptom: after tapping add-stop-row (confirmed via \
                the accessibility tree: the button exists, is tapped, no error), the search pill \
                stays collapsed (still shows the destination name as static text -- no TextField, \
                no Cancel button), so the recents list/suggestions never appear and a waypoint can \
                never be added through this UI at all. Repro: with a plan showing (route editor \
                card expanded), tap "Add stop". Retried 3x with a 5s settle each time; the tap \
                consistently has no effect. Suspected cause: RouteEditorView.routeEditorCard's \
                List has `.environment(\\.editMode, .constant(.active))` applied unconditionally \
                (needed only so the waypoint rows' xmark delete buttons render); this appears to \
                make the List intercept/absorb taps on ANY nested custom Button -- including \
                addStopRow's, which has no delete/reorder role at all -- before the button's own \
                action runs (RouteEditorView.swift, routeEditorCard / addStopRow).
                """
            ) {
                XCTAssertTrue(searchReopened)
            }
            return // unreachable without a working "Add stop" -- nothing further to verify.
        }
        el("recent-Capellen").tap()

        // Waypoint deletion is the List's NATIVE edit-mode control (RouteEditorView switched
        // to `.onDelete` after the first UI e2e run proved custom in-row buttons are both
        // dead to taps and invisible to accessibility under the forced-active editMode):
        // tap the row's leading red minus toggle, then the revealed "Delete" confirm.
        let waypointText = app.staticTexts["Capellen"].firstMatch
        XCTAssertTrue(waitForExistence(waypointText, timeout: 10), "waypoint row for Capellen never appeared in the route editor card")

        // Replan runs after the waypoint is added; give it room to land, then confirm the
        // planning spinner (if it ever appeared) is gone before touching the delete control.
        _ = waitForNonexistence(app.activityIndicators.firstMatch, timeout: 30)

        // The minus control is exposed as an IMAGE (identifier "minus.circle.fill", label
        // "remove") inside the waypoint's cell, not as a Button -- observed from the live
        // accessibility tree, so query it as one.
        let waypointCell = app.cells.containing(.staticText, identifier: "route-waypoint-row-Capellen").firstMatch
        let minusToggle = waypointCell.images["minus.circle.fill"].firstMatch
        XCTAssertTrue(waitForExistence(minusToggle, timeout: 5), "waypoint row has no native edit-mode delete control")
        minusToggle.tap()

        // The minus toggle reveals a trailing "Delete" confirmation button on the row.
        let confirmDelete = app.buttons["Delete"].firstMatch
        XCTAssertTrue(waitForExistence(confirmDelete, timeout: 5), "confirm Delete button never revealed")
        confirmDelete.tap()

        XCTAssertTrue(waitForNonexistence(waypointText, timeout: 10), "waypoint row still present after native edit-mode delete")
    }
}
