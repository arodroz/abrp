// Destination search (wayfinder #40): wraps MKLocalSearchCompleter for live suggestions and
// the recents list, ported from prototype/planner-ui's VariantDGoogle.swift.
import CoreLocation
import Foundation
import MapKit

/// Converts a region's RegionBounds.Box (wayfinder #47) into an MKCoordinateRegion for
/// MKLocalSearchCompleter's `region` bias.
private func mkRegion(for region: String) -> MKCoordinateRegion {
    let box = RegionBounds.box(for: region)
    return MKCoordinateRegion(
        center: CLLocationCoordinate2D(
            latitude: (box.latRange.lowerBound + box.latRange.upperBound) / 2,
            longitude: (box.lonRange.lowerBound + box.lonRange.upperBound) / 2),
        span: MKCoordinateSpan(
            latitudeDelta: box.latRange.upperBound - box.latRange.lowerBound,
            longitudeDelta: box.lonRange.upperBound - box.lonRange.lowerBound)
    )
}

/// Wraps MKLocalSearchCompleter for live suggestions as the user types. NSObject/
/// MKLocalSearchCompleterDelegate conformance needs `ObservableObject`, not `@Observable`.
///
/// M-07 (docs/codebase-audit-2026-08-29.md): the completer used to be pinned to the corridor
/// pack's bounds regardless of the active region. It's now constructed with, and kept in sync
/// with, PlanStore's `activeRegion` -- see RouteEditorView's init and
/// onChange(of: store.activeRegion).
///
/// Swift 6 strict concurrency (M-05 -- docs/codebase-audit-2026-08-29.md): MapKit doesn't
/// publish concurrency annotations for this delegate, so the compiler can't verify its callback
/// thread the way it can CLLocationManager's/MLNMapView's. `@preconcurrency` conformance is
/// still sound in practice, not just convenient: `completer.queryFragment` is set only from
/// this @MainActor class in response to UI typing, and Apple's own DTS guidance for this exact
/// Swift 6 migration (developer.apple.com/forums/thread/761518) has developers call
/// `MainActor.assumeIsolated` from inside this callback -- which traps if the assumption is
/// wrong -- confirming delivery is genuinely on the main thread.
@MainActor
final class DestinationSearchModel: NSObject, ObservableObject, @preconcurrency MKLocalSearchCompleterDelegate {
    @Published var query: String = "" {
        didSet { completer.queryFragment = query }
    }
    @Published var results: [MKLocalSearchCompletion] = []
    /// The region the completer is currently biased to -- exposed so the wiring can be checked
    /// directly (e.g. in the debugger or a log) since MKLocalSearchCompleter is network-backed
    /// and has no autotest coverage.
    private(set) var biasedRegion: String

    private let completer: MKLocalSearchCompleter

    init(region: String) {
        biasedRegion = region
        completer = MKLocalSearchCompleter()
        super.init()
        completer.resultTypes = [.address, .pointOfInterest]
        completer.region = mkRegion(for: region)
        completer.delegate = self
    }

    /// Called from RouteEditorView.onChange(of: store.activeRegion). A no-op guard avoids
    /// resetting the completer's in-flight region on every unrelated view update.
    func updateRegion(_ region: String) {
        guard region != biasedRegion else { return }
        biasedRegion = region
        completer.region = mkRegion(for: region)
    }

    func completerDidUpdateResults(_ completer: MKLocalSearchCompleter) {
        results = completer.results
    }

    func completer(_ completer: MKLocalSearchCompleter, didFailWithError error: Error) {
        results = []
    }
}

/// One entry in the search pill's recents list (@AppStorage-backed JSON, newest first, max 5,
/// deduped by name).
struct RecentDestination: Codable, Identifiable, Equatable {
    let name: String
    let lat: Double
    let lon: Double
    var id: String { "\(name)|\(lat)|\(lon)" }
    var coordinate: CLLocationCoordinate2D { CLLocationCoordinate2D(latitude: lat, longitude: lon) }

    /// Shared by the Settings "Clear Recent Destinations" button and the autotest (issue #56 /
    /// SEC-010): removes the key outright rather than writing "[]" itself, so
    /// @AppStorage("recentDestinations") (RouteEditorView) falls back to its own "[]" default.
    static func clearAll() {
        UserDefaults.standard.removeObject(forKey: "recentDestinations")
    }
}
