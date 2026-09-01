// SoC-over-distance chart (wayfinder #43), overhauled for legibility (wayfinder #83): arrival-
// SoC callouts at every Charging Stop and the destination (colored red/amber/green by margin --
// ABRP's per-stop annotation), a stop glyph (bolt + charge minutes) replacing the old anonymous
// dashed line, a low-opacity danger band under the charger floor, a driven-vs-ahead split of the
// predicted curve with a live-position dot in drive mode (replacing the old bare white
// RuleMark), an optional actual-SoC trail overlay (Tesla-style, drive mode only), labeled axes,
// and a scrub readout chip while dragging in planning mode. All the computations (callouts,
// margin coloring, curve interpolation/splitting, trail thinning) are pure functions in
// SoCChartModel.swift -- this view only renders what those hand it. Still ported off `store`'s
// typed `displayedPlan` (FfiPlan) and PlanStore's `selectedDistanceM`/scrub-marker plumbing;
// `pinnedDistanceM` (wayfinder #60) still repurposes this chart as the drive HUD's live-position
// marker, with the scrub gesture still disabled in that mode -- see that parameter's own comment.
import Charts
import PlannerKit
import SwiftUI

struct SoCChartView: View {
    let store: PlanStore
    var height: CGFloat = 220
    var showAxes: Bool = true
    /// Drive HUD's live snapped-position marker (wayfinder #60): when set, the live-position
    /// marker tracks this distance instead of `store.selectedDistanceM`, and the predicted curve
    /// splits into driven/ahead at it (wayfinder #83).
    var pinnedDistanceM: Double? = nil
    /// The actual-SoC trail (wayfinder #83, `DriveStore.socTrail`): empty in planning mode (no
    /// call site passes it) and whenever drive mode has no live telemetry yet.
    var trail: [SoCChartModel.SoCTrailPoint] = []
    /// The live OBD Display SoC at `pinnedDistanceM`, already freshness-checked by the caller
    /// (wayfinder #83, design point 4) -- nil falls back to the model's own interpolated curve
    /// value, same as before telemetry existed.
    var liveSocPct: Double? = nil
    /// Red/amber boundary for `SoCChartModel.socMarginColor` and the danger band (wayfinder #83):
    /// callers pass `store.chargerArrivalMinSoc * 100` (PlanStore.swift) where reachable; this
    /// default matches that setting's own 0.10 default for callers (and card-smoke) that don't.
    var amberFloorPct: Double = 10

    /// Scrub readout chip (wayfinder #83, design point 7): local to this view, set only while the
    /// planning-mode drag gesture below is active -- PlanStore's own scrub-marker plumbing is
    /// untouched.
    @State private var isScrubbing = false

    var body: some View {
        Chart {
            if let plan = store.displayedPlan {
                chartContent(plan)
            }
        }
        .chartYScale(domain: 0...100)
        .chartXAxis {
            if showAxes {
                AxisMarks(values: .automatic) { value in
                    // No vertical gridlines beyond the stop RuleMarks already in the plot.
                    AxisTick()
                    AxisValueLabel {
                        if let km = value.as(Double.self) {
                            Text("\(Int(km)) km")
                        }
                    }
                }
            }
        }
        .chartYAxis {
            if showAxes {
                AxisMarks(values: [0, 50, 100]) { value in
                    AxisGridLine().foregroundStyle(.gray.opacity(0.15))
                    AxisValueLabel {
                        if let pct = value.as(Double.self) {
                            Text("\(Int(pct))%")
                        }
                    }
                }
            }
        }
        .chartOverlay { proxy in
            GeometryReader { geo in
                ZStack(alignment: .top) {
                    // Pinned mode (wayfinder #60) installs no gesture at all: writing
                    // `store.selectedDistanceM` triggers `PlanStore.updateScrubMarker`'s
                    // `mapView.setCenter`, which would fight the following camera mid-drive.
                    if pinnedDistanceM == nil {
                        Rectangle()
                            .fill(Color.clear)
                            .contentShape(Rectangle())
                            .gesture(
                                DragGesture(minimumDistance: 0)
                                    .onChanged { value in
                                        isScrubbing = true
                                        let plotFrame = geo[proxy.plotAreaFrame]
                                        let xPos = value.location.x - plotFrame.origin.x
                                        guard let distKm: Double = proxy.value(atX: xPos) else { return }
                                        let maxKm = (store.displayedPlan?.totalDistM ?? 0) / 1000
                                        store.selectedDistanceM = min(max(distKm, 0), maxKm) * 1000
                                    }
                                    .onEnded { _ in isScrubbing = false }
                            )
                    }
                    if pinnedDistanceM == nil, isScrubbing, let plan = store.displayedPlan, let sel = store.selectedDistanceM {
                        scrubReadout(plan: plan, distanceM: sel)
                            .padding(.top, 4)
                            .allowsHitTesting(false)
                    }
                }
            }
        }
        .frame(height: height)
        .accessibilityIdentifier("soc-chart")
    }

    @ChartContentBuilder
    private func chartContent(_ plan: FfiPlan) -> some ChartContent {
        // Danger band (wayfinder #83, design point 3): below the charger floor, callout colors
        // already carry the warning, so no extra shading past the hairline at the floor itself.
        RectangleMark(yStart: .value("Danger min", 0), yEnd: .value("Danger max", amberFloorPct))
            .foregroundStyle(.red.opacity(0.08))
        RuleMark(y: .value("Floor", amberFloorPct))
            .foregroundStyle(.red.opacity(0.3))
            .lineStyle(StrokeStyle(lineWidth: 1))

        // Predicted curve, split at `pinnedDistanceM` (design point 4): with no pin (planning
        // mode), `behind` is empty and the whole curve renders at full opacity, same as before.
        // Every line below carries an explicit `series:`: without one, Swift Charts folds all
        // LineMarks sharing the same x/y value labels into a single path, and the driven/ahead
        // segments plus the trail render as one tangled zigzag (caught in the reviewer's
        // chart-demo-drive screenshot).
        let split = pinnedDistanceM.map { SoCChartModel.splitCurve(plan.socCurve, at: $0) } ?? (behind: [], ahead: plan.socCurve)
        ForEach(Array(split.behind.enumerated()), id: \.offset) { _, sample in
            LineMark(
                x: .value("Distance", sample.distM / 1000), y: .value("SoC", sample.soc * 100),
                series: .value("Series", "predicted-driven")
            )
            .foregroundStyle(.green.opacity(0.35))
            AreaMark(
                x: .value("Distance", sample.distM / 1000), y: .value("SoC", sample.soc * 100),
                series: .value("Series", "predicted-driven-fill")
            )
            .foregroundStyle(.green.opacity(0.05))
        }
        ForEach(Array(split.ahead.enumerated()), id: \.offset) { _, sample in
            LineMark(
                x: .value("Distance", sample.distM / 1000), y: .value("SoC", sample.soc * 100),
                series: .value("Series", "predicted-ahead")
            )
            .foregroundStyle(.green)
            AreaMark(
                x: .value("Distance", sample.distM / 1000), y: .value("SoC", sample.soc * 100),
                series: .value("Series", "predicted-ahead-fill")
            )
            .foregroundStyle(.green.opacity(0.15))
        }

        // Actual-SoC trail (wayfinder #83, design point 5): only drawn once non-empty -- a
        // dongle-free drive (or planning mode, which never passes one) leaves the chart
        // identical to the single predicted curve above.
        if !trail.isEmpty {
            ForEach(Array(trail.enumerated()), id: \.offset) { _, point in
                LineMark(
                    x: .value("Distance", point.distM / 1000), y: .value("SoC", point.socPct),
                    series: .value("Series", "actual")
                )
                .foregroundStyle(.white.opacity(0.85))
                .lineStyle(StrokeStyle(lineWidth: 2))
            }
        }

        // Stop glyphs (wayfinder #83, design point 2): a thinner dashed rule than before, with a
        // compact bolt + charge-minutes label at its top -- no charger names here, those live on
        // the stop list/map already.
        ForEach(ChargingStopVM.stops(from: plan)) { stop in
            RuleMark(x: .value("Stop", stop.distFromStartM / 1000))
                .foregroundStyle(.secondary.opacity(0.4))
                .lineStyle(StrokeStyle(lineWidth: 1, dash: [3, 2]))
                // fit(to: .chart) on every annotation below: the compact drive chart (160pt)
                // otherwise pushes top annotations out of the plot into the HUD header (caught
                // in the reviewer's chart-demo-drive screenshot).
                .annotation(
                    position: .top,
                    overflowResolution: .init(x: .fit(to: .chart), y: .fit(to: .chart))
                ) {
                    HStack(spacing: 2) {
                        Image(systemName: "bolt.fill")
                        Text(SoCChartModel.chargeMinutesLabel(stop.chargeS))
                    }
                    .font(.caption2)
                    .foregroundColor(.secondary)
                }
        }

        // Arrival-SoC callouts (wayfinder #83, design point 1): an invisible PointMark anchors
        // each annotation exactly at its arrival point (the curve's valley, or the destination's
        // final value); iOS 17's chart annotation layout keeps them off the stop-glyph labels
        // above.
        ForEach(SoCChartModel.arrivalCallouts(
            stops: ChargingStopVM.stops(from: plan), destinationDistM: plan.totalDistM,
            destinationSocFraction: plan.socCurve.last?.soc ?? 0, floorPct: amberFloorPct
        ), id: \.distM) { callout in
            PointMark(x: .value("Distance", callout.distM / 1000), y: .value("SoC", callout.socPct))
                .symbolSize(0)
                .annotation(
                    position: .top,
                    overflowResolution: .init(x: .fit(to: .chart), y: .fit(to: .chart))
                ) {
                    Text(callout.label)
                        .font(.caption2.bold())
                        .foregroundColor(callout.color)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(.regularMaterial, in: Capsule())
                }
        }

        // Live-position marker (wayfinder #83, design point 4): replaces the old bare white
        // RuleMark with a thin hairline plus a dot at the current SoC -- the live OBD reading
        // when fresh, else the model's own interpolated curve value, same as before telemetry
        // existed. Planning mode's scrub marker (`store.selectedDistanceM`) gets the same
        // treatment when no pin is set.
        if let markerDistM = pinnedDistanceM ?? store.selectedDistanceM {
            let currentPct = liveSocPct ?? SoCChartModel.interpolatedSocFraction(plan.socCurve, at: markerDistM) * 100
            RuleMark(x: .value("Now", markerDistM / 1000))
                .foregroundStyle(.white.opacity(0.6))
                .lineStyle(StrokeStyle(lineWidth: 1))
            PointMark(x: .value("Now", markerDistM / 1000), y: .value("SoC", currentPct))
                .foregroundStyle(.white)
                .symbolSize(70)
                .annotation(
                    position: .top,
                    overflowResolution: .init(x: .fit(to: .chart), y: .fit(to: .chart))
                ) {
                    Text(SoCChartModel.roundedPctLabel(currentPct))
                        .font(.caption2.bold())
                        .padding(.horizontal, 6)
                        .padding(.vertical, 2)
                        .background(.regularMaterial, in: Capsule())
                }
        }
    }

    /// The floating "134 km · 46%" chip (wayfinder #83, design point 7), shown near the top of
    /// the plot while the planning-mode scrub gesture is active.
    private func scrubReadout(plan: FfiPlan, distanceM: Double) -> some View {
        let pct = SoCChartModel.interpolatedSocFraction(plan.socCurve, at: distanceM) * 100
        return Text("\(Int((distanceM / 1000).rounded())) km \u{00B7} \(Int(pct.rounded()))%")
            .font(.caption2.bold())
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(.regularMaterial, in: Capsule())
    }
}
