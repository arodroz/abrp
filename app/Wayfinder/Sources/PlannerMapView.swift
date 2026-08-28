// Wraps PlanStore's single MLNMapView instance for SwiftUI. Ported verbatim from
// prototype/planner-ui's PlannerMapView.swift, adapted from ObservedObject to PlanStore's
// @Observable.
import MapLibre
import SwiftUI

struct PlannerMapView: UIViewRepresentable {
    let store: PlanStore

    func makeUIView(context: Context) -> MLNMapView { store.mapView }
    func updateUIView(_ uiView: MLNMapView, context: Context) {}
}
