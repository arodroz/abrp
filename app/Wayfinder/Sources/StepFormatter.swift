// EN maneuver-banner text templates (wayfinder #67, ADR 0012 point 3's HUD line extended to
// turn-by-turn): pure, static functions over an `FfiStep` -- the app owns instruction wording
// here since the pack ships only structured `FfiManeuver`/`FfiManeuverModifier`/road-name data
// (core/ffi/src/mapping.rs), no localized strings. `roadLabel` prefers the step's own `name`,
// falling back to `roadRef` with OSM's semicolon-joined alternatives rendered as " / " -- same
// display convention `dest`/`destRef` use in `secondary` below. No locale/i18n here: EN only,
// same as the rest of the app in this slice.
import PlannerKit

enum StepFormatter {
    static func primary(_ step: FfiStep, arriveLabel: String?) -> String {
        switch step.maneuver {
        case .depart:
            return "Depart"
        case .arrive:
            if let arriveLabel { return "Arrive at \(arriveLabel)" }
            return "Arrive at your destination"
        case .turn:
            let base: String
            switch step.modifier {
            case .straight: base = "Continue straight"
            case .slightLeft: base = "Turn slightly left"
            case .slightRight: base = "Turn slightly right"
            case .left: base = "Turn left"
            case .right: base = "Turn right"
            case .sharpLeft: base = "Turn sharp left"
            case .sharpRight: base = "Turn sharp right"
            case .uTurn: base = "Make a U-turn"
            }
            if let label = roadLabel(step), step.modifier != .uTurn {
                return "\(base) onto \(label)"
            }
            return base
        case .continue:
            guard let label = roadLabel(step) else { return "Continue" }
            if step.modifier == .straight {
                return "Continue on \(label)"
            }
            return "Continue \(directionPhrase(step.modifier)) on \(label)"
        case .offRamp:
            if !step.exitRef.isEmpty { return "Take exit \(step.exitRef)" }
            return "Take the exit" + sideSuffix(step.modifier)
        case .onRamp:
            return "Take the ramp" + sideSuffix(step.modifier)
        case .fork:
            switch sideFamily(step.modifier) {
            case .left: return "Keep left"
            case .right: return "Keep right"
            case .straight: return "Keep straight"
            }
        case .endOfRoad:
            let base: String
            switch sideFamily(step.modifier) {
            case .left: base = "At the end of the road, turn left"
            case .right: base = "At the end of the road, turn right"
            case .straight: base = "Continue at the end of the road"
            }
            if let label = roadLabel(step) { return base + " onto \(label)" }
            return base
        case .roundabout:
            if let exitCount = step.exitCount { return "At the roundabout, take the \(ordinal(exitCount)) exit" }
            return "At the roundabout"
        }
    }

    /// Signage line ("toward ..."), only for the maneuvers where the road itself isn't already
    /// named in `primary`.
    static func secondary(_ step: FfiStep) -> String? {
        switch step.maneuver {
        case .offRamp, .onRamp, .fork:
            if !step.dest.isEmpty { return "toward \(splitSemicolons(step.dest))" }
            if !step.destRef.isEmpty { return "toward \(splitSemicolons(step.destRef))" }
            return nil
        default:
            return nil
        }
    }

    static func iconSystemName(_ step: FfiStep) -> String {
        switch step.maneuver {
        case .roundabout: return "arrow.triangle.2.circlepath"
        case .fork: return "arrow.triangle.branch"
        case .arrive: return "mappin.and.ellipse"
        case .depart: return "arrow.up"
        case .offRamp, .onRamp:
            // Ramps default to bearing right rather than straight up (wayfinder #67 spec).
            return step.modifier == .straight ? "arrow.up.right" : modifierIcon(step.modifier)
        case .turn, .continue, .endOfRoad:
            return modifierIcon(step.modifier)
        }
    }

    static func formatDistance(_ m: Double) -> String {
        let clamped = max(0, m)
        if clamped >= 10_000 {
            return "\(Int((clamped / 1000).rounded())) km"
        }
        if clamped >= 1_000 {
            return String(format: "%.1f km", clamped / 1000)
        }
        return "\(Int((clamped / 10).rounded() * 10)) m"
    }

    /// Spoken form of a distance for voice prompts (wayfinder #68) -- coarser than
    /// `formatDistance`'s HUD/banner text, since a spoken number needs to be sayable in one
    /// breath rather than exact: nearest 50 m (min 50) under 1 km, one decimal (trailing .0
    /// dropped) from 1-10 km, whole km above that; "1 kilometer" singular exactly at "1".
    static func spokenDistance(_ meters: Double) -> String {
        let clamped = max(0, meters)
        if clamped >= 10_000 {
            return "\(Int((clamped / 1000).rounded())) kilometers"
        }
        if clamped >= 1_000 {
            let km = (clamped / 1000 * 10).rounded() / 10
            var rendered = String(format: "%.1f", km)
            if rendered.hasSuffix(".0") { rendered.removeLast(2) }
            return rendered == "1" ? "1 kilometer" : "\(rendered) kilometers"
        }
        let roundedM = max(50, Int((clamped / 50).rounded() * 50))
        return "\(roundedM) meters"
    }

    // MARK: Helpers

    private static func roadLabel(_ step: FfiStep) -> String? {
        if !step.name.isEmpty { return step.name }
        if !step.roadRef.isEmpty { return splitSemicolons(step.roadRef) }
        return nil
    }

    private static func splitSemicolons(_ value: String) -> String {
        value.replacingOccurrences(of: ";", with: " / ")
    }

    private enum Side { case left, right, straight }

    private static func sideFamily(_ modifier: FfiManeuverModifier) -> Side {
        switch modifier {
        case .slightLeft, .left, .sharpLeft: return .left
        case .slightRight, .right, .sharpRight: return .right
        case .straight, .uTurn: return .straight
        }
    }

    private static func sideSuffix(_ modifier: FfiManeuverModifier) -> String {
        switch sideFamily(modifier) {
        case .left: return " on the left"
        case .right: return " on the right"
        case .straight: return ""
        }
    }

    /// `.continue`'s non-straight direction phrase. `.uTurn` never reaches this in practice
    /// (a continuation maneuver doesn't reverse direction) but the switch must stay total.
    private static func directionPhrase(_ modifier: FfiManeuverModifier) -> String {
        switch modifier {
        case .straight: return "straight"
        case .slightLeft: return "slightly left"
        case .slightRight: return "slightly right"
        case .left: return "left"
        case .right: return "right"
        case .sharpLeft: return "sharp left"
        case .sharpRight: return "sharp right"
        case .uTurn: return "left"
        }
    }

    private static func modifierIcon(_ modifier: FfiManeuverModifier) -> String {
        switch modifier {
        case .straight: return "arrow.up"
        case .slightLeft: return "arrow.up.left"
        case .slightRight: return "arrow.up.right"
        case .left: return "arrow.turn.up.left"
        case .right: return "arrow.turn.up.right"
        case .sharpLeft: return "arrow.turn.left.down"
        case .sharpRight: return "arrow.turn.right.down"
        case .uTurn: return "arrow.uturn.left"
        }
    }

    private static func ordinal(_ n: UInt32) -> String {
        let mod100 = n % 100
        let suffix: String
        if (11...13).contains(mod100) {
            suffix = "th"
        } else {
            switch n % 10 {
            case 1: suffix = "st"
            case 2: suffix = "nd"
            case 3: suffix = "rd"
            default: suffix = "th"
            }
        }
        return "\(n)\(suffix)"
    }
}
