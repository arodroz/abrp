// Wayfinder app entry (wayfinder #39). The map surface is RootView (#42); the route editor,
// arrival card, and settings are separate later tickets.
import SwiftUI

@main
struct WayfinderApp: App {
    private let store = PlanStore()

    init() {
        Autotest.runIfRequested(store: store)
    }

    var body: some Scene {
        WindowGroup {
            RootView(store: store)
        }
    }
}
