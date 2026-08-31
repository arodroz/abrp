// Voice prompt scheduling (wayfinder #68, ADR 0012 point 3's HUD/banner line extended to spoken
// guidance): pure, no AV imports -- decides WHAT to say and WHEN from the SAME
// `StepTracker.GuidanceStep` array the maneuver banner (#67) consumes, so voice and the banner
// can never disagree. Tiered on time-to-maneuver and step approach length, the hand-rolled
// policy the research doc (docs/research/turn-by-turn.md §4) documents as Mapbox's legacy
// client-side heuristic -- no engine (MapKit/AVSpeech) ships this itself. Each (step, tier)
// fires at most once, and a lower tier can never fire after a higher one: the newest,
// closest-to-the-maneuver prompt always wins, which is "replace, don't queue" (SpeechController's
// job) expressed at the SCHEDULING layer -- a skipped tier is consumed, never queued for later.
import Foundation

enum VoicePromptScheduler {
    enum Tier: Int, Comparable {
        case far = 0
        case near = 1
        case now = 2

        static func < (lhs: Tier, rhs: Tier) -> Bool { lhs.rawValue < rhs.rawValue }
    }

    struct Prompt: Equatable {
        let stepIndex: Int
        let tier: Tier
        let text: String
    }

    /// Per-drive bookkeeping: which step the spoken tiers refer to, and the highest tier already
    /// spoken for it. Reset (`.init()`) whenever the step array itself changes -- a Plan
    /// adoption or a mid-drive replan swap (see DriveStore.snapshotPlan).
    struct State {
        var stepIndex: Int?
        var highestSpokenTier: Tier?

        init(stepIndex: Int? = nil, highestSpokenTier: Tier? = nil) {
            self.stepIndex = stepIndex
            self.highestSpokenTier = highestSpokenTier
        }
    }

    static let farTierTimeS = 70.0
    static let nearTierTimeS = 15.0
    static let nowTierDistanceM = 40.0
    /// Mapbox's legacy heuristic: a tier only fires if the step's own approach is long enough
    /// that the prompt has room to be useful (a short step doesn't get a 70-second-out prompt).
    static let farMinApproachM = 400.0
    static let nearMinApproachM = 100.0

    static func nextPrompt(
        state: inout State,
        steps: [StepTracker.GuidanceStep],
        upcomingIndex: Int,
        distanceAlongRouteM: Double,
        speedMPerS: Double
    ) -> Prompt? {
        guard steps.indices.contains(upcomingIndex) else { return nil }

        // A passed step's un-spoken tiers are dropped, never spoken late -- this is the
        // (step, tier) bookkeeping's reset point.
        if state.stepIndex != upcomingIndex {
            state.stepIndex = upcomingIndex
            state.highestSpokenTier = nil
        }

        let step = steps[upcomingIndex]
        let distanceToManeuverM = step.distAlongRouteM - distanceAlongRouteM
        let timeToManeuverS = distanceToManeuverM / max(speedMPerS, 1)
        let previousAnchorM = upcomingIndex > 0 ? steps[upcomingIndex - 1].distAlongRouteM : 0
        let approachM = step.distAlongRouteM - previousAnchorM

        let eligibleTier: Tier?
        if distanceToManeuverM <= nowTierDistanceM {
            eligibleTier = .now
        } else if timeToManeuverS <= nearTierTimeS, approachM >= nearMinApproachM {
            eligibleTier = .near
        } else if timeToManeuverS <= farTierTimeS, approachM >= farMinApproachM {
            eligibleTier = .far
        } else {
            eligibleTier = nil
        }

        guard let tier = eligibleTier else { return nil }
        if let highest = state.highestSpokenTier, tier <= highest { return nil }
        state.highestSpokenTier = tier

        let text: String
        switch tier {
        case .far:
            text = "In \(StepFormatter.spokenDistance(distanceToManeuverM)), \(lowercasedFirst(step.primary))"
        case .near:
            text = "In \(StepFormatter.spokenDistance(distanceToManeuverM)), \(lowercasedFirst(step.primary))" + thenSuffix(step)
        case .now:
            text = step.primary + thenSuffix(step)
        }
        return Prompt(stepIndex: upcomingIndex, tier: tier, text: text)
    }

    /// ".near"/".now" only: the next step's primary, lowercased, when StepTracker already chained
    /// it close behind (`then` is nil otherwise).
    private static func thenSuffix(_ step: StepTracker.GuidanceStep) -> String {
        step.then.map { ", then \(lowercasedFirst($0))" } ?? ""
    }

    /// First character lowercased, rest untouched ("Turn right..." -> "turn right...").
    private static func lowercasedFirst(_ s: String) -> String {
        guard let first = s.first else { return s }
        return first.lowercased() + s.dropFirst()
    }
}
