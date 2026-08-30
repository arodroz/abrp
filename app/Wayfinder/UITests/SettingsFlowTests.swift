// UI e2e flow 3 (wayfinder #31): the settings sheet's sections, the Departure SoC slider, the
// Appearance picker, and the Data section's "Clear Recent Destinations" confirmation. See
// WayfinderUITestCase for the seeded-recents/pre-granted-location determinism approach and
// scrollToElement for why the Form-backed sheet needs scrolling before most lookups.
import XCTest

final class SettingsFlowTests: WayfinderUITestCase {
    func testSettingsControls() throws {
        launchApp()

        XCTAssertTrue(waitForExistence(el("search-pill"), timeout: 60), "planner never became ready (search pill never appeared)")
        XCTAssertTrue(waitForExistence(el("settings-button"), timeout: 5))
        el("settings-button").tap()

        XCTAssertTrue(waitForExistence(app.staticTexts["Trip Logs"], timeout: 10), "settings sheet never appeared")

        // Sections: Trip Logs may be empty but its header still renders; Calibration only
        // renders once a Trip Log exists, so it's deliberately not checked here. Packs/Battery
        // come before the slider check below, so only scroll-check the ones after it here.
        for sectionTitle in ["Trip Logs", "Packs", "Battery"] {
            XCTAssertTrue(
                scrollToElement(app.staticTexts[sectionTitle]),
                "settings sheet is missing (or never scrolled to) the \"\(sectionTitle)\" section"
            )
        }

        // Departure SoC slider: drag to ~0.3 and confirm the label text changed off the default.
        let departureLabel = el("departure-soc-label")
        XCTAssertTrue(scrollToElement(departureLabel), "departure-soc-label never scrolled into view")
        let originalLabel = departureLabel.label
        let slider = app.sliders["departure-soc-slider"]
        XCTAssertTrue(scrollToElement(slider), "departure-soc-slider identifier isn't attached to a hittable slider element")
        slider.adjust(toNormalizedSliderPosition: 0.22) // (0.3 - 0.1) / (1.0 - 0.1)
        XCTAssertNotEqual(departureLabel.label, originalLabel, "Departure SoC label text didn't change after dragging the slider")

        for sectionTitle in ["Route", "Conditions", "Vehicle", "Data", "Appearance"] {
            XCTAssertTrue(
                scrollToElement(app.staticTexts[sectionTitle]),
                "settings sheet is missing (or never scrolled to) the \"\(sectionTitle)\" section"
            )
        }

        // Appearance: flip to Dark, then back to Light.
        let darkButton = el("appearance-picker").buttons["Dark"]
        XCTAssertTrue(scrollToElement(darkButton), "Appearance picker's Dark segment never became hittable")
        darkButton.tap()
        let lightButton = el("appearance-picker").buttons["Light"]
        XCTAssertTrue(waitForExistence(lightButton, timeout: 5))
        lightButton.tap()

        // Data: "Clear Recent Destinations" + confirmation dialog.
        let clearButton = app.buttons["Clear Recent Destinations"]
        XCTAssertTrue(scrollToElement(clearButton), "Clear Recent Destinations button never became hittable")
        clearButton.tap()
        let confirmClear = app.sheets.buttons["Clear"]
        XCTAssertTrue(waitForExistence(confirmClear, timeout: 5), "confirmation dialog for Clear Recent Destinations never appeared")
        confirmClear.tap()
        XCTAssertTrue(waitForNonexistence(confirmClear, timeout: 5), "confirmation dialog is still up after confirming Clear")

        // Done dismisses the sheet -- always in the nav bar, no scrolling needed.
        app.buttons["Done"].tap()
        XCTAssertTrue(waitForNonexistence(el("departure-soc-slider"), timeout: 5), "settings sheet is still up after tapping Done")
        XCTAssertTrue(waitForExistence(el("search-pill"), timeout: 5), "map/search UI isn't back after dismissing settings")
    }
}
