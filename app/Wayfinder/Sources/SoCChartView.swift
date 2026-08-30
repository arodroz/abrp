// SoC-over-distance chart (wayfinder #43): Line+AreaMark over the SoC curve, an orange dashed
// RuleMark per Charging Stop, and a draggable scrub overlay that publishes the selected
// distance to PlanStore (which moves a marker on the map -- see PlanStore.updateScrubMarker).
// Ported nearly verbatim from prototype/planner-ui's SoCChartView.swift, re-typed off
// FfiPlan/FfiSocPoint and PlanStore's `displayedPlan` (the main plan or, toggled, the
// stop-free alternative). `pinnedDistanceM` (wayfinder #60) repurposes this same chart as the
// drive HUD's live-position marker -- see its own comment for why the scrub gesture is
// disabled in that mode.
import Charts
import PlannerKit
import SwiftUI

struct SoCChartView: View {
    let store: PlanStore
    var height: CGFloat = 220
    var showAxes: Bool = true
    /// Drive HUD's live snapped-position marker (wayfinder #60): when set, the white RuleMark
    /// tracks this distance instead of `store.selectedDistanceM`.
    var pinnedDistanceM: Double? = nil

    var body: some View {
        Chart {
            if let plan = store.displayedPlan {
                ForEach(Array(plan.socCurve.enumerated()), id: \.offset) { _, sample in
                    LineMark(x: .value("Distance", sample.distM / 1000), y: .value("SoC", sample.soc * 100))
                        .foregroundStyle(.green)
                    AreaMark(x: .value("Distance", sample.distM / 1000), y: .value("SoC", sample.soc * 100))
                        .foregroundStyle(.green.opacity(0.15))
                }
                ForEach(ChargingStopVM.stops(from: plan)) { stop in
                    RuleMark(x: .value("Stop", stop.distFromStartM / 1000))
                        .foregroundStyle(.orange)
                        .lineStyle(StrokeStyle(lineWidth: 1.5, dash: [4, 3]))
                }
            }
            if let sel = pinnedDistanceM ?? store.selectedDistanceM {
                RuleMark(x: .value("Selected", sel / 1000))
                    .foregroundStyle(.white)
                    .lineStyle(StrokeStyle(lineWidth: 2))
            }
        }
        .chartYScale(domain: 0...100)
        .chartXAxis(showAxes ? .automatic : .hidden)
        .chartYAxis(showAxes ? .automatic : .hidden)
        .chartOverlay { proxy in
            GeometryReader { geo in
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
                                    let plotFrame = geo[proxy.plotAreaFrame]
                                    let xPos = value.location.x - plotFrame.origin.x
                                    guard let distKm: Double = proxy.value(atX: xPos) else { return }
                                    let maxKm = (store.displayedPlan?.totalDistM ?? 0) / 1000
                                    store.selectedDistanceM = min(max(distKm, 0), maxKm) * 1000
                                }
                        )
                }
            }
        }
        .frame(height: height)
        .accessibilityIdentifier("soc-chart")
    }
}
