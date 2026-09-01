// Route polyline + Charging Stop markers, ported from prototype/planner-ui's
// `addRouteAndStopsLayers` and `fitToRoute` (PlanStore.swift). Adapted from the prototype's
// `plan_json` dictionary shape to the typed `FfiPlan` (`FfiGeoPoint` polyline, `FfiStop`
// stops) -- still built as GeoJSON dictionaries and loaded via `MLNShape(data:encoding:)`,
// matching the prototype's proven approach, since PlannerKit has no GeoJSON type of its own.
import CoreLocation
import Foundation
import MapLibre
import PlannerKit
import UIKit

// @MainActor: every function here takes an MLNStyle/MLNMapView, which are main-actor-bound;
// isolating the whole namespace makes that explicit under Swift 6 strict concurrency (M-05)
// instead of leaving nonisolated statics that only happen to be called from the main actor.
@MainActor
enum RouteLayer {
    private static let routeSourceId = "route"
    private static let stopsSourceId = "stops"
    private static let connectorSourceId = "route-connector"
    static let routeLineId = "route-line"
    private static let routeLineTopId = "route-line-top"
    static let stopsCirclesId = "stops-circles"
    private static let stopsLabelsId = "stops-labels"
    private static let connectorLineId = "route-connector"

    /// `origin`/`destination`, when given, trim the drawn polyline to start/end at the pin
    /// rather than the snapped routing-graph junction node (RouteGeometry.trimmedDisplayPolyline,
    /// wayfinder #84) and draw a dashed connector stub over any remaining gap -- display only,
    /// `plan.polyline` itself is untouched (DriveStore/RouteSnap/StepTracker need the raw one).
    static func addLayers(
        to style: MLNStyle, plan: FfiPlan, origin: CLLocationCoordinate2D? = nil,
        destination: CLLocationCoordinate2D? = nil
    ) {
        for id in [routeLineId, routeLineTopId, stopsCirclesId, stopsLabelsId, connectorLineId] {
            if let layer = style.layer(withIdentifier: id) { style.removeLayer(layer) }
        }
        for id in [routeSourceId, stopsSourceId, connectorSourceId] {
            if let source = style.source(withIdentifier: id) { style.removeSource(source) }
        }

        guard !plan.polyline.isEmpty else { return }

        let fullPolyline = plan.polyline.map { CLLocationCoordinate2D(latitude: $0.lat, longitude: $0.lon) }
        var displayCoordinates = fullPolyline
        var connectors: [[CLLocationCoordinate2D]] = []
        if let origin, let destination {
            let trimmed = RouteGeometry.trimmedDisplayPolyline(fullPolyline, origin: origin, destination: destination)
            displayCoordinates = trimmed.display
            if let originConnector = trimmed.originConnector { connectors.append(originConnector) }
            if let destinationConnector = trimmed.destinationConnector { connectors.append(destinationConnector) }
        }

        let routeGeojson: [String: Any] = [
            "type": "Feature",
            "geometry": [
                "type": "LineString",
                "coordinates": displayCoordinates.map { [$0.longitude, $0.latitude] },
            ],
        ]
        guard let routeData = try? JSONSerialization.data(withJSONObject: routeGeojson),
              let routeShape = try? MLNShape(data: routeData, encoding: String.Encoding.utf8.rawValue)
        else { return }

        let routeSource = MLNShapeSource(identifier: routeSourceId, shape: routeShape, options: nil)
        style.addSource(routeSource)

        let routeLine = MLNLineStyleLayer(identifier: routeLineId, source: routeSource)
        routeLine.lineWidth = NSExpression(forConstantValue: 6.0)
        routeLine.lineColor = NSExpression(forConstantValue: UIColor.systemBlue)
        routeLine.lineJoin = NSExpression(forConstantValue: "round")
        routeLine.lineCap = NSExpression(forConstantValue: "round")
        style.addLayer(routeLine)

        // Thin brighter line on top, for looks.
        let routeLineTop = MLNLineStyleLayer(identifier: routeLineTopId, source: routeSource)
        routeLineTop.lineWidth = NSExpression(forConstantValue: 2.0)
        routeLineTop.lineColor = NSExpression(forConstantValue: UIColor.cyan)
        routeLineTop.lineJoin = NSExpression(forConstantValue: "round")
        routeLineTop.lineCap = NSExpression(forConstantValue: "round")
        style.addLayer(routeLineTop)

        if !connectors.isEmpty {
            let connectorFeatures: [[String: Any]] = connectors.map { connector in
                [
                    "type": "Feature",
                    "geometry": [
                        "type": "LineString",
                        "coordinates": connector.map { [$0.longitude, $0.latitude] },
                    ],
                ]
            }
            let connectorFC: [String: Any] = ["type": "FeatureCollection", "features": connectorFeatures]
            if let connectorData = try? JSONSerialization.data(withJSONObject: connectorFC),
               let connectorShape = try? MLNShape(data: connectorData, encoding: String.Encoding.utf8.rawValue) {
                let connectorSource = MLNShapeSource(identifier: connectorSourceId, shape: connectorShape, options: nil)
                style.addSource(connectorSource)

                // Google-Maps-style dotted stub from pin to road.
                let connectorLine = MLNLineStyleLayer(identifier: connectorLineId, source: connectorSource)
                connectorLine.lineWidth = NSExpression(forConstantValue: 3.0)
                connectorLine.lineColor = NSExpression(forConstantValue: UIColor.systemBlue)
                connectorLine.lineOpacity = NSExpression(forConstantValue: 0.7)
                connectorLine.lineDashPattern = NSExpression(forConstantValue: [1.5, 1.5])
                connectorLine.lineCap = NSExpression(forConstantValue: "round")
                connectorLine.lineJoin = NSExpression(forConstantValue: "round")
                style.addLayer(connectorLine)
            }
        }

        let stopFeatures: [[String: Any]] = plan.stops.map { stop in
            [
                "type": "Feature",
                "geometry": ["type": "Point", "coordinates": [stop.lon, stop.lat]],
                "properties": [
                    "name": stop.name, "power_kw": stop.powerKw,
                    // Precomputed label -- MapLibre iOS NSExpression does NOT support CONCAT.
                    "label": "\(stop.name) \(Int(stop.powerKw))kW",
                ],
            ]
        }
        let stopsFC: [String: Any] = ["type": "FeatureCollection", "features": stopFeatures]
        guard let stopsData = try? JSONSerialization.data(withJSONObject: stopsFC),
              let stopsShape = try? MLNShape(data: stopsData, encoding: String.Encoding.utf8.rawValue)
        else { return }

        let stopsSource = MLNShapeSource(identifier: stopsSourceId, shape: stopsShape, options: nil)
        style.addSource(stopsSource)

        let stopsCircles = MLNCircleStyleLayer(identifier: stopsCirclesId, source: stopsSource)
        stopsCircles.circleRadius = NSExpression(forConstantValue: 10.0)
        stopsCircles.circleColor = NSExpression(forConstantValue: UIColor.systemRed)
        stopsCircles.circleStrokeWidth = NSExpression(forConstantValue: 2.0)
        stopsCircles.circleStrokeColor = NSExpression(forConstantValue: UIColor.white)
        style.addLayer(stopsCircles)

        let stopsLabels = MLNSymbolStyleLayer(identifier: stopsLabelsId, source: stopsSource)
        stopsLabels.text = NSExpression(forKeyPath: "label")
        stopsLabels.textColor = NSExpression(forConstantValue: UIColor.white)
        stopsLabels.textHaloColor = NSExpression(forConstantValue: UIColor.black)
        stopsLabels.textHaloWidth = NSExpression(forConstantValue: 1.0)
        stopsLabels.textFontSize = NSExpression(forConstantValue: 11.0)
        stopsLabels.textAnchor = NSExpression(forConstantValue: "top")
        style.addLayer(stopsLabels)
    }

    /// Fits the camera to the plan's full, untrimmed polyline bounds (plus `origin`/
    /// `destination`, if given, in case a display trim would otherwise pull the bounds in from
    /// the actual pin).
    static func fitToRoute(
        mapView: MLNMapView, plan: FfiPlan, origin: CLLocationCoordinate2D? = nil,
        destination: CLLocationCoordinate2D? = nil
    ) {
        guard !plan.polyline.isEmpty else { return }
        var minLat = plan.polyline[0].lat
        var maxLat = minLat
        var minLon = plan.polyline[0].lon
        var maxLon = minLon
        for p in plan.polyline {
            minLat = min(minLat, p.lat)
            maxLat = max(maxLat, p.lat)
            minLon = min(minLon, p.lon)
            maxLon = max(maxLon, p.lon)
        }
        for coordinate in [origin, destination].compactMap({ $0 }) {
            minLat = min(minLat, coordinate.latitude)
            maxLat = max(maxLat, coordinate.latitude)
            minLon = min(minLon, coordinate.longitude)
            maxLon = max(maxLon, coordinate.longitude)
        }
        let bounds = MLNCoordinateBoundsMake(
            CLLocationCoordinate2D(latitude: minLat, longitude: minLon),
            CLLocationCoordinate2D(latitude: maxLat, longitude: maxLon))
        mapView.setVisibleCoordinateBounds(
            bounds,
            edgePadding: UIEdgeInsets(top: 60, left: 40, bottom: 60, right: 40),
            animated: true,
            completionHandler: nil)
    }
}
