// Destination search (wayfinder #40): wraps MKLocalSearchCompleter for live suggestions and
// the recents list, ported from prototype/planner-ui's VariantDGoogle.swift.
import CoreLocation
import Foundation
import MapKit

/// Roughly lat 49.4-53.6, lon 2.5-7.3 (Benelux corridor) -- biases MKLocalSearchCompleter's
/// suggestions per the pack's actual coverage.
let corridorRegion = MKCoordinateRegion(
    center: CLLocationCoordinate2D(latitude: 51.5, longitude: 4.9),
    span: MKCoordinateSpan(latitudeDelta: 4.2, longitudeDelta: 4.8)
)

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
