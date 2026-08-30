// Shared launch helper for the Wayfinder UI e2e suite (wayfinder #31 -- first UI-driven e2e
// pass). Determinism comes from two launch arguments read by the app itself: `-activeRegion
// corridor` (PlanStore's UserDefaults key, default "corridor" anyway -- set explicitly so a
// stale persisted override on the simulator can't change which pack loads) and
// `-recentDestinations <json>` (RouteEditorView's @AppStorage("recentDestinations") key),
// seeding the two recents flows 1/2 depend on so no test ever touches the live
// MKLocalSearchCompleter path. Location permission is pre-granted by the run script via
// `xcrun simctl privacy ... grant location org.anteras.wayfinder`; the interruption monitor
// below is only a backup in case a run starts before that lands.
import XCTest

@MainActor
class WayfinderUITestCase: XCTestCase {
    var app: XCUIApplication!

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    /// `name` -> (lat, lon) for the two seeded recents, reused by tests that need the raw
    /// coordinates (none currently do, but keeping this alongside the launch JSON avoids the
    /// two ever drifting apart).
    static let seededRecents: [(name: String, lat: Double, lon: Double)] = [
        ("Antwerp", 51.2194, 4.4025),
        ("Capellen", 49.645, 5.99),
    ]

    func launchApp() {
        app = XCUIApplication()
        let recentsJSON = Self.seededRecents.map { "{\"name\":\"\($0.name)\",\"lat\":\($0.lat),\"lon\":\($0.lon)}" }
            .joined(separator: ",")
        app.launchArguments = [
            "-activeRegion", "corridor", "-recentDestinations", Self.legacyPlistQuoted("[\(recentsJSON)]"),
        ]

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

    /// `-key value` launch arguments go through `NSUserDefaults`'s legacy-property-list
    /// argument parser, not a plain string assignment: a bare JSON array/object (starting with
    /// `[`/`{`, using `:`) fails that parse and the whole key is silently dropped -- confirmed
    /// empirically via `simctl launch --console-pty` (a plain scalar like "corridor" survives;
    /// the JSON recents payload came back `nil`). Wrapping the payload as a legacy-plist
    /// double-quoted string literal (escaping `\` then `"`) makes it parse as one NSString, so
    /// `UserDefaults.standard.string(forKey:)` gets back the original JSON text intact.
    private static func legacyPlistQuoted(_ value: String) -> String {
        let escaped = value.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
        return "\"\(escaped)\""
    }

    /// Matches by accessibilityIdentifier regardless of the SwiftUI-inferred XCUIElement type
    /// (an HStack with a tap gesture, a List row, a Button label, etc. don't all surface the
    /// same element type) -- every identifier added to the app for this suite is looked up this
    /// way so a type guess can never be the reason a lookup fails.
    func el(_ identifier: String) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: identifier).firstMatch
    }

    /// Drives the shared happy path flow 1 depends on (recents -> destination -> plan) so
    /// flow 2's waypoint test doesn't repeat it inline.
    @discardableResult
    func planToRecent(named name: String, plannerReadyTimeout: TimeInterval = 60) -> Bool {
        guard waitForExistence(el("search-pill"), timeout: plannerReadyTimeout) else { return false }
        el("search-pill").tap()
        guard waitForExistence(el("recent-\(name)"), timeout: 10) else { return false }
        el("recent-\(name)").tap()
        return waitForExistence(el("result-card"), timeout: 30)
    }

    /// XCUIElement has no built-in "wait until gone"; polls `exists` down to false instead of
    /// sleeping for the full timeout on the (common) case where it disappears quickly.
    @discardableResult
    func waitForNonexistence(_ element: XCUIElement, timeout: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if !element.exists { return true }
            RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        }
        return !element.exists
    }

    /// `XCUIElement.waitForExistence(timeout:)` waits for the app to report "quiescent" before
    /// each check, and the always-animating MapLibre surface (compass, tile/camera animation)
    /// underneath every screen in this app means that never happens -- confirmed empirically:
    /// a SwiftUI view that appears via `withAnimation` (e.g. the recents list right after the
    /// search pill expands) can fail a 10s `waitForExistence` outright, while a manual poll of
    /// the same `exists` check succeeds within a second. Every existence wait in this suite
    /// goes through this instead, except native alerts/action sheets, which aren't affected.
    @discardableResult
    func waitForExistence(_ element: XCUIElement, timeout: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if element.exists { return true }
            RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        }
        return element.exists
    }

    /// The settings sheet is a `Form` (UITableView-backed) inside a `.medium`/`.large` detent
    /// sheet: lower sections aren't laid out (so don't `exist`) until scrolled into view, and a
    /// swipe-up starting from the top of the sheet's content resizes the sheet itself before it
    /// starts scrolling the Form. Swiping repeatedly handles both.
    @discardableResult
    func scrollToElement(_ element: XCUIElement, maxSwipes: Int = 10) -> Bool {
        var attempts = 0
        while !(element.exists && element.isHittable), attempts < maxSwipes {
            app.swipeUp()
            attempts += 1
        }
        return element.exists && element.isHittable
    }
}
