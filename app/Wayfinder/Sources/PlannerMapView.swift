// Wraps PlanStore's single MLNMapView instance for SwiftUI. Ported from
// prototype/planner-ui's PlannerMapView.swift/GoogleMapView, adapted from ObservedObject to
// PlanStore's @Observable. Adds the route editor's map gestures (wayfinder #40): long-press
// to set the origin, tap for the charger-callout hit test (done by the caller, which owns
// the callout state -- this view only forwards the raw tap point).
import MapLibre
import SwiftUI

struct PlannerMapView: UIViewRepresentable {
    let store: PlanStore
    var onLongPress: (CLLocationCoordinate2D) -> Void
    var onTap: (CGPoint) -> Void

    func makeUIView(context: Context) -> MLNMapView {
        let mapView = store.mapView

        let longPress = UILongPressGestureRecognizer(
            target: context.coordinator, action: #selector(Coordinator.handleLongPress(_:)))
        longPress.minimumPressDuration = 0.4
        mapView.addGestureRecognizer(longPress)
        context.coordinator.longPress = longPress

        let tap = UITapGestureRecognizer(
            target: context.coordinator, action: #selector(Coordinator.handleTap(_:)))
        tap.delegate = context.coordinator
        mapView.addGestureRecognizer(tap)
        context.coordinator.tap = tap

        return mapView
    }

    func updateUIView(_ uiView: MLNMapView, context: Context) {}

    static func dismantleUIView(_ uiView: MLNMapView, coordinator: Coordinator) {
        if let longPress = coordinator.longPress { uiView.removeGestureRecognizer(longPress) }
        if let tap = coordinator.tap { uiView.removeGestureRecognizer(tap) }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(mapView: store.mapView, onLongPress: onLongPress, onTap: onTap)
    }

    final class Coordinator: NSObject, UIGestureRecognizerDelegate {
        let mapView: MLNMapView
        let onLongPress: (CLLocationCoordinate2D) -> Void
        let onTap: (CGPoint) -> Void
        var longPress: UILongPressGestureRecognizer?
        var tap: UITapGestureRecognizer?

        init(
            mapView: MLNMapView, onLongPress: @escaping (CLLocationCoordinate2D) -> Void,
            onTap: @escaping (CGPoint) -> Void
        ) {
            self.mapView = mapView
            self.onLongPress = onLongPress
            self.onTap = onTap
        }

        @objc func handleLongPress(_ gesture: UILongPressGestureRecognizer) {
            guard gesture.state == .began else { return }
            let point = gesture.location(in: mapView)
            onLongPress(mapView.convert(point, toCoordinateFrom: mapView))
        }

        @objc func handleTap(_ gesture: UITapGestureRecognizer) {
            guard gesture.state == .ended else { return }
            onTap(gesture.location(in: mapView))
        }

        // Let our tap recognizer coexist with MapLibre's own built-in gesture recognizers
        // (annotation selection, double-tap zoom, etc).
        func gestureRecognizer(
            _ gestureRecognizer: UIGestureRecognizer,
            shouldRecognizeSimultaneouslyWith otherGestureRecognizer: UIGestureRecognizer
        ) -> Bool {
            true
        }
    }
}
