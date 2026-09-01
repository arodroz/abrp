// Wayfinder app entry (wayfinder #39). The map surface is RootView (#42); the route editor,
// arrival card, and settings are separate later tickets. The pack installer (#47) is owned
// here too, alongside PlanStore, and checked for updates once on launch (fire-and-forget --
// offline must never block startup). TripLogStore (wayfinder #51) is owned here too, separate
// from PlanStore -- see TripLogStore.swift's header. `AppDelegate` exists solely to catch the
// background URLSession completion handler (codebase audit H-01/H-03, wayfinder #47 --
// docs/codebase-audit-2026-08-29.md): PackInstaller's background downloads need it stashed
// somewhere `urlSessionDidFinishEvents(forBackgroundURLSession:)` can reach. DriveStore
// (Drive Mode core, wayfinder #59) is owned here too, wrapping `store` (PlanStore) rather than
// duplicating its map/route state. This is also where CarPlaySceneDelegate (wayfinder #70) gets
// handed `store`/`driveStore` -- UIKit instantiates that scene delegate by class name, so it has
// no other way to reach the stores this app already owns. TelemetryLinkStore (wayfinder #78/#79)
// is constructed here too, wrapping the first real `CxBleLink` -- CoreBluetooth degrades that to
// `.backoff(.bluetoothUnavailable)` on the simulator (see CxBleLink.swift), so constructing it
// unconditionally is safe there too.
import Foundation
import PlannerKit
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
    private let telemetryStore: TelemetryLinkStore
    private let driveStore: DriveStore

    init() {
        _ = Self.launchUptime
        let telemetryStore = TelemetryLinkStore(link: CxBleLink())
        // wayfinder #79: one fresh TelemetrySession (one full poll sweep) per poll cycle -- see
        // TelemetryLinkStore's own header for why. 77.4 kWh Long Range is the driver's actual car
        // (wayfinder #55); a profile/variant picker is future work (`loadTelemetryProfile` is
        // that future UI's validation entry point, unused in this v1 hardcoded path).
        telemetryStore.makeDialogue = {
            guard let json = Ioniq5Profile.loadJson(),
                  let session = try? TelemetrySession(profileJson: json, variantId: Ioniq5Profile.variantId)
            else { return nil }
            return TelemetrySessionDialogue(session: session)
        }
        self.telemetryStore = telemetryStore
        // wayfinder #80: Trip Log auto-capture reads straight off the live readings, only when
        // fresh -- see TelemetryLinkStore.snapshotFreshnessS.
        tripStore.telemetrySnapshot = { [telemetryStore] in
            guard let lastReadingAt = telemetryStore.lastReadingAt,
                  Date().timeIntervalSince(lastReadingAt) <= TelemetryLinkStore.snapshotFreshnessS
            else { return nil }
            let readings = telemetryStore.latestReadings
            return TripTelemetrySnapshot(
                displaySocPct: readings[.displaySoc], bmsSocPct: readings[.bmsSoc],
                cumulativeChargeKwh: readings[.cumulativeChargeEnergy],
                cumulativeDischargeKwh: readings[.cumulativeDischargeEnergy]
            )
        }
        driveStore = DriveStore(planStore: store, tripStore: tripStore, telemetryStore: telemetryStore)
        // wayfinder #70: the CarPlay scene is UIKit-instantiated by class name, so it can't be
        // handed the stores any other way.
        CarPlaySceneDelegate.planStore = store
        CarPlaySceneDelegate.driveStore = driveStore
        CarPlaySceneDelegate.telemetryStore = telemetryStore
        Autotest.runIfRequested(
            store: store, installer: packInstaller, tripStore: tripStore, driveStore: driveStore,
            telemetryStore: telemetryStore
        )
        let installer = packInstaller
        // wayfinder #55: an install of the active region must release the live Planner's
        // mmapped .rpack before the installer's deep verify tries to mmap the staged
        // replacement (and only then -- planning stays available through the download), then
        // re-adopt whatever's on disk once the attempt ends: the new pack on success, the
        // untouched old one on failure. One rule either way.
        packInstaller.onDeepVerifyWillStart = { [weak store] region in store?.unloadForPackUpdate(region: region) }
        packInstaller.onInstallDidEnd = { [weak store] region in store?.packsDidChange(region: region) }
        Task { await installer.checkForUpdates() }
    }

    var body: some Scene {
        WindowGroup {
            RootView(store: store, installer: packInstaller, tripStore: tripStore, driveStore: driveStore, telemetryStore: telemetryStore)
        }
    }
}
