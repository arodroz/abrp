// CarPlay scene delegate (wayfinder #70): display-only v1 -- the phone stays the ONLY control
// surface (route entry, Go/End, settings, SoC correction all live there), and this scene just
// mirrors PlanStore/DriveStore's already-observable state onto the car's screen via
// CarPlayRootView. No CPTemplate-driven interaction here on purpose: a turn list or a
// destination-search template would duplicate state PlanStore already owns and race the phone
// UI for control of it -- a later ticket can add read/limited-write templates once that's
// designed, not bolted on here. The entitlement that makes this scene loadable at all is
// simulator-gated (project.yml's CODE_SIGN_ENTITLEMENTS[sdk=iphonesimulator*], wayfinder #71):
// Apple hasn't granted com.apple.developer.carplay-maps to this team yet.
//
// UIKit instantiates this class by name (Info.plist's UISceneDelegateClassName, project.yml),
// so it can't be handed PlanStore/DriveStore through any initializer -- WayfinderApp.init stashes
// them into the static vars below once, before this scene can possibly connect.
import CarPlay
import SwiftUI
import UIKit

@MainActor
final class CarPlaySceneDelegate: UIResponder, CPTemplateApplicationSceneDelegate {
    static var planStore: PlanStore?
    static var driveStore: DriveStore?
    static var telemetryStore: TelemetryLinkStore?

    private var interfaceController: CPInterfaceController?

    func templateApplicationScene(
        _ templateApplicationScene: CPTemplateApplicationScene,
        didConnect interfaceController: CPInterfaceController,
        to window: CPWindow
    ) {
        self.interfaceController = interfaceController
        guard let planStore = Self.planStore, let driveStore = Self.driveStore, let telemetryStore = Self.telemetryStore else { return }

        // Cold launch from the CarPlay home screen (phone locked/in pocket): RootView.onAppear
        // -- the normal load() trigger -- may never fire, leaving this surface on MapLibre's
        // default style with no packs. load() is hasStartedLoad-guarded, so the common
        // phone-first path makes this a no-op.
        planStore.load()

        window.rootViewController = UIHostingController(
            rootView: CarPlayRootView(planStore: planStore, driveStore: driveStore, telemetryStore: telemetryStore)
        )

        // Deliberately NO root CPMapTemplate in this display-only v1. Everything guidance shows
        // lives in the CPWindow's own view hierarchy above (ManeuverBannerView + HUD strip), so
        // the template would contribute zero visible chrome -- and setting one crashes the iOS
        // 26.4 SIMULATOR runtime's CarPlayTemplateUIHost outright: its
        // CPSMapTemplateViewController._configureNavigationBarShareButton probes
        // -[CPSTemplateInstance vehicleSupportsDestinationSharing], unimplemented in the same
        // runtime's CarPlaySupport (doesNotRecognizeSelector -> abort; crash log
        // CarPlayTemplateUIHost-2026-08-31-231648.ips). Revisit when real-car CarPlay lands via
        // wayfinder #71 -- a later ticket can add CPMapTemplate/CPNavigationSession for cluster
        // integration on hardware that isn't this simulator host.
    }

    func templateApplicationScene(
        _ templateApplicationScene: CPTemplateApplicationScene,
        didDisconnectInterfaceController interfaceController: CPInterfaceController
    ) {
        self.interfaceController = nil
    }
}
