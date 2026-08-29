// Destination search (wayfinder #40): wraps MKLocalSearchCompleter for live suggestions and
// the recents list, ported from prototype/planner-ui's VariantDGoogle.swift.
import CoreLocation
import Foundation
import MapKit

/// Biases MKLocalSearchCompleter's suggestions per the corridor pack's actual coverage --
/// derived from RegionBounds.swift's shared table (wayfinder #47) rather than a second
/// hardcoded copy of the same box.
let corridorRegion: MKCoordinateRegion = {
    let box = RegionBounds.box(for: "corridor")
    return MKCoordinateRegion(
        center: CLLocationCoordinate2D(
            latitude: (box.latRange.lowerBound + box.latRange.upperBound) / 2,
            longitude: (box.lonRange.lowerBound + box.lonRange.upperBound) / 2),
        span: MKCoordinateSpan(
            latitudeDelta: box.latRange.upperBound - box.latRange.lowerBound,
            longitudeDelta: box.lonRange.upperBound - box.lonRange.lowerBound)
    )
}()

/// Wraps MKLocalSearchCompleter for live suggestions as the user types. NSObject/
/// MKLocalSearchCompleterDelegate conformance needs `ObservableObject`, not `@Observable`.
@MainActor
final class DestinationSearchModel: NSObject, ObservableObject, MKLocalSearchCompleterDelegate {
    @Published var query: String = "" {
        didSet { completer.queryFragment = query }
    }
    @Published var results: [MKLocalSearchCompletion] = []

    private let completer: MKLocalSearchCompleter

    override init() {
        completer = MKLocalSearchCompleter()
        super.init()
        completer.resultTypes = [.address, .pointOfInterest]
        completer.region = corridorRegion
        completer.delegate = self
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
}
