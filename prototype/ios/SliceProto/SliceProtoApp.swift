// Prototype: three planner-UI variants (wayfinder #23), switchable via floating pill.
// Throwaway branch prototype/planner-ui.

import SwiftUI

enum PlannerVariant: String, CaseIterable {
    case sheetStack = "A"
    case formFirst = "B"
    case drivingOverlay = "C"
    case google = "D"

    var label: String {
        switch self {
        case .sheetStack: return "Sheet stack"
        case .formFirst: return "Form-first"
        case .drivingOverlay: return "Driving overlay"
        case .google: return "Google"
        }
    }
}

struct RootView: View {
    @StateObject private var store = PlanStore()
    @AppStorage("plannerUIVariant") private var variantRaw: String = PlannerVariant.google.rawValue

    private var variant: PlannerVariant { PlannerVariant(rawValue: variantRaw) ?? .google }

    var body: some View {
        ZStack {
            if !store.missingPaths.isEmpty {
                missingView
            } else {
                Group {
                    switch variant {
                    case .sheetStack: VariantASheetStack(store: store)
                    case .formFirst: VariantBFormFirst(store: store)
                    case .drivingOverlay: VariantCDrivingOverlay(store: store)
                    case .google: VariantDGoogle(store: store)
                    }
                }
            }

            // Above everything else in the app, including sheets/panels raised by a variant.
            VariantSwitcherPill(variantRaw: $variantRaw)
                .zIndex(999)
        }
    }

    private var missingView: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("PROTO ERROR: missing files")
                .font(.headline)
                .foregroundColor(.white)
            ForEach(store.missingPaths, id: \.self) { p in
                Text(p)
                    .font(.system(.body, design: .monospaced))
                    .foregroundColor(.white)
            }
        }
        .padding()
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(Color.black)
    }
}

/// Fixed bottom-center high-contrast pill, deliberately not part of any variant's design
/// language, so it always reads as prototype scaffolding. Cycles with wraparound and
/// persists the chosen variant across relaunch via @AppStorage.
struct VariantSwitcherPill: View {
    @Binding var variantRaw: String

    private var variant: PlannerVariant { PlannerVariant(rawValue: variantRaw) ?? .google }
    private var all: [PlannerVariant] { PlannerVariant.allCases }

    var body: some View {
        VStack {
            Spacer()
            HStack(spacing: 12) {
                Button(action: cyclePrev) {
                    Text("\u{25C0}").bold()
                }
                Text("\(variant.rawValue) \u{00B7} \(variant.label)")
                    .font(.system(.footnote, design: .monospaced))
                    .bold()
                Button(action: cycleNext) {
                    Text("\u{25B6}").bold()
                }
            }
            .foregroundColor(.white)
            .padding(.horizontal, 16)
            .padding(.vertical, 10)
            .background(Color.black)
            .clipShape(Capsule())
            .overlay(Capsule().stroke(Color.yellow, lineWidth: 2))
            .padding(.bottom, 24)
        }
    }

    private func cyclePrev() {
        let idx = all.firstIndex(of: variant) ?? 0
        variantRaw = all[(idx - 1 + all.count) % all.count].rawValue
    }

    private func cycleNext() {
        let idx = all.firstIndex(of: variant) ?? 0
        variantRaw = all[(idx + 1) % all.count].rawValue
    }
}

@main
struct SliceProtoApp: App {
    var body: some Scene {
        WindowGroup {
            RootView()
        }
    }
}
