// Per-region geographic bounds (wayfinder #47), shared by PlanStore's CoreLocation origin-fix
// gate and SearchModel's MKLocalSearchCompleter bias -- ADR 0007 fixes the v1 catalog to
// exactly these three regions, so a small static table replaces the single hardcoded corridor
// box both call sites used to carry independently.

enum RegionBounds {
    struct Box {
        let latRange: ClosedRange<Double>
        let lonRange: ClosedRange<Double>
    }

    private static let byRegion: [String: Box] = [
        "corridor": Box(latRange: 49.4...53.6, lonRange: 2.5...7.3),
        "lu-dev": Box(latRange: 49.4...50.2, lonRange: 5.7...6.6),
        "eu-west": Box(latRange: 41.3...55.1, lonRange: -5.6...15.1),
    ]

    static func box(for region: String) -> Box {
        byRegion[region] ?? byRegion["corridor"]!
    }
}
