// 3D Drive Mode's extruded buildings (wayfinder #91; prototype resolution on #90, research in
// docs/research/maplibre-extrusions-ios.md §6): one `MLNFillExtrusionStyleLayer` on the
// protomaps `buildings` source-layer plus the `MLNLight` and tile-LOD threshold that go with it.
// Installed once per style load (the light/dark swap reloads the style, so
// `PlanStore.mapView(_:didFinishLoading:)` re-installs), hidden until Drive Mode asks for it:
// the layer's predicate is set at creation and never mutated (maplibre-native #3039), so
// visibility is an `isVisible` toggle. The planning map and CarPlay's own map never show it.
import Foundation
import MapLibre
import UIKit

@MainActor
enum DriveExtrusions {
    static let layerId = "buildings-3d"
    /// The flat fill this replaces while visible -- style-light.json / style-dark.json layer id.
    private static let flatBuildingsId = "buildings"
    private static let sourceId = "protomaps"
    private static let minzoom: Double = 15
    /// The flat fill's own max zoom, read fresh at every install (a style reload recreates it).
    private static var flatDefaultMaxZoom: Float?

    static func install(style: MLNStyle, mapView: MLNMapView, isDark: Bool) {
        if let existing = style.layer(withIdentifier: layerId) { style.removeLayer(existing) }
        flatDefaultMaxZoom = style.layer(withIdentifier: flatBuildingsId)?.maximumZoomLevel
        guard let source = style.source(withIdentifier: sourceId) else { return }

        let layer = MLNFillExtrusionStyleLayer(identifier: layerId, source: source)
        layer.sourceLayerIdentifier = "buildings"
        layer.predicate = NSPredicate(format: "kind IN %@", ["building", "building_part"])
        layer.minimumZoomLevel = Float(minzoom)

        // Height/base fade in over half a zoom level so buildings grow rather than pop.
        // Plain `building` features carry no `min_height` (only `building_part` does), hence the
        // coalesce.
        let heightStops: [NSNumber: NSExpression] = [
            NSNumber(value: minzoom): NSExpression(forConstantValue: 0),
            NSNumber(value: minzoom + 0.5): NSExpression(forKeyPath: "height"),
        ]
        let baseStops: [NSNumber: NSExpression] = [
            NSNumber(value: minzoom): NSExpression(forConstantValue: 0),
            NSNumber(value: minzoom + 0.5): NSExpression(format: "mgl_coalesce({min_height, 0})"),
        ]
        layer.fillExtrusionHeight = NSExpression(
            format: "mgl_interpolate:withCurveType:parameters:stops:($zoomLevel, 'linear', nil, %@)", heightStops)
        layer.fillExtrusionBase = NSExpression(
            format: "mgl_interpolate:withCurveType:parameters:stops:($zoomLevel, 'linear', nil, %@)", baseStops)

        layer.fillExtrusionColor = NSExpression(forConstantValue: isDark
            ? UIColor(white: 0.28, alpha: 1) : UIColor(white: 0.86, alpha: 1))
        layer.fillExtrusionOpacity = NSExpression(forConstantValue: 0.45)
        layer.fillExtrusionHasVerticalGradient = NSExpression(forConstantValue: true)
        // 5x memory + fps drops on a z14-capped source (maplibre-native #4343/#4542).
        layer.fillExtrusionRoundedCornerDistance = NSExpression(forConstantValue: 0)
        layer.isVisible = false

        // Above the roads, below the route: RouteLayer inserts its ribbon immediately below the
        // first symbol layer, so going below THAT ribbon lands the extrusion under the casing and
        // over every road layer. A later RouteLayer.addLayers (replan) re-inserts the route below
        // the first symbol layer again, i.e. still above this one.
        if let route = style.layer(withIdentifier: RouteLayer.routeLineId) {
            style.insertLayer(layer, below: route)
        } else if let firstSymbol = style.layers.first(where: { $0 is MLNSymbolStyleLayer }) {
            style.insertLayer(layer, below: firstSymbol)
        } else {
            style.addLayer(layer)
        }

        let light = MLNLight()
        light.anchor = NSExpression(forConstantValue: "viewport")
        light.position = NSExpression(
            forConstantValue: NSValue(mlnSphericalPosition: MLNSphericalPositionMake(1.15, 210, 40)))
        light.intensity = NSExpression(forConstantValue: 0.35)
        light.color = NSExpression(forConstantValue: isDark ? UIColor(white: 0.9, alpha: 1) : UIColor.white)
        style.light = light

        // Radians; the 60 deg default never fires at the 60 deg camera cap, so variable LOD
        // would be off (maplibre-native #2958).
        mapView.tileLodPitchThreshold = 30 * .pi / 180
    }

    /// While the extrusion is on, the flat fill is capped at the extrusion's minzoom rather than
    /// hidden: the Follow Camera zooms out to 14.5 at speed, and hiding the fill outright left no
    /// buildings at all below z15. Uncapped again when the extrusion goes away.
    static func setVisible(_ visible: Bool, style: MLNStyle) {
        style.layer(withIdentifier: layerId)?.isVisible = visible
        guard let flat = style.layer(withIdentifier: flatBuildingsId), let flatDefaultMaxZoom else { return }
        flat.maximumZoomLevel = visible ? Float(minzoom) : flatDefaultMaxZoom
    }
}
