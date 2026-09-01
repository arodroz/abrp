// The FFI adapter (wayfinder #79) wrapping PlannerKit's `TelemetrySession` behind the
// `TelemetryDialogue` protocol (TelemetryDialogue.swift), so TelemetryPump/TelemetryLinkStore can
// drive the real Rust engine exactly like ScriptedDialogue does in obd-smoke -- see that
// protocol's header for why this seam exists. One `TelemetrySession` completes ONE full poll
// sweep over the profile's command list (per-command freq scheduling is the caller's job, ignored
// in v1 -- TelemetryLinkStore's poll loop sweeps everything each cycle).
import Foundation
import PlannerKit

/// `TelemetryDialogue` plus a drain hook, so `TelemetryLinkStore`'s poll loop can pull decoded
/// readings without knowing the concrete FFI type.
protocol TelemetryReadingDialogue: TelemetryDialogue {
    func drainReadings() -> [FfiTelemetryReading]
}

final class TelemetrySessionDialogue: TelemetryReadingDialogue {
    private let session: TelemetrySession

    init(session: TelemetrySession) {
        self.session = session
    }

    func takeOutgoing() -> Data? { session.outgoing() }
    func feed(_ data: Data) { session.feed(bytes: data) }
    func onTimeout() { session.onTimeout() }
    var isFinished: Bool { session.isFinished() }
    func drainReadings() -> [FfiTelemetryReading] { session.drainReadings() }
}

/// The driver's actual car (wayfinder #79 v1 default, ADR-noted future work): hardcoded pending a
/// profile/variant picker UI -- `loadTelemetryProfile` (PlannerKit) is the general-purpose entry
/// point that future UI will validate a chosen profile through; v1 skips it and hands the bundled
/// JSON straight to `TelemetrySession`.
enum Ioniq5Profile {
    static let variantId = "77_4_kwh"

    /// `nil` (after an assertionFailure) if the bundle resource is missing -- callers leave
    /// telemetry off rather than crash.
    static func loadJson() -> String? {
        guard let url = Bundle.main.url(forResource: "hyundai-ioniq5.tprof", withExtension: "json", subdirectory: "profiles") else {
            assertionFailure("hyundai-ioniq5.tprof.json missing from the app bundle")
            return nil
        }
        return try? String(contentsOf: url, encoding: .utf8)
    }
}
