// Placeholder root view (wayfinder #39). The real map/route-editor UI is a
// separate later ticket; this just proves pack sideload + PlannerKit wiring
// are working end to end.
import SwiftUI

struct RootView: View {
    @State private var store = PlanStore()

    var body: some View {
        VStack(spacing: 16) {
            Text("Wayfinder")
                .font(.largeTitle.bold())
            Text(packStatusText)
            Text(plannerStatusText)
        }
        .padding()
        .onAppear { store.load() }
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
