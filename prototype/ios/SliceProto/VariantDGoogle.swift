// VariantD "Google" (Google Maps directions idiom): full-screen map, a floating search pill
// (destination search via MKLocalSearchCompleter/MKLocalSearch -- decided in #9), origin set
// by long-pressing the map, and a compact bottom result card (not a drag sheet) that expands
// upward to reveal the itinerary + SoC chart. Zero settings surface.

import CoreLocation
import MapKit
import MapLibre
import SwiftUI

/// Roughly lat 49.4-53.6, lon 2.5-7.3 (Benelux corridor) -- biases MKLocalSearchCompleter's
/// suggestions, per the pack's actual coverage.
private let corridorRegion = MKCoordinateRegion(
    center: CLLocationCoordinate2D(latitude: 51.5, longitude: 4.9),
    span: MKCoordinateSpan(latitudeDelta: 4.2, longitudeDelta: 4.8)
)

/// Wraps MKLocalSearchCompleter for live suggestions as the user types.
private final class DestinationSearchModel: NSObject, ObservableObject, MKLocalSearchCompleterDelegate {
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

/// Wraps the shared MLNMapView with a long-press recognizer that sets the plan's origin
/// (crude prototype affordance -- no drag-to-adjust).
private struct GoogleMapView: UIViewRepresentable {
    @ObservedObject var store: PlanStore
    var onLongPress: (CLLocationCoordinate2D) -> Void

    func makeUIView(context: Context) -> MLNMapView {
        let mapView = store.mapView
        let recognizer = UILongPressGestureRecognizer(
            target: context.coordinator, action: #selector(Coordinator.handleLongPress(_:)))
        recognizer.minimumPressDuration = 0.4
        mapView.addGestureRecognizer(recognizer)
        context.coordinator.recognizer = recognizer
        return mapView
    }

    func updateUIView(_ uiView: MLNMapView, context: Context) {}

    static func dismantleUIView(_ uiView: MLNMapView, coordinator: Coordinator) {
        if let recognizer = coordinator.recognizer {
            uiView.removeGestureRecognizer(recognizer)
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(mapView: store.mapView, onLongPress: onLongPress)
    }

    final class Coordinator: NSObject {
        let mapView: MLNMapView
        let onLongPress: (CLLocationCoordinate2D) -> Void
        var recognizer: UILongPressGestureRecognizer?

        init(mapView: MLNMapView, onLongPress: @escaping (CLLocationCoordinate2D) -> Void) {
            self.mapView = mapView
            self.onLongPress = onLongPress
        }

        @objc func handleLongPress(_ gesture: UILongPressGestureRecognizer) {
            guard gesture.state == .began else { return }
            let point = gesture.location(in: mapView)
            let coordinate = mapView.convert(point, toCoordinateFrom: mapView)
            onLongPress(coordinate)
        }
    }
}

struct VariantDGoogle: View {
    @ObservedObject var store: PlanStore
    @StateObject private var searchModel = DestinationSearchModel()
    @FocusState private var searchFocused: Bool

    @State private var searchExpanded = false
    @State private var destinationTitle: String?
    @State private var cardExpanded = false
    @State private var toast: String?

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .top) {
                GoogleMapView(store: store, onLongPress: { store.setOrigin($0) })
                    .ignoresSafeArea()

                VStack(spacing: 8) {
                    searchPill
                    if searchExpanded && !searchModel.results.isEmpty {
                        suggestionsList
                    }
                    Spacer()
                }
                .padding(.horizontal)
                .padding(.top, 8)

                if let toast {
                    Text(toast)
                        .font(.footnote).bold()
                        .foregroundColor(.white)
                        .padding(.horizontal, 14)
                        .padding(.vertical, 8)
                        .background(Color.black.opacity(0.85), in: Capsule())
                        .padding(.top, 68)
                        .frame(maxWidth: .infinity)
                        .transition(.move(edge: .top).combined(with: .opacity))
                        .allowsHitTesting(false)
                }

                VStack {
                    Spacer()
                    if store.plan != nil {
                        ResultCard(store: store, expanded: $cardExpanded, onTapStop: { store.panMap(to: $0.coordinate) })
                            .frame(height: cardExpanded ? geo.size.height * 0.7 : nil)
                            .padding(.horizontal, 12)
                            .padding(.bottom, 78) // clear the floating variant switcher pill
                    }
                }
            }
        }
        .onChange(of: store.planVersion) { _, _ in store.fitToRoute() }
        .onChange(of: store.planError) { _, newValue in
            guard newValue != nil else { return }
            showToast("Outside pack region")
        }
    }

    private var searchPill: some View {
        HStack(spacing: 10) {
            Image(systemName: "magnifyingglass")
                .foregroundColor(.secondary)
            if searchExpanded {
                TextField("Search destination", text: $searchModel.query)
                    .focused($searchFocused)
                    .submitLabel(.search)
            } else {
                Text(destinationTitle ?? "Search destination")
                    .foregroundColor(destinationTitle == nil ? .secondary : .primary)
                    .lineLimit(1)
            }
            Spacer()
            if store.isPlanning {
                ProgressView().scaleEffect(0.8)
            }
            if searchExpanded {
                Button {
                    searchFocused = false
                    withAnimation { searchExpanded = false }
                    searchModel.query = ""
                } label: {
                    Image(systemName: "xmark.circle.fill").foregroundColor(.secondary)
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 12)
        .background(.regularMaterial, in: Capsule())
        .shadow(color: .black.opacity(0.15), radius: 6, y: 2)
        .contentShape(Capsule())
        .onTapGesture {
            guard !searchExpanded else { return }
            withAnimation { searchExpanded = true }
            searchFocused = true
        }
    }

    private var suggestionsList: some View {
        VStack(alignment: .leading, spacing: 0) {
            ForEach(Array(searchModel.results.enumerated()), id: \.offset) { idx, completion in
                Button {
                    select(completion)
                } label: {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(completion.title).font(.subheadline).foregroundColor(.primary)
                        if !completion.subtitle.isEmpty {
                            Text(completion.subtitle).font(.caption).foregroundColor(.secondary)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 14)
                    .padding(.vertical, 8)
                }
                .buttonStyle(.plain)
                if idx != searchModel.results.count - 1 {
                    Divider().padding(.leading, 14)
                }
            }
        }
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
        .shadow(color: .black.opacity(0.15), radius: 6, y: 2)
    }

    private func select(_ completion: MKLocalSearchCompletion) {
        let search = MKLocalSearch(request: MKLocalSearch.Request(completion: completion))
        search.start { response, _ in
            DispatchQueue.main.async {
                searchFocused = false
                withAnimation { searchExpanded = false }
                searchModel.query = ""
                guard let coordinate = response?.mapItems.first?.placemark.coordinate else {
                    showToast("Outside pack region")
                    return
                }
                destinationTitle = completion.title
                store.planTo(destination: coordinate)
            }
        }
    }

    private func showToast(_ message: String) {
        withAnimation { toast = message }
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) {
            withAnimation { toast = nil }
        }
    }
}

/// Google-directions-style bottom card: collapsed shows total time + distance/arrival + a
/// horizontal row of Charging Stop chips; expanded (chevron or swipe) reveals the itinerary
/// and SoC chart.
private struct ResultCard: View {
    @ObservedObject var store: PlanStore
    @Binding var expanded: Bool
    var onTapStop: (ChargingStopVM) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Capsule()
                .fill(Color.secondary.opacity(0.4))
                .frame(width: 36, height: 5)
                .frame(maxWidth: .infinity)
                .padding(.top, 8)

            summary

            if let plan = store.plan, !plan.stops.isEmpty {
                stopChipsRow(plan)
            }

            if expanded {
                Divider().padding(.top, 4)
                ScrollView {
                    VStack(alignment: .leading, spacing: 16) {
                        itinerary
                        SoCChartView(store: store)
                            .padding(.horizontal)
                    }
                    .padding(.vertical, 12)
                }
            } else {
                Spacer().frame(height: 12)
            }
        }
        .background(.regularMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 20, style: .continuous))
        .shadow(color: .black.opacity(0.2), radius: 10, y: 4)
        .gesture(
            DragGesture()
                .onEnded { value in
                    if value.translation.height > 40 {
                        withAnimation(.spring()) { expanded = false }
                    } else if value.translation.height < -40 {
                        withAnimation(.spring()) { expanded = true }
                    }
                }
        )
    }

    private var summary: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: 2) {
                if let plan = store.plan {
                    Text(formatDuration(plan.totalTimeS)).font(.title2).bold()
                    Text("\(formatKm(plan.totalDistM)) \u{00B7} arrive \(formatSocPct(plan.arrivalSoc))")
                        .font(.subheadline)
                        .foregroundColor(.secondary)
                } else {
                    Text(store.isPlanning ? "Planning\u{2026}" : "No plan yet").font(.title2).bold()
                }
            }
            Spacer()
            Button {
                withAnimation(.spring()) { expanded.toggle() }
            } label: {
                Image(systemName: expanded ? "chevron.down" : "chevron.up")
                    .font(.headline)
                    .foregroundColor(.secondary)
                    .padding(8)
            }
        }
        .padding(.horizontal, 16)
        .padding(.top, 4)
    }

    private func stopChipsRow(_ plan: Plan) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(Array(plan.stops.enumerated()), id: \.element.id) { _, stop in
                    Button {
                        onTapStop(stop)
                    } label: {
                        HStack(spacing: 6) {
                            Image(systemName: "bolt.fill").foregroundColor(.orange)
                            VStack(alignment: .leading, spacing: 0) {
                                Text(stop.name)
                                    .font(.caption).bold()
                                    .lineLimit(1)
                                    .truncationMode(.tail)
                                Text(
                                    "+\(Int(((stop.departSoc - stop.arrivalSoc) * 100).rounded()))% \u{00B7} "
                                        + formatDuration(stop.chargeS)
                                )
                                .font(.caption2)
                                .foregroundColor(.secondary)
                            }
                            .frame(maxWidth: 110, alignment: .leading)
                        }
                        .padding(.horizontal, 10)
                        .padding(.vertical, 6)
                        .background(Color.orange.opacity(0.12), in: Capsule())
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 8)
        }
    }

    private var itinerary: some View {
        VStack(alignment: .leading, spacing: 0) {
            row(title: "Origin", subtitle: "Depart at \(formatSocPct(store.departSoc))")
            if let plan = store.plan {
                ForEach(Array(plan.stops.enumerated()), id: \.element.id) { _, stop in
                    stopRow(stop)
                }
                row(title: "Destination", subtitle: "Arrive at \(formatSocPct(plan.arrivalSoc))")
            }
        }
    }

    private func row(title: String, subtitle: String) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(title).font(.headline)
            Text(subtitle).font(.caption).foregroundColor(.secondary)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
    }

    private func stopRow(_ stop: ChargingStopVM) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            Text(stop.name).font(.headline)
            Text(
                "\(Int(stop.powerKw)) kW \u{00B7} \(formatSocPct(stop.arrivalSoc))\u{2192}"
                    + "\(formatSocPct(stop.departSoc)) \u{00B7} " + formatDuration(stop.chargeS)
            )
            .font(.caption)
            .foregroundColor(.secondary)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
    }
}
