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

## Replaying a Trip Log (wayfinder #87)

Feeds a recorded Trip Log into Drive Mode at the desk (real timing, scaled), so the Follow Camera and other drive-time work can be judged against a real recorded drive instead of the synthetic `-simulatedDriveDistancesM` fix train. Two launch arguments, read the same way as `-simulatedLocationFix`:

- `-replayTripLog <path>`: an absolute path (Simulator can read Mac paths) or a bare filename resolved inside the app's `Documents/trip-logs/` directory (where real logs pulled off the phone already live).
- `-replaySpeed <double>`: time multiplier, default 10.

`PlanStore.load()` decodes the log, adopts its first sample as the origin (same `adoptLocationFixAsOriginIfEligible` path a real location fix uses), and sets its last sample as the destination once the planner is ready. Example, from the repo root, using the untracked 2026-09-02 corridor drive:

```
xcrun simctl launch <SIMULATOR_UUID> org.anteras.wayfinder \
  -replayTripLog "$(pwd)/.trip-logs/tlog-1788327781-DA35087B.json" -replaySpeed 30
```

On the phone, pass just the filename (`-replayTripLog tlog-1788327781-DA35087B.json`) — it resolves inside the on-device `Documents/trip-logs/`.

Once the plan lands, tap **Go** and confirm the start SoC as usual; `DriveStore.enterDrive` then spawns a Task that walks the log's samples in order, building a `CLLocation` per sample (coordinate, `alt_m`, `hacc_m`, `speed_mps`, course from the bearing between consecutive samples) and feeding it to `ingest`, sleeping between samples for the real recorded interval divided by `-replaySpeed`. It prints a start line (sample count + speed) and an end line to the console.

Caveats:

- **The destination must fall inside the installed pack**, or planning fails ("No route found — outside pack region?"). The 2026-09-02 log's last sample (48.547, 4.190) is well outside the `corridor`/`lu-dev` packs — only its first ~half (up to t≈7413s, index 6741, 49.400/5.627) is. Use a log (or a trimmed copy) whose last sample lands in the active pack, or install a wider pack (`eu-west`).
- **The replayed trace was a real, complex drive, not the straight route the planner computes between its first/last sample** — expect off-route replans (ADR 0012 point 6) during a long replay, same as any real drive that deviates from its plan.
- **No Trip Log is recorded for a replayed drive**: `TripLogStore` reads `-replayTripLog` at init and skips its save on End/arrival, so replays never pollute `Documents/trip-logs/` with synthetic calibration data.
- There is no UI-injection path for tapping Go on the Simulator (see above); `--autotest replay-demo` supplies it for automated/console verification — it plans and drives exactly like a real replay, only substituting the Go tap.

## 3D Drive Mode prototype (wayfinder #90)

**Throwaway.** Everything lives in `app/Wayfinder/Sources/Proto3D.swift` plus small gated hooks in `PlanStore.swift` and `DriveStore.swift`; with no `-proto*` argument the app behaves exactly as it did before. Three independent gates:

| Argument | Default | What it does |
|---|---|---|
| `-proto3d 1` | off | `MLNFillExtrusionStyleLayer` on `protomaps`/`buildings` (`kind IN {building, building_part}`), inserted below the route ribbon, plus an `MLNLight` (viewport anchor, position 1.15/210/40, intensity 0.35) and `tileLodPitchThreshold`. Visible only while `phase == .driving` and the camera is following/free-look; the flat `buildings` fill is hidden while it is |
| `-protoOpacity <0-1>` | `0.75` | `fillExtrusionOpacity` (1.0 drops MapLibre's depth pre-pass — the first knob to try if fps misses) |
| `-protoMinzoom <z>` | `15` | layer `minimumZoomLevel`; height/base fade in from `z` to `z + 0.5` |
| `-protoCamera 1` | off | the re-tuned Follow Camera (pitch, `contentInset` framing, look-ahead zoom, per-fix animation) instead of the shipped `altitude: 800, pitch: 45` over 0.8 s |
| `-protoPitch <deg>` | `60` | Follow Camera pitch |
| `-protoZoomMin <z>` / `-protoZoomMax <z>` | `14.5` / `17` | look-ahead 1500 m maps to `protoZoomMin`, 200 m to `protoZoomMax`, linear between; zoom moves at most 0.1 levels/s and freezes below 7 km/h |
| `-protoLookAheadS <s>` | `32` | look-ahead seconds: `lookAhead = clamp(speed × s, 200 m, 1500 m)`, pulled in to `dTurn + 100 m` when a manoeuvre is upcoming |
| `-protoFps 1` | off | counts `mapViewDidFinishRenderingFrame` and logs one `os_log` line every 5 s at **default** level, subsystem `org.anteras.wayfinder`, category `proto.fps`: average fps, min frame interval, zoom, pitch, extrusions on/off |
| `-protoLod <deg>` | `30` | `tileLodPitchThreshold`, in degrees (pass `60` to disable variable tile LOD again) — diagnostic knob |
| `-protoInset <0/1>` | `1` | `0` drops the `contentInset` framing and centres the vehicle like the shipped camera — diagnostic knob |

### Simulator

Combines the replay harness above with the three gates (pack-bearing iPhone 17 Pro `C95993C6-C86A-4FC8-A7CE-82FB03C0B62C`):

```
xcrun simctl privacy C95993C6-C86A-4FC8-A7CE-82FB03C0B62C grant location org.anteras.wayfinder
xcrun simctl launch --console-pty C95993C6-C86A-4FC8-A7CE-82FB03C0B62C org.anteras.wayfinder \
  --autotest replay-demo -activeRegion corridor \
  -replayTripLog "$(pwd)/.trip-logs/<log>.json" -replaySpeed 20 \
  -proto3d 1 -protoCamera 1 -protoFps 1
```

`--console-pty` shows only `print()` output. The fps lines are `os_log`, so stream them separately:

```
xcrun simctl spawn C95993C6-C86A-4FC8-A7CE-82FB03C0B62C log stream \
  --predicate 'subsystem == "org.anteras.wayfinder" AND category == "proto.fps"' --style compact
```

**Pick a log whose track has basemap tiles, not just a routable position.** The `corridor` pack's PMTiles bbox is 2.51/49.44 → 7.09/53.51 and its actual coverage is a corridor inside that box: every Trip Log in `.trip-logs/` was recorded around Longwy (49.48/5.75), where the pack has **no** z14 tiles at all, so a replay of one renders a blank basemap (with or without the prototype — check against a no-flag run before blaming the gates). Luxembourg City, Arlon and anything on the LU→NL corridor do have tiles.

### Phone

Arguments go after the bundle identifier, but `devicectl`'s own parser reads a leading `-p…` as a bundle of short flags (`-t` is its `--timeout`), so separate them with `--`:

```
xcrun devicectl device process launch --console --terminate-existing \
  --device ED47BA12-C341-5363-AEFE-C20015477C96 org.anteras.wayfinder \
  -- -replayTripLog <log>.json -replaySpeed 20 -proto3d 1 -protoCamera 1 -protoFps 1
```

On the phone `-replayTripLog` takes a bare filename inside `Documents/trip-logs/`. `--console` streams os_log at default level, so the `proto.fps` lines arrive in that same output — which is why they are logged at default level and not `.debug`. The phone is the only place the fps numbers mean anything; the Simulator renders on the Mac's GPU and sits at ~60 fps regardless.
