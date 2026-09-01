// Staleness presentation for the live SoC chip (wayfinder #79): a pure enum, no view/store
// dependencies, so live-soc-smoke can assert its cases directly and the drive HUD/CarPlay strip
// can never derive staleness differently from each other -- both read this, never
// TelemetryLinkStore's raw `lastReadingAt` themselves.
import Foundation

enum LiveSocPresentation: Equatable {
    case hidden
    case fresh(Double)
    case stale(Double)

    static func compute(soc: Double?, age: TimeInterval?, staleAfterS: TimeInterval = 10) -> LiveSocPresentation {
        guard let soc, let age else { return .hidden }
        return age >= staleAfterS ? .stale(soc) : .fresh(soc)
    }
}
