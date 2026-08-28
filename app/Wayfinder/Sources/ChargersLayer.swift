// Chargers layer, ported from prototype/planner-ui's `addChargersLayers` (PlanStore.swift):
// same muted circle styling, radius interpolated by power_kw. The prototype read a pre-built
// `chargers.geojson`; production has no such file, so `makeSource` builds the GeoJSON feature
// collection at runtime from the parsed cpack-1 charger records (CPack1.swift) and writes it
// to a temp file, loaded via `MLNShapeSource(identifier:url:...)`.
//
// NOT clustered, unlike the prototype: on MapLibre 6.29.0, `MLNShapeSourceOptionClustered`
// silently drops every feature from this source -- confirmed by adding an unconditional,
// unpredicated debug circle layer on the clustered source, which rendered nothing either. The
// same source with clustering disabled renders all 1,549 points correctly. This looks like a
// genuine MapLibre 6.29.0 issue with dynamic GeoJSON clustering, not a mistake in this port;
// worth a follow-up ticket if clustering is wanted later.
import Foundation
import MapLibre
import UIKit

enum ChargersLayer {
    static let sourceId = "chargers"
    static let pointsLayerId = "chargers-points"

    static func makeSource(chargers: [CPack1Charger]) -> MLNShapeSource? {
        let features: [[String: Any]] = chargers.map { c in
            var properties: [String: Any] = ["name": c.name, "power_kw": c.maxPowerKw]
            if let operatorName = c.operatorName { properties["operator"] = operatorName }
            return [
                "type": "Feature",
                "geometry": ["type": "Point", "coordinates": [c.lon, c.lat]],
                "properties": properties,
            ]
        }
        let collection: [String: Any] = ["type": "FeatureCollection", "features": features]
        guard let data = try? JSONSerialization.data(withJSONObject: collection) else {
            return nil
        }
        let dst = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent("wayfinder-chargers.geojson")
        guard (try? data.write(to: dst, options: .atomic)) != nil else {
            return nil
        }
        return MLNShapeSource(identifier: sourceId, url: dst, options: nil)
    }

    static func addLayers(to style: MLNStyle, source: MLNShapeSource) {
        style.addSource(source)

        // Subdued gray-green, only from zoom ~8, so the Plan's own Charging Stops layer
        // (bright red, RouteLayer.swift) stays visually dominant.
        let mutedColor = UIColor(red: 0.56, green: 0.64, blue: 0.56, alpha: 1.0)

        let radiusStops: NSDictionary = [0: 2.5, 50: 3.0, 150: 3.75, 350: 4.5]
        let points = MLNCircleStyleLayer(identifier: pointsLayerId, source: source)
        points.circleRadius = NSExpression(
            format: "mgl_interpolate:withCurveType:parameters:stops:(power_kw, 'linear', nil, %@)",
            radiusStops)
        points.circleColor = NSExpression(forConstantValue: mutedColor)
        points.circleOpacity = NSExpression(forConstantValue: 0.6)
        points.circleStrokeWidth = NSExpression(forConstantValue: 0.5)
        points.circleStrokeColor = NSExpression(forConstantValue: UIColor.white.withAlphaComponent(0.6))
        points.minimumZoomLevel = 8
        style.addLayer(points)
    }
}
