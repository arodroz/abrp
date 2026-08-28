// The map surface (wayfinder #42): the map IS the root surface, full-screen, with a small
// status overlay while the pack + planner are loading. Search UI, the result card/bottom
// sheet, and settings are separate later tickets (#40, #43, #44).
import SwiftUI

struct RootView: View {
    let store: PlanStore
    @Environment(\.colorScheme) private var colorScheme

    var body: some View {
        ZStack(alignment: .top) {
            PlannerMapView(store: store)
                .ignoresSafeArea()
            if !isReady {
                statusOverlay
            }
        }
        .onAppear {
            store.setAppearance(dark: colorScheme == .dark)
            store.load()
        }
        .onChange(of: colorScheme) { _, newValue in
            store.setAppearance(dark: newValue == .dark)
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
}
