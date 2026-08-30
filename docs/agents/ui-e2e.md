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
