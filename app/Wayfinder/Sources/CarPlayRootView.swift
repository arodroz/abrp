// CarPlay display surface (wayfinder #70): mirrors PlanStore/DriveStore's already-observable
// state onto the car's screen -- no state of its own, no independent map/route data path, and no
// controls (see CarPlaySceneDelegate.swift's header for why: the phone stays the only control
// surface in this v1). CarPlayMapView owns a SEPARATE MLNMapView from PlanStore's own (CarPlay
// renders to its own screen/window), kept in sync by copying styleURL, route layers, and the
// drive puck across on every observed change -- see its Coordinator/updateUIView comments for
// the exact sync rules.
import CoreLocation
import MapLibre
import SwiftUI
import os

struct CarPlayRootView: View {
    let planStore: PlanStore
    let driveStore: DriveStore

    var body: some View {
        ZStack {
            CarPlayMapView(planStore: planStore, driveStore: driveStore)
                .ignoresSafeArea()

            VStack {
                // Same nil-gate as RootView's driveControlsOverlay: no upcoming step (v1 pack)
                // or guidance muted (off-route-but-unreplanned, mid-replan).
                if let banner = driveStore.banner {
                    ManeuverBannerView(banner: banner)
                        .padding(16)
                }
                Spacer()
                HStack {
                    if driveStore.phase != .idle, let hud = driveStore.hud {
                        hudStrip(hud)
                    }
                    Spacer()
                }
                .padding(16)
            }

            if planStore.displayedPlan == nil {
                Text("Plan a route on your iPhone")
                    .font(.headline)
                    .padding(.horizontal, 20)
                    .padding(.vertical, 12)
                    .background(.regularMaterial, in: Capsule())
            }
        }
    }

    /// Bottom-leading HUD strip: the same ETA/remaining-distance/next-stop values as DriveCard's
    /// summary line, minus the tap-to-correct-SoC button and the expandable SoC chart -- both are
    /// phone-only controls, and this surface is display-only. Visual idiom matches
    /// ManeuverBannerView (regularMaterial, rounded 16, shadow).
    private func hudStrip(_ hud: DriveStore.DriveHud) -> some View {
        HStack(spacing: 12) {
            Text(formatClock(hud.etaDate))
                .font(.title2).bold()
            Text(
                "\(StepFormatter.formatDistance(hud.remainingDistM)) \u{00B7} \(hud.nextLabel) arrive \(formatSocPct(hud.nextArrivalSoc))"
            )
            .font(.subheadline)
            .foregroundColor(.secondary)
            .lineLimit(1)
        }
        .padding(16)
        .background(.regularMaterial)
        .clipShape(RoundedRectangle(cornerRadius: 16, style: .continuous))
        .shadow(color: .black.opacity(0.2), radius: 10, y: 4)
    }

    /// "HH:mm", identical to DriveCard's own `formatClock`.
    private func formatClock(_ date: Date) -> String {
        let formatter = DateFormatter()
        formatter.dateFormat = "HH:mm"
        return formatter.string(from: date)
    }
}

/// CarPlay's own map view (wayfinder #70). Deliberately a second `MLNMapView` instance, not
/// `planStore.mapView` reused across two screens -- CarPlay renders to a separate `CPWindow` on
/// the car's display, and MapLibre's `MLNMapView` (like any `UIView`) can only live in one
/// window at a time. `updateUIView` re-reads PlanStore/DriveStore's observable properties on
/// every SwiftUI-triggered pass -- that IS the update mechanism (no Combine, no timers): style
/// URL and route layers are copied across, and a following camera + puck are driven while
/// driving.
struct CarPlayMapView: UIViewRepresentable {
    let planStore: PlanStore
    let driveStore: DriveStore

    func makeUIView(context: Context) -> MLNMapView {
        let mapView = MLNMapView(frame: .zero)
        mapView.delegate = context.coordinator
        // PlanStore's own map leaves MapLibre's default attribution/logo/compass chrome on --
        // the phone screen has room for it. CarPlay's doesn't, and Apple's map-app guidelines
        // ask for a chrome-free map here, so it's hidden on THIS map view only.
        mapView.attributionButton.isHidden = true
        mapView.logoView.isHidden = true
        mapView.compassView.isHidden = true
        mapView.showsUserLocation = false
        return mapView
    }

    func updateUIView(_ mapView: MLNMapView, context: Context) {
        // Reading every one of these here, rather than caching them elsewhere, is what makes
        // SwiftUI re-invoke this method on change -- the update mechanism this view relies on.
        // `styleVersion`/`mapStyleURL`, NOT `mapView.styleURL`: the phone map is a stored
        // object, so mutating its styleURL invalidates nothing -- an idle CarPlay connect would
        // keep the default world style forever once the pack finished loading (found live on
        // the sim, #70).
        let styleVersion = planStore.styleVersion
        let planVersion = planStore.planVersion
        let displayedPlan = planStore.displayedPlan
        let phase = driveStore.phase
        let snappedCoordinate = driveStore.snappedCoordinate
        let smoothedCourseDeg = driveStore.smoothedCourseDeg
        let coordinator = context.coordinator

        // (a) Style stays mirrored off PlanStore's applied style -- covers a pack finishing its
        // load after this scene has already connected, a light/dark swap, and a region switch.
        // Keyed on styleVersion, NOT URL equality: applyStyle rewrites ONE temp file, so the URL
        // compares equal across re-patches and an equality guard would never reload this map
        // (found live on the sim, #70: CarPlay stuck on the content it read first). Reassigning
        // the same URL forces MapLibre to reload, exactly like the phone map's own path. Before
        // the first applyStyle (no pack located yet) mapStyleURL is nil: leave the default style.
        if let styleURL = planStore.mapStyleURL, styleVersion != coordinator.lastRenderedStyleVersion {
            coordinator.styleLoaded = false
            mapView.styleURL = styleURL
            coordinator.lastRenderedStyleVersion = styleVersion
        }

        // (b) Route layers re-added only once the (possibly just-swapped) style has actually
        // finished loading, and only on a genuinely new plan. A nil `displayedPlan` just records
        // the version and leaves whatever's already drawn alone -- there's no empty FfiPlan to
        // hand RouteLayer.addLayers to clear it with, and nothing is ever drawn before the first
        // plan lands anyway.
        if coordinator.styleLoaded, planVersion != coordinator.lastRenderedPlanVersion {
            if let style = mapView.style, let displayedPlan {
                RouteLayer.addLayers(to: style, plan: displayedPlan)
                if phase == .idle {
                    RouteLayer.fitToRoute(mapView: mapView, plan: displayedPlan)
                }
            }
            coordinator.lastRenderedPlanVersion = planVersion
        }

        // (c) Drive puck + following camera, mirroring DriveStore's own applyFollowingCamera --
        // torn down the moment phase returns to .idle.
        if phase != .idle, let snappedCoordinate {
            if let puckAnnotation = coordinator.puckAnnotation {
                puckAnnotation.coordinate = snappedCoordinate
            } else {
                let annotation = MLNPointAnnotation()
                annotation.title = "drive-puck"
                annotation.coordinate = snappedCoordinate
                mapView.addAnnotation(annotation)
                coordinator.puckAnnotation = annotation
            }
            let camera = MLNMapCamera(
                lookingAtCenter: snappedCoordinate, altitude: 800, pitch: 45, heading: smoothedCourseDeg
            )
            mapView.setCamera(camera, withDuration: 0.8, animationTimingFunction: CAMediaTimingFunction(name: .linear))
        } else if let puckAnnotation = coordinator.puckAnnotation {
            mapView.removeAnnotation(puckAnnotation)
            coordinator.puckAnnotation = nil
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(planStore: planStore, driveStore: driveStore)
    }

    // Swift 6 strict concurrency (same justification as PlanStore.swift/DriveStore.swift's own
    // `@preconcurrency` conformances): MLNMapView calls its delegate on main, and this Coordinator
    // is only ever touched from `updateUIView`, itself main-actor-bound.
    @MainActor
    final class Coordinator: NSObject, @preconcurrency MLNMapViewDelegate {
        var lastRenderedPlanVersion: Int?
        /// 0 = nothing applied yet; PlanStore.styleVersion starts at 0 and bumps to 1 on its
        /// first applyStyle, so the first comparison always adopts a located pack's style.
        var lastRenderedStyleVersion = 0
        var styleLoaded = false
        var puckAnnotation: MLNPointAnnotation?

        private let planStore: PlanStore
        private let driveStore: DriveStore

        init(planStore: PlanStore, driveStore: DriveStore) {
            self.planStore = planStore
            self.driveStore = driveStore
        }

        func mapView(_ mapView: MLNMapView, didFinishLoading style: MLNStyle) {
            styleLoaded = true
            // A style load/reload wipes every layer/source, and nothing observable changes when
            // it finishes -- Coordinator state can't trigger a SwiftUI update pass, so waiting
            // for the next updateUIView would leave an idle connect (plan already on the phone
            // screen) showing a bare map until some store property happens to change. Draw here
            // directly instead, the same way PlanStore's own didFinishLoading re-adds its layers.
            if let plan = planStore.displayedPlan {
                RouteLayer.addLayers(to: style, plan: plan)
                if driveStore.phase == .idle {
                    RouteLayer.fitToRoute(mapView: mapView, plan: plan)
                }
            } else {
                // No plan to frame: adopt the phone map's current camera instead of stranding
                // the car's screen at the style's default world-at-z0 view.
                mapView.setCamera(planStore.mapView.camera, animated: false)
            }
            lastRenderedPlanVersion = planStore.planVersion
        }

        /// Same "drive-puck" title convention as PlanStore.mapView(_:imageFor:) and
        /// DriveStore.addPuckIfNeeded -- PlanStore.drivePuckImage is the identical shared image,
        /// not a redrawn copy.
        func mapView(_ mapView: MLNMapView, imageFor annotation: MLNAnnotation) -> MLNAnnotationImage? {
            guard annotation.title ?? "" == "drive-puck" else { return nil }
            return PlanStore.drivePuckImage
        }

        /// The car's screen has no toast/error surface, so a style that fails to load would
        /// otherwise just sit there blank and unexplained -- log it where `log show` can see it.
        func mapViewDidFailLoadingMap(_ mapView: MLNMapView, withError error: Error) {
            Logger(subsystem: "org.anteras.wayfinder", category: "carplay")
                .error("CarPlay map style failed to load: \(String(describing: error), privacy: .public)")
        }
    }
}
