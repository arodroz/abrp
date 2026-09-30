// 120 fps while driving (wayfinder #95, ADR 0002's 120 fps bar). MapLibre's own display link
// never asks ProMotion for more than 60 Hz, so it needs BOTH of these (measured on an iPhone 15
// Pro): `preferredFramesPerSecond = 120` alone stays at 60 because the display never leaves
// 60 Hz; a 120 Hz `CADisplayLink` alone ticks the display at 120 but MapLibre still draws at
// 60. Together MapLibre renders at 119.8-120. The link's callback is a no-op -- it exists only
// to request the frame-rate range. (`CADisableMinimumFrameDurationOnPhone` is set in Info.plist.)
import MapLibre
import QuartzCore

@MainActor
enum DriveFrameRate {
    /// CADisplayLink retains its target strongly, so the link targets this throwaway object.
    private final class Target: NSObject {
        @objc func tick() {}
    }

    private static var link: CADisplayLink?

    static func start(mapView: MLNMapView) {
        mapView.preferredFramesPerSecond = MLNMapViewPreferredFramesPerSecond(rawValue: 120)
        guard link == nil else { return }
        let link = CADisplayLink(target: Target(), selector: #selector(Target.tick))
        link.preferredFrameRateRange = CAFrameRateRange(minimum: 80, maximum: 120, preferred: 120)
        link.add(to: .main, forMode: .common)
        self.link = link
    }

    static func stop(mapView: MLNMapView) {
        link?.invalidate()
        link = nil
        mapView.preferredFramesPerSecond = .default
    }
}
