// The map surface (wayfinder #42) plus the search and route editor overlay (wayfinder #40),
// the Plan result card (wayfinder #43), and the settings sheet (wayfinder #44): the map is the
// root surface, full-screen, with the route editor card and search pill on top, a locate-me
// button, a charger tap callout, and a toast for search/plan errors; the result card sits at
// the bottom once a plan exists, below the locate-me button and charger callout. A gear button
// mirrors locate-me on the opposite (bottom-left) corner, presenting the settings sheet over
// the still-visible map. Appearance combines the system color scheme (reported here, on every
// change) with PlanStore's persisted override via `updateAppearance(systemDark:)`.
import SwiftUI

struct RootView: View {
    let store: PlanStore
    @Environment(\.colorScheme) private var colorScheme

    @State private var toast: String?
    @State private var chargerCallout: ChargerCalloutInfo?

    var body: some View {
        ZStack(alignment: .top) {
            PlannerMapView(
                store: store,
                onLongPress: { store.setOrigin($0) },
                onTap: { handleMapTap(at: $0) }
            )
            .ignoresSafeArea()

            if !isReady {
                statusOverlay
            } else {
                RouteEditorView(store: store, onToast: showToast)
                    .padding(.horizontal)
                    .padding(.top, 8)
            }

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
                HStack {
                    settingsButton
                    Spacer()
                    locateMeButton
                }
                .padding(.horizontal, 16)
                .padding(.bottom, 8)
                if let chargerCallout {
                    ChargerCalloutView(info: chargerCallout, onDismiss: { self.chargerCallout = nil })
                        .padding(.horizontal, 12)
                        .padding(.bottom, 8)
                }
                if store.plan != nil {
                    ResultCard(store: store)
                }
            }
        }
        .onAppear {
            store.updateAppearance(systemDark: colorScheme == .dark)
            store.load()
            store.requestLocationPermission()
        }
        .onChange(of: colorScheme) { _, newValue in
            store.updateAppearance(systemDark: newValue == .dark)
        }
        .onChange(of: store.planVersion) { _, _ in
            if let plan = store.plan {
                RouteLayer.fitToRoute(mapView: store.mapView, plan: plan)
            }
        }
        .onChange(of: store.planErrorVersion) { _, _ in
            if let message = store.planErrorMessage { showToast(message) }
        }
        .sheet(isPresented: Binding(get: { store.showingSettings }, set: { store.showingSettings = $0 })) {
            SettingsForm(store: store)
                .presentationDetents([.medium, .large])
        }
    }

    private var isReady: Bool {
        store.plannerStatus == .ready
    }

    private var statusOverlay: some View {
        VStack(spacing: 8) {
            Text(packStatusText)
            Text(plannerStatusText)
        }
        .font(.footnote)
        .padding(10)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 10))
        .padding(.top, 48)
    }

    private var packStatusText: String {
        switch store.packStatus {
        case .missing: return "Pack: missing"
        case .loaded(let region): return "Pack: \(region) found"
        }
    }

    private var plannerStatusText: String {
        switch store.plannerStatus {
        case .idle: return "Planner: idle"
        case .loading: return "Planner: loading…"
        case .ready: return "Planner: ready"
        case .failed(let message): return "Planner: failed (\(message))"
        }
    }

    private var locateMeButton: some View {
        Button {
            store.centerOnUser()
        } label: {
            Image(systemName: "location.fill")
                .font(.headline)
                .foregroundColor(.blue)
                .frame(width: 44, height: 44)
                .background(.regularMaterial, in: Circle())
                .shadow(color: .black.opacity(0.2), radius: 4, y: 2)
        }
    }

    private var settingsButton: some View {
        Button {
            store.showingSettings = true
        } label: {
            Image(systemName: "gearshape.fill")
                .font(.headline)
                .foregroundColor(.blue)
                .frame(width: 44, height: 44)
                .background(.regularMaterial, in: Circle())
                .shadow(color: .black.opacity(0.2), radius: 4, y: 2)
        }
    }

    // MARK: Charger tap callout

    /// Queries the (unclustered) chargers point layer for a hit within a small tolerance
    /// rect around the tap; a miss dismisses whatever callout is showing.
    private func handleMapTap(at point: CGPoint) {
        let mapView = store.mapView
        let tolerance: CGFloat = 22
        let rect = CGRect(x: point.x - tolerance / 2, y: point.y - tolerance / 2, width: tolerance, height: tolerance)

        let hits = mapView.visibleFeatures(in: rect, styleLayerIdentifiers: [ChargersLayer.pointsLayerId])
        guard let charger = hits.first else {
            chargerCallout = nil
            return
        }
        let attrs = charger.attributes
        let name = attrs["name"] as? String ?? "Charger"
        let powerKw = (attrs["power_kw"] as? NSNumber)?.doubleValue ?? 0
        chargerCallout = ChargerCalloutInfo(name: name, powerKw: powerKw, operatorName: attrs["operator"] as? String)
    }

    // MARK: Toast

    private func showToast(_ message: String) {
        withAnimation { toast = message }
        DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) {
            withAnimation { toast = nil }
        }
    }
}
