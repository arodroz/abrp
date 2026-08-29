// Wayfinder app entry (wayfinder #39). The map surface is RootView (#42); the route editor,
// arrival card, and settings are separate later tickets. The pack installer (#47) is owned
// here too, alongside PlanStore, and checked for updates once on launch (fire-and-forget --
// offline must never block startup).
import Foundation
import SwiftUI

@main
struct WayfinderApp: App {
    /// Process launch time (wayfinder #45): a static let initializes lazily on first access,
    /// so touching it first thing in init(), before anything else runs, pins t0 as close to
    /// process entry as Swift allows -- `--autotest perf`'s cold_from_launch_ms baseline.
    /// `nonisolated` because `App` is `@MainActor`-isolated but perf's measurement runs off
    /// a detached Task, same as the rest of Autotest's PlannerClient calls.
    nonisolated static let launchUptime = ProcessInfo.processInfo.systemUptime

    private let store = PlanStore()
    private let packInstaller = PackInstaller()

    init() {
        _ = Self.launchUptime
        Autotest.runIfRequested(store: store, installer: packInstaller)
        let installer = packInstaller
        Task { await installer.checkForUpdates() }
    }

    var body: some Scene {
        WindowGroup {
            RootView(store: store, installer: packInstaller)
        }
    }
}
