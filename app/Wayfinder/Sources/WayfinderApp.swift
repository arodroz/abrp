// Wayfinder app entry (wayfinder #39). The UI surfaces (map, route editor,
// arrival card, settings) are separate later tickets -- RootView is a
// placeholder for now.
import SwiftUI

@main
struct WayfinderApp: App {
    init() {
        Autotest.runIfRequested()
    }

    var body: some Scene {
        WindowGroup {
            RootView()
        }
    }
}
