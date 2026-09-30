// Follow Camera (wayfinder #92, ADR 0012 decision 3 amendment; research in
// docs/research/follow-camera-behaviour.md section 8): the phone's drive camera arithmetic, kept
// pure and out of DriveStore so the constants are readable in one place. DriveStore's
// `applyFollowingCamera` is the only caller. CarPlay keeps its own constants for now.
import UIKit

enum FollowCamera {
    /// Fixed pitch; the look-ahead zoom carries all the "how far ahead" information.
    static let pitchDeg: Double = 60

    /// Look-ahead distance in metres: speed x 32 s clamped to 200...1500 m, pulled in to just past
    /// the next manoeuvre (dTurnM + 100 m, never below 200 m) so the turn is framed as it nears.
    static func lookAheadM(speedMps: Double, dTurnM: Double?) -> Double {
        var d = min(max(max(0, speedMps) * 32, 200), 1500)
        if let dTurnM { d = min(d, dTurnM + 100) }
        return max(200, d)
    }

    /// 200 m -> zoom 17, 1500 m -> zoom 14.5, linear in between and clamped.
    static func zoom(forLookAheadM lookAheadM: Double) -> Double {
        let t = min(max((lookAheadM - 200) / (1500 - 200), 0), 1)
        return 17 + (14.5 - 17) * t
    }

    /// Commanded zoom: steps toward `target` by at most 0.1 zoom levels per wall-clock second, and
    /// holds still below 7 km/h (a crawl or standstill must not breathe the map). The first call
    /// (nil `current`) adopts the target outright.
    static func rateLimited(current: Double?, target: Double, dtS: Double, speedMps: Double) -> Double {
        guard let current else { return target }
        guard speedMps * 3.6 >= 7 else { return current }
        let maxStep = 0.1 * max(0, dtS)
        return current + min(max(target - current, -maxStep), maxStep)
    }

    /// Vehicle at 70 % of the VISIBLE map strip (banner bottom to HUD top), not of the whole view.
    /// MapLibre centres the camera in the inset-adjusted rect, i.e. at (H + top - bottom) / 2; with
    /// top = banner + 0.4 x visible and bottom = hud that is banner + 0.7 x visible. The ticket's
    /// literal `top = 0.4H + hud + banner` lands at 0.7H + banner/2 whatever the HUD height, so an
    /// expanded DriveCard covered the puck; this rule lifts the puck when the card expands instead.
    static func contentInset(viewHeight: CGFloat, hudHeight: CGFloat, bannerHeight: CGFloat) -> UIEdgeInsets {
        let visible = viewHeight - bannerHeight - hudHeight
        guard visible > 0 else { return .zero }
        return UIEdgeInsets(top: bannerHeight + 0.4 * visible, left: 0, bottom: hudHeight, right: 0)
    }
}
