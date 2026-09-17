// THROWAWAY PROTOTYPE (wayfinder #90): 3D Drive Mode -- extruded buildings, a re-tuned Follow
// Camera and an fps readout, every one of them behind a launch-argument gate so the shipped app
// is bit-for-bit unchanged without them. Nothing here is meant to survive: no tests, no
// abstractions, hard-coded numbers straight out of the two research notes
// (docs/research/maplibre-extrusions-ios.md §6, docs/research/follow-camera-behaviour.md §8).
//
// Gates (NSArgumentDomain, i.e. `xcrun simctl launch <udid> <bundle> -proto3d 1 ...`):
//   -proto3d 1        extruded buildings in Drive Mode
//   -protoCamera 1    the re-tuned Follow Camera
//   -protoFps 1       the 5 s fps/os_log readout
// Knobs: -protoOpacity, -protoMinzoom, -protoPitch, -protoZoomMin, -protoZoomMax, -protoLookAheadS,
// and the two diagnostic ones -protoLod, -protoInset. All of them are in docs/agents/ui-e2e.md
// under "3D Drive Mode prototype".
import Foundation
import MapLibre
import os
import UIKit

enum ProtoFlags {
    static var extrusionsOn: Bool { UserDefaults.standard.bool(forKey: "proto3d") }
    static var cameraOn: Bool { UserDefaults.standard.bool(forKey: "protoCamera") }
    static var fpsOn: Bool { UserDefaults.standard.bool(forKey: "protoFps") }

    static var opacity: Double { double("protoOpacity", default: 0.75) }
    static var minzoom: Double { double("protoMinzoom", default: 15) }
    static var pitch: Double { double("protoPitch", default: 60) }
    static var zoomMin: Double { double("protoZoomMin", default: 14.5) }
    static var zoomMax: Double { double("protoZoomMax", default: 17) }
    static var lookAheadS: Double { double("protoLookAheadS", default: 32) }
    /// Diagnostic knobs, added while chasing a blank-basemap regression on the first run:
    /// `-protoLod <deg>` (60 disables variable tile LOD again) and `-protoInset 0` (drops the
    /// contentInset framing, centring the vehicle like the shipped camera).
    static var lodPitchDeg: Double { double("protoLod", default: 30) }
    static var insetOn: Bool { UserDefaults.standard.object(forKey: "protoInset") == nil
        || UserDefaults.standard.bool(forKey: "protoInset") }

    /// `UserDefaults.double(forKey:)` returns 0 for an absent key, which would silently zero a
    /// knob; presence is checked explicitly instead.
    private static func double(_ key: String, default fallback: Double) -> Double {
        guard UserDefaults.standard.object(forKey: key) != nil else { return fallback }
        return UserDefaults.standard.double(forKey: key)
    }
}

/// The `MLNFillExtrusionStyleLayer` + `MLNLight` pair, per the extrusions research note's
/// "Recommended layer + light configuration". Installed once per style load (the light/dark swap
/// reloads the style, so `PlanStore.mapView(_:didFinishLoading:)` re-installs), hidden until
/// Drive Mode asks for it: the layer's predicate is set at creation and never mutated
/// (maplibre-native #3039), so visibility is an `isVisible` toggle.
@MainActor
enum ProtoExtrusions {
    static let layerId = "proto-buildings-3d"
    /// The flat fill this replaces while visible -- style-light.json / style-dark.json layer id.
    private static let flatBuildingsId = "buildings"
    private static let sourceId = "protomaps"

    static func install(style: MLNStyle, mapView: MLNMapView, isDark: Bool) {
        guard ProtoFlags.extrusionsOn else { return }
        if let existing = style.layer(withIdentifier: layerId) { style.removeLayer(existing) }
        guard let source = style.source(withIdentifier: sourceId) else { return }

        let minzoom = ProtoFlags.minzoom
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
        layer.fillExtrusionOpacity = NSExpression(forConstantValue: ProtoFlags.opacity)
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

        // Radians; the 60 deg default never fires at the 60 deg camera cap, so variable LOD is
        // off unless lowered (maplibre-native #2958).
        mapView.tileLodPitchThreshold = ProtoFlags.lodPitchDeg * .pi / 180
    }

    /// Flat fill off while the extrusion is on, and back on when it goes away.
    static func setVisible(_ visible: Bool, style: MLNStyle) {
        guard ProtoFlags.extrusionsOn else { return }
        style.layer(withIdentifier: layerId)?.isVisible = visible
        style.layer(withIdentifier: flatBuildingsId)?.isVisible = !visible
    }
}

/// `-protoFps 1`: counts `mapViewDidFinishRenderingFrame` callbacks and logs a line every 5 s at
/// os_log DEFAULT level (debug never reaches `devicectl --console`). Simulator numbers are not
/// the acceptance measure -- the phone is.
@MainActor
final class ProtoFpsMeter {
    private static let log = Logger(subsystem: "org.anteras.wayfinder", category: "proto.fps")
    private static let windowS = 5.0

    private var windowStart: CFTimeInterval?
    private var lastFrameAt: CFTimeInterval?
    private var frames = 0
    private var minIntervalS = Double.greatestFiniteMagnitude

    func recordFrame(mapView: MLNMapView, extrusionsVisible: Bool) {
        let now = CACurrentMediaTime()
        if let last = lastFrameAt { minIntervalS = min(minIntervalS, now - last) }
        lastFrameAt = now
        frames += 1

        guard let start = windowStart else {
            windowStart = now
            return
        }
        let elapsed = now - start
        guard elapsed >= Self.windowS else { return }

        let fps = Double(frames) / elapsed
        let minMs = minIntervalS == .greatestFiniteMagnitude ? 0 : minIntervalS * 1000
        Self.log.notice("""
            fps=\(fps, format: .fixed(precision: 1)) frames=\(self.frames) window=\(elapsed, format: .fixed(precision: 2))s \
            minFrameMs=\(minMs, format: .fixed(precision: 2)) \
            zoom=\(mapView.zoomLevel, format: .fixed(precision: 2)) \
            pitch=\(mapView.camera.pitch, format: .fixed(precision: 1)) \
            extrusions=\(extrusionsVisible ? "on" : "off")
            """)
        windowStart = now
        frames = 0
        minIntervalS = .greatestFiniteMagnitude
    }
}

/// `-protoCamera 1`: the Follow Camera re-tune's pure arithmetic, kept out of DriveStore so the
/// gated branch there stays a handful of lines.
enum ProtoCamera {
    /// Look-ahead distance in metres from speed, pulled in by an upcoming manoeuvre.
    static func lookAheadM(speedMps: Double, dTurnM: Double?) -> Double {
        var d = min(max(max(0, speedMps) * ProtoFlags.lookAheadS, 200), 1500)
        if let dTurnM { d = min(d, dTurnM + 100) }
        return max(200, d)
    }

    /// 200 m -> zoomMax (17), 1500 m -> zoomMin (14.5), linear in between.
    static func zoom(forLookAheadM lookAheadM: Double) -> Double {
        let zoomMax = ProtoFlags.zoomMax
        let zoomMin = ProtoFlags.zoomMin
        let t = min(max((lookAheadM - 200) / (1500 - 200), 0), 1)
        return zoomMax + (zoomMin - zoomMax) * t
    }

    /// 0.1 zoom levels per second of wall clock, and frozen below 7 km/h.
    static func rateLimited(current: Double?, target: Double, dtS: Double, speedMps: Double) -> Double {
        guard let current else { return target }
        guard speedMps * 3.6 >= 7 else { return current }
        let maxStep = 0.1 * max(0, dtS)
        return current + min(max(target - current, -maxStep), maxStep)
    }

    /// Vehicle at ~70 % of the view height: the camera centre sits at the middle of the
    /// inset-adjusted content rect, so a heavy top inset pushes it down the screen.
    /// Banner/HUD heights are eyeballed off RootView's overlay -- an approximation is fine here.
    static func contentInset(viewHeight: CGFloat, bannerShown: Bool) -> UIEdgeInsets {
        guard ProtoFlags.insetOn, viewHeight > 0 else { return .zero }
        let bannerH: CGFloat = bannerShown ? 104 : 0
        let hudH: CGFloat = 168 // drive controls row + collapsed DriveCard
        // The camera centre lands at the middle of the inset-adjusted rect, i.e. at
        // (H + top - bottom) / 2. The ticket's literal "top = 0.4H (+ banner)" puts that at 60 %
        // of the height; the research note's `0.4H + bottomHUD` is what actually yields the 70 %
        // the ticket asks for, so the HUD height is added to the top as well.
        return UIEdgeInsets(top: 0.4 * viewHeight + hudH + bannerH, left: 0, bottom: hudH, right: 0)
    }
}
