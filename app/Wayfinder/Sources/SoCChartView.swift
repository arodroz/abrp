// SoC-over-distance chart (wayfinder #43): Line+AreaMark over the SoC curve, an orange dashed
// RuleMark per Charging Stop, and a draggable scrub overlay that publishes the selected
// distance to PlanStore (which moves a marker on the map -- see PlanStore.updateScrubMarker).
// Ported nearly verbatim from prototype/planner-ui's SoCChartView.swift, re-typed off
// FfiPlan/FfiSocPoint and PlanStore's `displayedPlan` (the main plan or, toggled, the
// stop-free alternative).
import Charts
import PlannerKit
import SwiftUI

struct SoCChartView: View {
    let store: PlanStore
    var height: CGFloat = 220
    var showAxes: Bool = true

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
            if let sel = store.selectedDistanceM {
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
        .frame(height: height)
    }
}
