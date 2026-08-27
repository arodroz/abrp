// Reusable wrapper so each variant can embed the single shared MLNMapView either
// full-screen or inside a panel/page.

import MapLibre
import SwiftUI

struct PlannerMapView: UIViewRepresentable {
    @ObservedObject var store: PlanStore

    func makeUIView(context: Context) -> MLNMapView { store.mapView }
    func updateUIView(_ uiView: MLNMapView, context: Context) {}
}
