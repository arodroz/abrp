// Regular-user map interaction (no scripted flow, no custom launch arguments beyond what
// XCUITest itself requires): pan, pinch, double-tap, rotate, long-press, rapid multi-finger
// mashing on the map surface. Exists because the button-flow tests never touched the map
// itself, and real use crashed repeatedly (SIGABRT in UIKit's delayed-touch bookkeeping:
// `-[UIGestureRecognizer _delayTouchesForEvent:inPhase:]` -> `insertObject:atIndex:` throwing
// -- five identical reports on 2026-08-30, one predating that day's UI changes). The only
// assertion that matters here is "the app is still running".
import XCTest

final class MapGestureTests: WayfinderUITestCase {
    /// Launches with NO app-level launch arguments -- as close to an icon tap as XCUITest
    /// gets -- so persisted state and default code paths match a real user's session.
    private func launchBare() {
        app = XCUIApplication()
        addUIInterruptionMonitor(withDescription: "Location permission") { alert in
            for label in ["Allow While Using App", "Allow Once", "OK"] {
                let button = alert.buttons[label]
                if button.exists {
                    button.tap()
                    return true
                }
            }
            return false
        }
        app.launch()
    }

    private func assertStillRunning(_ what: String) {
        XCTAssertEqual(app.state, .runningForeground, "app is no longer in the foreground after \(what) (crashed?)")
    }

    /// The three real crashes on 2026-08-30 came 70s and 11s apart -- crash, relaunch, crash
    /// again within seconds. That is a user touching the map DURING startup (style/planner
    /// still loading), not after politely waiting for it to settle like the flow tests do.
    /// Coordinate-based events only: element-level multi-touch gestures (twoFingerTap/
    /// pinch/rotate) run an occlusion check that fails against the full-window startup
    /// overlay ("Unable to compute coordinates for gesture after 5 attempts"), aborting
    /// the test before the app is ever touched. Coordinate events skip that check -- and
    /// match the real crashes, which were plain taps during startup. Post-settle
    /// multi-touch coverage lives in testMapMashingLikeARegularUser.
    func testImmediateMashingAtLaunch() throws {
        launchBare()
        let window = app.windows.firstMatch
        XCTAssertTrue(window.waitForExistence(timeout: 15))

        // No settling wait: straight onto the glass from the first frame.
        for round in 1...4 {
            for i in 0..<8 {
                window.coordinate(
                    withNormalizedOffset: CGVector(dx: 0.25 + 0.08 * Double(i % 5), dy: 0.3 + 0.07 * Double(i % 4))
                ).tap()
            }
            window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.45)).doubleTap()
            window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).press(forDuration: 0.6)
            window.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.6))
                .press(forDuration: 0.05, thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: 0.7, dy: 0.4)))
            window.coordinate(withNormalizedOffset: CGVector(dx: 0.7, dy: 0.3))
                .press(forDuration: 0.05, thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.7)))
            assertStillRunning("immediate coordinate mashing (round \(round))")
        }
    }

    /// Flows the button-tap tests never exercised: dragging the result card (its SwiftUI
    /// DragGesture bridges to a UIKit recognizer that participates in touch-delay
    /// bookkeeping), scrubbing the SoC chart, dragging the settings sheet between detents,
    /// and typing into the live search field.
    func testCardSheetAndSearchLikeARegularUser() throws {
        launchApp() // seeded recents -- the interaction under test is gestures, not search determinism

        XCTAssertTrue(waitForExistence(el("search-pill"), timeout: 60))
        el("search-pill").tap()
        XCTAssertTrue(waitForExistence(el("recent-Antwerp"), timeout: 10))
        el("recent-Antwerp").tap()
        XCTAssertTrue(waitForExistence(el("result-card"), timeout: 60))

        let window = app.windows.firstMatch
        let card = el("result-card")

        // Drag the card up/down/partially, fling it, repeatedly -- the human gesture the
        // chevron-tap tests bypassed.
        for round in 1...3 {
            card.swipeUp()
            assertStillRunning("card swipe up (round \(round))")
            card.swipeDown()
            assertStillRunning("card swipe down (round \(round))")
            let start = card.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.2))
            let mid = window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5))
            start.press(forDuration: 0.1, thenDragTo: mid)
            assertStillRunning("card partial drag (round \(round))")
            // Scrub across the expanded card region (SoC chart drag) then poke the map.
            mid.press(forDuration: 0.1, thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.5)))
            window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.3)).tap()
            assertStillRunning("scrub + map poke (round \(round))")
        }

        // Settings sheet: open, drag between detents, swipe-dismiss.
        el("settings-button").tap()
        let sheet = app.otherElements["route-editor-card"] // anchor no longer visible; just use window drags
        _ = sheet
        Thread.sleep(forTimeInterval: 1)
        for round in 1...2 {
            window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.55))
                .press(forDuration: 0.1, thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.15)))
            assertStillRunning("sheet drag up (round \(round))")
            window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.15))
                .press(forDuration: 0.1, thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.6)))
            assertStillRunning("sheet drag down (round \(round))")
        }
        window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.55))
            .press(forDuration: 0.1, thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.98)))
        assertStillRunning("sheet swipe dismiss")

        // Live search typing: tap the pill, type, let MKLocalSearchCompleter fire, clear it.
        el("search-pill").tap()
        let field = app.textFields.firstMatch
        if waitForExistence(field, timeout: 5) {
            field.typeText("Antwerpen")
            Thread.sleep(forTimeInterval: 2)
            field.typeText("\n")
        }
        assertStillRunning("live search typing")
    }

    func testMapMashingLikeARegularUser() throws {
        launchBare()

        let map = app.otherElements.firstMatch
        XCTAssertTrue(map.waitForExistence(timeout: 30))
        // Give the map style + chargers a moment, like a human staring at the screen first.
        Thread.sleep(forTimeInterval: 3)

        let window = app.windows.firstMatch

        for round in 1...3 {
            window.swipeLeft()
            window.swipeUp()
            assertStillRunning("pan (round \(round))")

            window.pinch(withScale: 2.0, velocity: 8)
            assertStillRunning("pinch out (round \(round))")
            window.pinch(withScale: 0.4, velocity: -8)
            assertStillRunning("pinch in (round \(round))")

            window.doubleTap()
            assertStillRunning("double tap (round \(round))")

            window.rotate(.pi / 4, withVelocity: 2)
            assertStillRunning("rotate (round \(round))")

            // Long-press drops an origin pin (RootView.onLongPress -> setOrigin).
            window.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.45)).press(forDuration: 0.8)
            assertStillRunning("long press (round \(round))")

            // Rapid mashing: quick taps and tiny swipes interleaved, the way a fidgeting
            // human (or a pocket) actually hits a map.
            for i in 0..<6 {
                let c = window.coordinate(
                    withNormalizedOffset: CGVector(dx: 0.3 + 0.07 * Double(i % 4), dy: 0.35 + 0.06 * Double(i % 3)))
                c.tap()
                c.press(forDuration: 0.05, thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: 0.6, dy: 0.5)))
            }
            assertStillRunning("rapid mashing (round \(round))")
        }
    }
}
