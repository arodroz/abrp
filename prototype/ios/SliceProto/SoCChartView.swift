// SoC Curve chart (see CONTEXT.md): SoC% over distance, Charging Stops as rule marks,
// draggable scrub that publishes the selected distance to PlanStore.

import Charts
import SwiftUI

struct SoCChartView: View {
    @ObservedObject var store: PlanStore
    var height: CGFloat = 220
    var showAxes: Bool = true

    private struct Sample: Identifiable {
        let id: Int
        let distKm: Double
        let socPct: Double
    }

    private var samples: [Sample] {
        guard let plan = store.plan else { return [] }
        return plan.socCurve.enumerated().map { idx, s in
            Sample(id: idx, distKm: s.distM / 1000, socPct: s.soc * 100)
        }
    }

    var body: some View {
        Chart {
            ForEach(samples) { s in
                LineMark(x: .value("Distance", s.distKm), y: .value("SoC", s.socPct))
                    .foregroundStyle(.green)
                AreaMark(x: .value("Distance", s.distKm), y: .value("SoC", s.socPct))
                    .foregroundStyle(.green.opacity(0.15))
            }
            if let plan = store.plan {
                ForEach(plan.stops) { stop in
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
                                let maxKm = (store.plan?.totalDistM ?? 0) / 1000
                                store.selectedDistanceM = min(max(distKm, 0), maxKm) * 1000
                            }
                    )
            }
        }
        .frame(height: height)
    }
}
