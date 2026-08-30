// Wayfinder app entry (wayfinder #39). The map surface is RootView (#42); the route editor,
// arrival card, and settings are separate later tickets. The pack installer (#47) is owned
// here too, alongside PlanStore, and checked for updates once on launch (fire-and-forget --
// offline must never block startup). TripLogStore (wayfinder #51) is owned here too, separate
// from PlanStore -- see TripLogStore.swift's header. `AppDelegate` exists solely to catch the
// background URLSession completion handler (codebase audit H-01/H-03, wayfinder #47 --
// docs/codebase-audit-2026-08-29.md): PackInstaller's background downloads need it stashed
// somewhere `urlSessionDidFinishEvents(forBackgroundURLSession:)` can reach. DriveStore
// (Drive Mode core, wayfinder #59) is owned here too, wrapping `store` (PlanStore) rather than
// duplicating its map/route state.
import Foundation
import SwiftUI
import UIKit

/// Stores the completion handler iOS hands `application(_:handleEventsForBackgroundURLSession:
/// completionHandler:)` when it relaunches (or wakes a suspended) app to deliver background
/// URLSession events. `PackInstaller.urlSessionDidFinishEvents(forBackgroundURLSession:)` calls
/// it once its session delegate has drained the events, telling the OS this app is done
/// processing and can be suspended/snapshotted again.
final class AppDelegate: NSObject, UIApplicationDelegate {
    var backgroundSessionCompletionHandler: (() -> Void)?

    func application(
        _ application: UIApplication,
        handleEventsForBackgroundURLSession identifier: String,
        completionHandler: @escaping () -> Void
    ) {
        backgroundSessionCompletionHandler = completionHandler
    }
}

@main
struct WayfinderApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    /// Process launch time (wayfinder #45): a static let initializes lazily on first access,
    /// so touching it first thing in init(), before anything else runs, pins t0 as close to
    /// process entry as Swift allows -- `--autotest perf`'s cold_from_launch_ms baseline.
    /// `nonisolated` because `App` is `@MainActor`-isolated but perf's measurement runs off
    /// a detached Task, same as the rest of Autotest's PlannerClient calls.
    nonisolated static let launchUptime = ProcessInfo.processInfo.systemUptime

    private let store = PlanStore()
    private let packInstaller = PackInstaller()
    private let tripStore = TripLogStore()
    private let driveStore: DriveStore

    init() {
        _ = Self.launchUptime
        driveStore = DriveStore(planStore: store)
        Autotest.runIfRequested(store: store, installer: packInstaller, tripStore: tripStore, driveStore: driveStore)
        let installer = packInstaller
        Task { await installer.checkForUpdates() }
    }

    var body: some Scene {
        WindowGroup {
            RootView(store: store, installer: packInstaller, tripStore: tripStore, driveStore: driveStore)
        }
    }
}
