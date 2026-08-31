# UI end-to-end tests

Run the `WayfinderUITests` target against the booted iPhone 17 Pro simulator (its Documents container already has the `corridor`/`eu-west` packs installed, which the suite needs). Pre-grant location once per run (a fresh install can otherwise show a system permission prompt the suite doesn't handle), then run the suite:

```
xcrun simctl privacy <SIMULATOR_UUID> grant location org.anteras.wayfinder
xcodebuild test -project app/Wayfinder/Wayfinder.xcodeproj -scheme Wayfinder \
  -destination 'platform=iOS Simulator,id=<SIMULATOR_UUID>' \
  -only-testing:WayfinderUITests ARCHS=arm64 CODE_SIGNING_ALLOWED=NO
```

Regenerate the Xcode project first with `xcodegen -s app/Wayfinder/project.yml` if `project.yml` or the `UITests/` sources changed. Determinism comes from two launch arguments `WayfinderUITestCase.launchApp()` passes on every run: `-activeRegion corridor` and `-recentDestinations <json>`, seeding `RouteEditorView`'s recents list so tests reach destinations through the seeded recents rows instead of the live `MKLocalSearchCompleter` path (which isn't scriptable in a sim and would make plans non-reproducible). The recents JSON is wrapped in a legacy-property-list quoted-string literal before being passed as a launch argument -- `NSUserDefaults`'s command-line argument parser silently drops a bare JSON array/object value instead of falling back to a plain string, which was confirmed empirically (`xcrun simctl launch --console-pty` showed the key coming back `nil`) before that fix.

`DriveFlowTests` additionally passes `-simulatedLocationFix "lat,lon"` (via `launchApp(extraLaunchArguments:)`), adopted at load as the current-location origin, because XCUITest cannot inject CoreLocation fixes.

**The test run replaces the installed app with an unsigned build.** `xcodebuild test` rebuilds the app target under `CODE_SIGNING_ALLOWED=NO` and installs that build on the simulator — harmless for the tests themselves, but the unsigned binary is rejected by `nsurlsessiond` ("does not have a bundle ID"), so every background download fails with NSURLError -1 afterwards and `--autotest install-smoke` reports a false failure. Re-signing that product by hand (`codesign -f -s - --deep`) is not reliably sufficient. After any UI-test run, rebuild with default signing (`xcodebuild build` with no signing overrides) and `simctl install` that app before running the store-driven autotest modes.

Every `XCTExpectFailure` block in the suite is a tracked app bug, not a weakened assertion: the test still exercises the real UI path and records the exact symptom/repro/suspected cause as the expected-failure message, so the suite stays green (or amber, precisely) while surfacing what's actually broken. Treat a new `XCTExpectFailure` appearing in a run as a regression report, and a previously-`XCTExpectFailure`'d assertion suddenly passing as a fix worth removing the wrapper for.

## CarPlay on the simulator (wayfinder #70)

The CarPlay scene runs on the simulator because sim builds don't validate entitlements against a provisioning profile — `project.yml` applies `Wayfinder.entitlements` (`com.apple.developer.carplay-maps`) via `CODE_SIGN_ENTITLEMENTS[sdk=iphonesimulator*]` only, so device signing keeps working while Apple's grant (#71) is pending.

Verifying it is a UI-scripting exercise; the recipe that actually works:

- **Attach the display**: Simulator menu **I/O → External Displays → CarPlay**, scriptable via `osascript` menu clicking (Terminal needs Accessibility). **Never attach during boot**: a display restored/attached while the device is still booting comes up as a black, input-dead framebuffer that survives reboots and `backboardd` kills. Cure: close the CarPlay *window* (its close button, not the menu item — the menu checkmark goes stale), confirm `simctl io booted screenshot --display external` fails ("Timeout waiting for screen surfaces" = truly detached), then re-attach via the menu on the settled device.
- **Screenshot it**: `xcrun simctl io booted screenshot --display external out.png` (device-side; works headless).
- **Click in it**: AppleScript `click at` is delivered mid-cursor-flight and misses small targets; CGEvent posting from ad-hoc binaries lacks the Accessibility grant and silently no-ops. Use `cliclick m:X,Y w:300 c:X,Y` (move, settle, click). Map content coordinates through the window's AXGroup: it reports position/size for the 800×480 screen (scale = w/800), queried fresh per click — the window drifts.
- **A tapped icon that bounces back to the previous app + a `CarPlayTemplateUIHost` crash log** means the system host died, not the app. The iOS 26.4 runtime's host crashes on any root `CPMapTemplate` (`-[CPSTemplateInstance vehicleSupportsDestinationSharing]: unrecognized selector`), which is why `CarPlaySceneDelegate` sets no root template. After that crash the dashboard can wedge (frozen clock) — recover with the detach/re-attach cure above.

Sim screenshots proving the surface (home icon, mid-drive banner + HUD, idle pack map): `docs/research/carplay-sim/`.
