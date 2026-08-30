# Drive-flow UX: search → Go → active guidance in Google Maps and Apple Maps

Research for issue #57 (part of #28, blocking #55). Reconstructs how the two reference apps take a route from search to active guidance, where the boundary lies between full turn-by-turn and bare route-following, and how the EV layer is surfaced during a drive — sharp enough to design a minimal Wayfinder drive mode. Every load-bearing claim carries the URL it was read from; facts checked 2026-08-30. Claims that could not be pinned to a trustworthy source are marked **UNVERIFIED** (collected in §8).

Terms follow `CONTEXT.md`: *Plan*, *Leg*, *Charging Stop*, *Waypoint*, *Charger*, *SoC*. Reference-app terminology ("route", "stop") is kept verbatim when describing those apps.

## 0. Summary / decision facts

- Both reference apps use the **same five-state skeleton**: browse/search → place selected → route preview → active guidance → arrival, with the bottom sheet transforming at each step and the Go/Start button living on the preview card ([Apple guide](https://support.apple.com/en-gb/guide/iphone/ipha84a94043/ios), [Google support](https://support.google.com/maps/answer/144339?hl=en&co=GENIE.Platform%3DAndroid)).
- Both gate guidance on the origin being the current location: Google offers only **Preview** otherwise ([support 144339](https://support.google.com/maps/answer/144339?hl=en&co=GENIE.Platform%3DAndroid)); Apple swaps **Go** for **Steps** — a static turn list with no spoken guidance ([Apple guide](https://support.apple.com/en-gb/guide/iphone/iphd3c85c193/ios)).
- Both treat **off-route reroute as automatic and silent** and **better-route as a prompt** the driver accepts (Google: [Navigator reference](https://developers.google.com/maps/documentation/navigation/android-sdk/reference/com/google/android/libraries/navigation/Navigator), [autoevolution](https://www.autoevolution.com/news/google-maps-starts-showing-confusing-route-popups-on-android-and-android-auto-254999.html); Apple: community-corroborated only, see §8).
- Google's own Navigation SDK proves the during-drive HUD is **not a monolith**: maneuver header, ETA footer, trip-progress bar, speedometer, camera mode, and voice are each independently toggleable components ([iOS controls doc](https://developers.google.com/maps/documentation/navigation/ios-sdk/controls)). Every SDK examined (Mapbox, Google, HERE, TomTom) treats banners/lanes/voice as *consumers* of a core progress engine, never producers of state the core needs — so route-following without turn-by-turn is an established architecture, not a hack (§5).
- The recurring in-drive EV pattern across Apple, Google built-in, Tesla, and ABRP: **one continuously re-estimated number dominates — battery-%-at-arrival** — with Charging Stops living in the route card/directions list (arrival SoC + charge time each), and check-off driven by energy/position, not by tapping (§6, [Google help](https://support.google.com/maps/answer/9773205?hl=en), [Ford doc](https://www.ford.ca/support/how-tos/electric-vehicles/other-electric-vehicle-information/how-do-i-use-apple-maps-ev-routing/)).
- The minimal viable drive-mode element set (§5.4, grounded in what the SDKs ship as separable): snap-to-route + puck, heading-up following camera with a free-look/re-center escape, progress + ETA from remaining geometry, off-route detection decoupled from replan (hand off to our own planner — Charging Stops must be re-solved), and per-stop arrival detection replacing maneuver-step tracking as the only stepper.

## 1. Google Maps: state machine and UI per state

### 1.1 State machine

```
[Browse/Search]
   │ search or tap place on map
   ▼
[Place selected] ── tap "Directions" (bottom-left of place sheet)
   ▼
[Route preview / directions view]   best route blue, alternates gray
   │  • tap gray route → becomes the selected (blue) route
   │  • "More" → Edit stops (≤9), depart/arrive time, Options (avoid tolls/highways)
   │  • swipe up route info → steps list; "Preview" → simulated turn-by-turn (no GPS lock)
   │
   ├─ tap "Start"  (offered only when current location == route origin;
   │                otherwise only "Preview")
   ▼
[Active guidance / following camera]
   │  • user pans map ──────────────► [Free-look] ── tap "Re-center" ──► following
   │  • magnifier (top-right) ─────► [Search along route] → stop added, guidance continues
   │  • faster/slower route found ─► popup with alternate + ETA delta; tap to accept
   │  • go off route ──────────────► automatic silent reroute (no visible state change)
   │  • swipe card up → "Exit" ────► [Ended, plain map]
   │  • reach waypoint ────────────► arrival card for stop → resume to next destination
   ▼
[Arrival] — arrival message replaces the routing card, then dismisses to plain map
   (Google's Android-for-Cars template spec: message state ≥8 s, then auto-transition)
```

Transition sources: Start-vs-Preview gating — [support 144339](https://support.google.com/maps/answer/144339?hl=en&co=GENIE.Platform%3DAndroid) ("To start navigation, your starting point must be your location. Otherwise, you get a preview"). Exit via drag-up — [iOS support 3273406](https://support.google.com/maps/answer/3273406?hl=en&co=GENIE.Platform%3DiOS). Add-stop-during-nav via magnifier — [TechCrunch 2015](https://techcrunch.com/2015/10/20/google-maps-now-lets-you-add-a-stop-along-your-route-check-gas-prices), [Gadget Hacks](https://smartphones.gadgethacks.com/how-to/google-maps-101-add-stop-after-youve-started-navigation-0179200/). Faster-route popup — [autoevolution](https://www.autoevolution.com/news/google-maps-starts-showing-confusing-route-popups-on-android-and-android-auto-254999.html). Arrival auto-transition — [Arrive at a destination (Android for Cars)](https://developers.google.com/cars/design/create-apps/sample-flows/arrive-at-destination).

### 1.2 Route preview

Per [support 144339](https://support.google.com/maps/answer/144339?hl=en&co=GENIE.Platform%3DAndroid):

- Mode row: Driving / Transit / Walking / Ride / Cycle / Motorcycle.
- "The best route to your destination is blue. All other routes are gray." Alternates are selected by tapping them on the map; each route shows its estimated travel time as a floating callout on the polyline. (In the Navigation SDK the callout content is a formatting option, `routeCalloutFormat`: default/time/distance — [camera/controls docs](https://developers.google.com/maps/documentation/navigation/ios-sdk/controls).)
- Bottom card: route summary; swipe up → full step list; "Preview" → simulated turn-by-turn; "More" menu → **Edit stops** (up to 9 destinations, drag-handle reorder), **Set depart or arrive time**, **Options** → avoid tolls/highways (SDK adds ferries — [route-preferences](https://developers.google.com/maps/documentation/navigation/ios-sdk/route-preferences)).
- **Start** is the primary pill at the bottom of the card (exact color/placement UNVERIFIED from a Google source, consistent across all screenshots in secondary coverage).
- March 2025 Android redesign of this screen: travel time rendered largest, distance demoted to small text, ETA shown *before* Start (previously ETA appeared only after) — [9to5Google](https://9to5google.com/2025/03/31/google-maps-directions-redesign/), [Android Central](https://www.androidcentral.com/apps-software/google-maps/google-maps-is-giving-your-eta-screen-a-glow-up-on-android). A signal of what Google considers primary at preview time: time, not distance.

### 1.3 During-drive HUD

- **Top maneuver banner** ("header" in the SDK): next-turn arrow + distance + street name, secondary instruction row, optional accessory view — [iOS controls](https://developers.google.com/maps/documentation/navigation/ios-sdk/controls).
- **Lane guidance**: per-lane arrows in the banner at multi-lane junctions, recommended lanes highlighted — [9to5Google guide](https://9to5google.com/guides/lane-guidance/), [Android Police](https://www.androidpolice.com/google-maps-android-auto-update-makes-lane-guidance-better/).
- **Bottom ETA bar** ("footer"): time remaining / ETA / distance at the bottom of the map — [iOS controls](https://developers.google.com/maps/documentation/navigation/ios-sdk/controls). Exact left/center/right ordering in the consumer app UNVERIFIED from a primary source; time-remaining is the visual primary. Swiping the card up reveals: **Exit**, Share trip progress, Search along route, Preview Route (= overview), Directions (step list), Settings — [iOS support 3273406](https://support.google.com/maps/answer/3273406?hl=en&co=GENIE.Platform%3DiOS).
- **Exit**: "Drag up from the bottom and tap Exit" — [iOS support 3273406](https://support.google.com/maps/answer/3273406?hl=en&co=GENIE.Platform%3DiOS). Navigation itself is never swipe-dismissed.
- **Overview toggle**: "Preview Route" in the swiped-up card; SDK *Overview camera mode* "displays the entire route, north-up top-down, auto-zoomed to fit" — [camera doc](https://developers.google.com/maps/documentation/navigation/ios-sdk/camera).
- **Camera**: default *Following* = "45-degree view angle, camera behind current position facing direction of travel" (tilted, heading-up). Panning enters *Free* mode; a **Re-center** button "appears when the user scrolls the map view"; Google's sample auto-returns Free→Following after ~5 s idle — [camera doc](https://developers.google.com/maps/documentation/navigation/ios-sdk/camera).
- **Audio**: sound icon cycles three states — Mute / Alerts only / Sound — [iOS support 3273406](https://support.google.com/maps/answer/3273406?hl=en&co=GENIE.Platform%3DiOS); identical trichotomy in the SDK (`GMSNavigationVoiceGuidance`: silent / alertsOnly / alertsAndGuidance — [reference](https://developers.google.com/maps/documentation/navigation/ios-sdk/reference/objc/Enums/GMSNavigationVoiceGuidance)).
- **Speedometer / speed limit**: opt-in via Settings → Navigation; speed-limit sign where available; tapping it toggles the speedometer; SDK speedometer sits in a lower corner and changes to alert coloring when exceeding the limit — [support 9356324](https://support.google.com/maps/answer/9356324?hl=en&co=GENIE.Platform%3DiOS), [iOS controls](https://developers.google.com/maps/documentation/navigation/ios-sdk/controls).
- **Reroute (off route)**: "if a driver misses a turn, the Nav SDK automatically recalculates a new route" — automatic, no confirmation — [Navigator reference](https://developers.google.com/maps/documentation/navigation/android-sdk/reference/com/google/android/libraries/navigation/Navigator).
- **Reroute (better route)**: popup offering the faster route with ETA delta; similar-ETA alternates also render as gray polylines during guidance and *driving onto one adopts it* without any tap — [autoevolution](https://www.autoevolution.com/news/google-maps-starts-showing-confusing-route-popups-on-android-and-android-auto-254999.html), [blog.afi.io SDK teardown](https://blog.afi.io/blog/google-navigation-sdk-standalone/). Reports of Android auto-switching on a countdown are UNVERIFIED from a primary source ([Android Central](https://www.androidcentral.com/apps-software/google-maps-faster-route-available-sucks)).
- **Arrival**: arrival message replaces the routing card, then auto-dismisses (≥8 s in the [Android-for-Cars template spec](https://developers.google.com/cars/design/create-apps/sample-flows/arrive-at-destination)); per-waypoint arrival fires an arrival card, then guidance resumes to the next destination ([events doc](https://developers.google.com/maps/documentation/navigation/android-sdk/events), `ArrivalEvent.isFinalDestination`).

### 1.4 Preview → drive carry-over

- The route selected in preview (including a tapped gray alternate) is what guidance follows; Start does not re-plan (implied by the Start flow in [support 144339](https://support.google.com/maps/answer/144339?hl=en&co=GENIE.Platform%3DAndroid); no explicit statement found — UNVERIFIED as stated).
- Avoid options bind at route-computation time and apply to reroutes: the SDK attaches routing preferences to the destination request and requires resetting destinations to change strategy mid-trip — [route-preferences](https://developers.google.com/maps/documentation/navigation/ios-sdk/route-preferences). Route options are **inputs to computation, not live toggles during guidance**.
- Stops added in preview (≤9) carry in as sequential waypoints with per-waypoint arrival events — [events doc](https://developers.google.com/maps/documentation/navigation/android-sdk/events).
- Audio mode and speedometer are app settings persisting across sessions, not per-trip — [support 9356324](https://support.google.com/maps/answer/9356324), [iOS support 3273406](https://support.google.com/maps/answer/3273406?hl=en&co=GENIE.Platform%3DiOS).

## 2. Apple Maps: state machine and UI per state

### 2.1 State machine

```
[Browse/Idle]
  map + persistent bottom sheet (search field, Siri Suggestions, Places, Recents, Guides)
     │ tap search result / tap POI / long-press map (drops pin)
     ▼
[Place Card]
  sheet with place details; row of action buttons, leftmost = Directions (car icon + ETA)
     │ tap Directions
     ▼
[Route Preview]
  map zooms to whole route; primary + alternate routes drawn; route card lists each
  route with ETA + a Go button PER ROUTE; Avoid / Leave-at / Add Stop / EV picker in card
     │ tap Go on a route          │ start ≠ My Location → "Steps" (static list, no guidance)
     ▼                            │ swipe card down / X → back to Place Card / Browse
[Active Guidance]
  full-screen nav camera, top maneuver banner, bottom ETA card
     │ overview button ⇄ [Route Overview] (tap again to return)
     │ tap route card → expanded menu (Add Stop, Share ETA, Report Incident, End Route)
     │ off-route → automatic reroute (silent; see §8)
     │ faster route found → accept/dismiss prompt (community-corroborated; see §8)
     │ EV charge too low → OFFERS a route to nearest compatible Charger
     │ tap End Route / Siri "stop navigating" ───────────────► [Browse/Idle]
     ▼ arrival detected
[Arrived] — guidance ends; parked-car marker dropped on exiting the vehicle
     ▼
[Browse/Idle]
```

Transition sources: search → Directions → "Tap Go or Steps for the route you want to take" — [Apple guide ipha84a94043](https://support.apple.com/en-gb/guide/iphone/ipha84a94043/ios). End: "Tap the card at the bottom of the screen, then tap End Route" or Siri — [iph837d13d03](https://support.apple.com/en-gb/guide/iphone/iph837d13d03/ios). Overview toggle — [iph1b3553719](https://support.apple.com/en-gb/guide/iphone/iph1b3553719/ios). Low-charge offer — [iphc5e3a4b4b](https://support.apple.com/en-gb/guide/iphone/iphc5e3a4b4b/ios). Parked-car marker — [iph215b053f6](https://support.apple.com/en-gb/guide/iphone/iph215b053f6/ios).

### 2.2 The bottom sheet across the flow

- **Browse**: persistent sheet with a grab bar; scoots down for more map, swipe up fills the screen (search, suggestions, recents) — [AppleInsider walkthrough](https://appleinsider.com/inside/apple-maps/tips/inside-apple-maps---how-to-get-the-most-out-of-your-iphones-navigation-app). Matches the HIG resizable-sheet model: system detents `large` and `medium` (~half height), grabber drag-or-tap to cycle; medium detent recommended for progressive disclosure — [Sheets HIG](https://developer.apple.com/design/human-interface-guidelines/sheets). (Whether Maps' smallest resting height is a custom detent is UNVERIFIED — Apple doesn't document it.)
- **Place card**: details panel; the leftmost of four blue action icons is Directions, "usually show[ing] a car and a time estimate" — [AppleInsider](https://appleinsider.com/inside/apple-maps/tips/inside-apple-maps---how-to-get-the-most-out-of-your-iphones-navigation-app). The HIG now formalizes "place cards" (callout/compact/sheet styles) for third-party maps — [Maps HIG](https://developer.apple.com/design/human-interface-guidelines/maps).
- **Route preview card**, all controls "below the destination" ([ipha84a94043](https://support.apple.com/en-gb/guide/iphone/ipha84a94043/ios) unless noted):
  - Route list with one **Go button per route row**; tapping the row itself (not Go) opens the turn list ([iph1b3553719](https://support.apple.com/en-gb/guide/iphone/iph1b3553719/ios)) — a clean affordance split between "commit" and "inspect".
  - **Now** → Leave at / Arrive by (predicted-traffic ETAs).
  - **Avoid** → tolls / highways checkboxes.
  - **Add Stop**: up to 14 stops, drag-handle reorder, swipe-left delete before Go — [iph837d13d03](https://support.apple.com/en-gb/guide/iphone/iph837d13d03/ios).
  - EV: "Before you tap Go, scroll down in the route card. Choose another electric vehicle… or tap Different Car" — [iphc5e3a4b4b](https://support.apple.com/en-gb/guide/iphone/iphc5e3a4b4b/ios).
  - Alternates drawn on the zoomed-out map; "Tap the routes to change the route itself… To follow that route straight away, tap Go" — [AppleInsider](https://appleinsider.com/inside/apple-maps/tips/inside-apple-maps---how-to-get-the-most-out-of-your-iphones-navigation-app).
- **During guidance** the sheet is replaced by a compact bottom route card (ETA + exit affordance) that expands on tap — see §2.3. For prolonged/immersive tasks the HIG itself says to prefer full-screen over a sheet — [Sheets HIG](https://developer.apple.com/design/human-interface-guidelines/sheets) — the pattern Maps follows when guidance starts.

### 2.3 During-drive HUD

- **Top maneuver banner**: next turn + distance + lane needed; tapping it toggles full panel ⇄ basic mode (next turn + distance only) — [justinstrawn.com teardown](https://justinstrawn.com/lane-guidance/) (secondary; Apple publishes no banner spec).
- **Lane guidance**: "Lane guidance prepares you for turns and exits" — [apple.com/maps](https://www.apple.com/maps/); street-level 3D perspective at complex interchanges — [ipha84a94043](https://support.apple.com/en-gb/guide/iphone/ipha84a94043/ios), [newsroom 2021](https://www.apple.com/newsroom/2021/09/apple-maps-introduces-new-ways-to-explore-major-cities-in-3d/).
- **Bottom route card**: "A section at the bottom will show the current ETA as well as let you exit navigation" — [AppleInsider](https://appleinsider.com/inside/apple-maps/tips/inside-apple-maps---how-to-get-the-most-out-of-your-iphones-navigation-app). Tap to expand into the in-drive hub: **Add Stop** (categories while driving — gas/charging/food; "Your route is updated, and the chosen destination is the next stop") and **End Route** — [iph837d13d03](https://support.apple.com/en-gb/guide/iphone/iph837d13d03/ios); **Report an Incident** (Crash, Speed Check, Traffic, Roadwork, Hazard, Road Closure) — [support 105024](https://support.apple.com/en-us/105024), [iphb8a99022c](https://support.apple.com/guide/iphone/check-traffic-conditions-report-incidents-iphb8a99022c/ios); **Share ETA** (auto-updating messages) — [iph65c86df8c](https://support.apple.com/guide/iphone/share-your-estimated-time-of-arrival-iph65c86df8c/ios). Exact bar layout (ETA · min · mi + ellipsis) is UNVERIFIED in Apple docs; the contents above are the documented set.
- **Audio**: floating Audio Control button during guidance; all directions / alerts only / none, plus soft-normal-loud; defaults in Settings → Apps → Maps → Spoken Directions — [iphd3c85c193](https://support.apple.com/en-gb/guide/iphone/iphd3c85c193/ios).
- **Overview / camera**: dedicated button zooms to the entire route, tap again returns to turn-by-turn — [iph1b3553719](https://support.apple.com/en-gb/guide/iphone/iph1b3553719/ios). 3D road-level perspective at complex intersections ([apple.com/maps](https://www.apple.com/maps/)); dusk-activated "moonlit glow" night mode ([newsroom 2021](https://www.apple.com/newsroom/2021/09/apple-maps-introduces-new-ways-to-explore-major-cities-in-3d/)). The default tilted heading-up follow camera is universally observed but never specified by Apple — UNVERIFIED as a documented fact.
- **Speed limit / compass**: chips toggled in Settings → Apps → Maps → Driving — [ipha84a94043](https://support.apple.com/en-gb/guide/iphone/ipha84a94043/ios); real-time speed limits and speed cameras — [apple.com/maps](https://www.apple.com/maps/).
- **Backgrounding / Live Activity**: guidance continues when another app is open (return via the directions banner or status-bar indicator) — [ipha84a94043](https://support.apple.com/en-gb/guide/iphone/ipha84a94043/ios); directions render as a Live Activity in the Dynamic Island — [iph28f50d10d](https://support.apple.com/guide/iphone/view-live-activities-in-the-dynamic-island-iph28f50d10d/ios). Live Activities HIG: glanceable essentials only, medium-weight-or-heavier text, dark and light support, for tasks with a defined beginning and end — [Live Activities HIG](https://developer.apple.com/design/human-interface-guidelines/live-activities).
- **Arrival**: guidance ends at destination; parked-car marker dropped on exiting the vehicle — [iph215b053f6](https://support.apple.com/en-gb/guide/iphone/iph215b053f6/ios). Arrival radius and any explicit "You have arrived" card: UNVERIFIED (undocumented).

### 2.4 Preview → drive carry-over

- Stops added pre-Go carry into guidance; order edited by dragging; post-Go additions become "the next stop" — [iph837d13d03](https://support.apple.com/en-gb/guide/iphone/iph837d13d03/ios).
- Avoid and Leave-at/Arrive-by shape the routes offered before Go — [ipha84a94043](https://support.apple.com/en-gb/guide/iphone/ipha84a94043/ios).
- The tapped alternate is what Go starts — [AppleInsider](https://appleinsider.com/inside/apple-maps/tips/inside-apple-maps---how-to-get-the-most-out-of-your-iphones-navigation-app).
- The vehicle chosen in the preview card determines routing/charging for the drive — [iphc5e3a4b4b](https://support.apple.com/en-gb/guide/iphone/iphc5e3a4b4b/ios).

## 3. Cross-app comparison

| Aspect | Google Maps | Apple Maps |
|---|---|---|
| Commit button | One **Start** pill at card bottom, gated on origin = current location (else Preview only) | **Go** button on *each* route row; Steps replaces Go for remote origins |
| Alternates | Blue selected / gray alternates, tap polyline to switch; ETA callouts on lines | Alternates drawn on zoomed-out map, tap to switch; per-route rows in card |
| Waypoints pre-Go | ≤9 stops, drag reorder ("More → Edit stops") | ≤14 stops, drag reorder, swipe-delete ("Add Stop") |
| ETA bar | Bottom footer: time remaining (primary) / ETA / distance | Bottom route card: ETA + exit; tap to expand hub |
| Exit | Drag card up → **Exit** | Tap card → **End Route** (or Siri) |
| Overview | "Preview Route" in swiped-up card; SDK: north-up top-down fit | Dedicated overview button, tap toggles back |
| Off-route | Automatic silent reroute | Automatic reroute (undocumented, community-corroborated) |
| Better route | Popup with ETA delta; drive-onto-alternate adopts it | Accept/dismiss prompt (community-corroborated) |
| Audio | 3-state cycle: Mute / Alerts / Sound | 3-state: none / alerts only / all + volume |
| Free-look escape | Pan → Free mode + Re-center button (auto-return ~5 s in SDK sample) | Pan → overview-ish free state; re-center affordance (not formally documented) |
| Arrival | Arrival card ≥8 s → plain map; per-waypoint arrival cards | Guidance ends; parked-car marker |

(Row sources are the per-app sections above.)

## 4. Route preview: what it must contain (both apps agree)

1. **Whole-route map fit** with all alternates visible and tappable.
2. **Per-route summary** (time primary, distance secondary — Google's 2025 redesign made this explicit; [9to5Google](https://9to5google.com/2025/03/31/google-maps-directions-redesign/)).
3. **Commit affordance adjacent to the route it commits** (Apple's per-row Go is the cleaner pattern for multiple Plans).
4. **Inspection without commitment**: step list / turn list reachable from the row, plus a no-GPS "Preview"/"Steps" mode for remote origins.
5. **Options that bind at computation time** (avoid, departure time, vehicle) — both apps treat them as inputs to the route request, not live drive toggles.
6. What carries into the drive: the selected alternate, the waypoint list, the option set, and (Apple EV) the chosen vehicle. Nothing is re-planned at Go.

## 5. Route-following vs turn-by-turn: the component boundary

The SDKs name the two modes: HERE has **turn-by-turn navigation mode** vs **tracking mode** ("only tracks the current position… no voice instructions are provided") — [HERE SDK navigation guide](https://developer.here.com/documentation/ios-sdk-navigate/4.6.0.0/dev_guide/topics/navigation.html); TomTom drops into **free driving mode** between deviation and replan — [continuous replanning](https://developer.tomtom.com/navigation/ios/guides/navigation/continuous-replanning). Wayfinder's desired drive mode sits *between*: route-attached, maneuver-free.

### 5.1 Component inventory

| Component | Classification | Evidence |
|---|---|---|
| Camera following (heading-up, pitch, zoom) | **Standalone** | Mapbox `NavigationCamera` is its own module (idle/following/overview) fed by a `ViewportDataSource` — [Mapbox camera](https://docs.mapbox.com/ios/navigation/guides/map-and-camera/navigation-camera/); Google `cameraMode` set independently of guidance UI — [camera doc](https://developers.google.com/maps/documentation/navigation/ios-sdk/camera); MapKit `.followWithHeading` needs no route at all — [MKUserTrackingMode](https://developer.apple.com/documentation/mapkit/mkusertrackingmode/followwithheading) |
| Position puck + course smoothing | **Standalone** | Mapbox legacy caps course manipulation (`RouteControllerMaxManipulatedCourseAngle` = 25°) independent of maneuvers — [legacy constants](https://mapbox.github.io/mapbox-navigation-ios/navigation/0.4.0/Configuration.html) |
| Snapping position to the route line | **Standalone** | Google ships `GMSRoadSnappedLocationProvider` as a separate start/stop provider — [events doc](https://developers.google.com/maps/documentation/navigation/ios-sdk/events); Mapbox snapping distance = 10 m — [legacy constants](https://mapbox.github.io/mapbox-navigation-ios/navigation/0.4.0/Configuration.html) |
| Progress along polyline (distance/time remaining) | **Standalone** | Mapbox `RouteProgress` updates on every valid fix — [route-progress guide](https://docs.mapbox.com/ios/navigation/v2/guides/turn-by-turn-navigation/route-progress/); Google `didUpdateRemainingTime/Distance` — [events](https://developers.google.com/maps/documentation/navigation/ios-sdk/events); HERE `RouteProgressListener` — [HERE navigation](https://docs.here.com/here-sdk/docs/flutter-navigation); TomTom `ProgressUpdatedListener` ~1 Hz — [TomTom guide](https://developer.tomtom.com/navigation/android/guides/navigation/turn-by-turn-navigation) |
| ETA updates | **Standalone** | Same objects; Google fires on thresholds (default ≥1 s / ≥1 m change) — event-driven with hysteresis, not per-tick — [get-route-info](https://developers.google.com/maps/documentation/navigation/ios-sdk/get-route-info) |
| Off-route detection | **Standalone** | Mapbox: separately toggleable (`rerouteConfig.detectsReroute = false`) and observable (`.activeGuidance(.offRoute)`) — [rerouting guide](https://docs.mapbox.com/ios/navigation/guides/turn-by-turn-navigation/rerouting/); HERE emits deviation-in-meters and *the app decides* whether to reroute — [HERE navigation](https://docs.here.com/here-sdk/docs/flutter-navigation) |
| Reroute / replan | **Standalone**, separable from detection | TomTom auto-replan can be disabled and replaced with manual `setActiveRoutePlan` — [continuous replanning](https://developer.tomtom.com/navigation/ios/guides/navigation/continuous-replanning) — key for Wayfinder: replans must go through our planner, not a road-router |
| Arrival detection | **Standalone** | Mapbox `ArrivalOptions` (`arrivalInSeconds` recommended, or `arrivalInMeters`) + `ArrivalObserver` — [arrival detection](https://docs.mapbox.com/android/navigation/guides/ui-components/arrival-detection/); legacy maneuver-zone radius 40 m — [legacy constants](https://mapbox.github.io/mapbox-navigation-ios/navigation/0.4.0/Configuration.html); Google `didArriveAtWaypoint` — [events](https://developers.google.com/maps/documentation/navigation/ios-sdk/events) |
| Speed limit display | Standalone (needs map attributes, not a route) | HERE `SpeedWarningListener` works in tracking mode without a route — [HERE SDK](https://developer.here.com/documentation/ios-sdk-navigate/4.6.0.0/dev_guide/topics/navigation.html). Not available to Wayfinder from a bare polyline. |
| Maneuver banners | **TBT-only** | Require per-step instruction data + step tracking; omittable in Mapbox custom assembly vs drop-in `NavigationViewController` |
| Lane guidance | **TBT-only** | Bundled with maneuver instructions (Google `GMSNavigationNavInfo.currentStep`) — [nav-only feed](https://developers.google.com/maps/documentation/navigation/ios-sdk/nav-only-feed) |
| Voice instructions | **TBT-only, explicitly detachable** | Mapbox `SpeechSynthesizing` is a separate component; HERE text/voice listeners "can be attached or removed separately"; HERE tracking mode has no voice by design |
| Route line rendering; GPS pipeline, dead reckoning | Shared infrastructure | Both modes consume route geometry; Mapbox dead-reckoning interval 1.0 s sits below everything — [legacy constants](https://mapbox.github.io/mapbox-navigation-ios/navigation/0.4.0/Configuration.html) |

**The load-bearing architectural fact**: in every SDK examined, banners/lanes/voice are *consumers* of the core progress engine's events, never producers of state the core needs. Google's turn-by-turn **data feed** proves the entire UI is detachable: apps can consume `GMSNavigatorListener.didChangeNavInfo` with no Google UI at all — [nav-only feed](https://developers.google.com/maps/documentation/navigation/ios-sdk/nav-only-feed). Google's SDK exposes header, footer, trip-progress bar, speedometer, recenter button, and alternate-route display each as an independent toggle — [iOS controls](https://developers.google.com/maps/documentation/navigation/ios-sdk/controls).

### 5.2 Documented calibration numbers

- Off-route: Mapbox legacy default **50 m** off the step before reroute (`RouteControllerMaximumDistanceBeforeRecalculating`); snapping **10 m**; TomTom detects deviation in the **100–200 m** band — [Mapbox legacy constants](https://mapbox.github.io/mapbox-navigation-ios/navigation/0.4.0/Configuration.html), [TomTom replanning](https://developer.tomtom.com/navigation/ios/guides/navigation/continuous-replanning).
- Arrival: Mapbox maneuver-zone radius **40 m**, or time-based `arrivalInSeconds` (traffic-aware, recommended) — [arrival detection](https://docs.mapbox.com/android/navigation/guides/ui-components/arrival-detection/).
- ETA UI updates: Google thresholds default **1 s / 1 m** — [get-route-info](https://developers.google.com/maps/documentation/navigation/ios-sdk/get-route-info).
- Course smoothing: Mapbox caps snapped-course correction at **25°** — [legacy constants](https://mapbox.github.io/mapbox-navigation-ios/navigation/0.4.0/Configuration.html).

### 5.3 What MapKit gives natively (and doesn't)

- `MKDirections`/`MKRoute`: polyline, distance, `expectedTravelTime`, steps with per-step polylines and instruction text — data only — [MKRoute](https://developer.apple.com/documentation/mapkit/mkroute), [MKDirections](https://developer.apple.com/documentation/mapkit/mkdirections). (Wayfinder computes Legs with its own Routing Engine, so this matters only as a statement of the platform gap.)
- `MKMapView.userTrackingMode = .followWithHeading`: "the map follows the user's location and rotates when the heading changes" — [followWithHeading](https://developer.apple.com/documentation/mapkit/mkusertrackingmode/followwithheading). Known to fight manual zoom — [Apple forums](https://developer.apple.com/forums/thread/689782). (Wayfinder renders with MapLibre, which has its own tracking modes; the point stands that following-camera is platform-level, guidance is not.)
- **Not provided by Apple**: guidance UI, route progress, map matching, off-route detection, reroute triggers, nav camera with pitch/zoom-to-speed. Apple's own turn-by-turn UI is private to Apple Maps (UNVERIFIED as a single citable statement; consistent across all sources). Everything in §5.1's standalone column must be hand-built: distance-along-polyline from the fix, remaining distance/time from remaining geometry, perpendicular distance for off-route, radius checks for arrival.

### 5.4 Precedents for route-following without turn-by-turn

- **ABRP itself** (the closest precedent): driving mode shows destination, ETA, distance remaining, a route progress bar, plus a next-Leg graph of elevation and predicted SoC%, positioned as "a real-time plan follow-up tool… replan as necessary" — [ABRP 3.3 blog](https://forum.abetterrouteplanner.com/blogs/entry/20-a-better-routeplanner-33/). No maneuver guidance in this mode.
- **HERE tracking mode**: production drive UI without a route — current street name, map-matched position, speed limits, no voice — [HERE SDK](https://developer.here.com/documentation/ios-sdk-navigate/4.6.0.0/dev_guide/topics/navigation.html).
- **TomTom free driving mode**: the SDK itself runs route-less between deviation and replan — route attachment is a soft state, not a mode switch — [continuous replanning](https://developer.tomtom.com/navigation/ios/guides/navigation/continuous-replanning).
- **Komoot** (cycling/hiking): follows a planned route with off-route warnings + haptics and auto-reroute; on "off-grid" segments it degrades to pure track-following with a *dismissible* off-route warning — [Komoot warnings](https://www.komoot.com/help/warnings), [Navigation FAQ](https://support.komoot.com/hc/en-us/articles/10605424981402-Navigation-FAQ). Its documented failure mode is the design caution: a bare "you're off the line" nag with no replan path.
- **OsmAnd "Navigate by Track"**: follows an imported GPX, auto-recalculates on deviation — [OsmAnd docs](https://osmand.net/docs/user/navigation/setup/gpx-navigation/).
- **Google's destination-less "driving mode"** (ETA/traffic, no maneuvers; discontinued ~2025) — [SlashGear](https://www.slashgear.com/1967918/google-maps-driving-mode-discontinued-reason/), [9to5Google](https://9to5google.com/2025/04/25/google-assistant-driving-mode-maps/) (press-only; no surviving first-party doc — UNVERIFIED beyond that).

### 5.5 Minimal viable drive-mode element set

Grounded in what the SDKs demonstrably ship as separable — the core engine plus camera, zero guidance consumers:

1. **Snap-to-route + puck**: perpendicular projection of the GPS fix onto the Plan's Leg polylines; snap within ~10 m, show raw position beyond; smooth course with a capped correction (~25°).
2. **Following camera**: heading-up from course (not compass, at driving speed), fixed pitch, bearing smoothing; a *free* state on user gesture with a Re-center affordance returning to following (Google's exact pattern, [camera doc](https://developers.google.com/maps/documentation/navigation/ios-sdk/camera)); an overview toggle fitting the whole remaining Plan.
3. **Progress + ETA**: distance-along-route → remaining distance/time from remaining Leg geometry; throttle UI updates Google-style (≥1 s / ≥1 m deltas). For Wayfinder this is also the input to the SoC-vs-Plan comparison — which ABRP shows is the actual value of the mode.
4. **Off-route detection decoupled from replan**: flag at ~50 m sustained deviation; then hand off to the Wayfinder planner (the HERE/TomTom pattern — deviation event, app-controlled replan), since Charging Stops must be re-solved, not just road geometry.
5. **Arrival/stop detection**: radius-along-route check per Charging Stop and destination (~40 m zone or time-based), advancing the "current Leg" — this replaces maneuver-step tracking as the only stepper.

**Explicitly deferred** (proven detachable everywhere): maneuver banners, lane guidance, voice, speed-limit display (needs map attributes a polyline doesn't carry).

## 6. The EV layer during an active drive

### 6.1 Apple Maps EV routing

- iOS 14 launch: "Electric vehicle routing adds charging stops along a planned route based on current vehicle charge and charger types" — [Apple newsroom, June 2020](https://www.apple.com/newsroom/2020/06/apple-reimagines-the-iphone-experience-with-ios-14/); charging time is folded into the ETA — [idownloadblog tutorial](https://www.idownloadblog.com/2020/09/30/apple-maps-electric-vehicle-routing-tutorial/). Implemented as a Maps + SiriKit Intents integration (automaker app implements `INGetCarPowerLevelStatusIntent`; [`INCar`](https://developer.apple.com/documentation/intents/incar) "represents a specific electric vehicle that Maps uses during route planning and navigation") — **not** a public MapKit routing API ([forums thread](https://developer.apple.com/forums/thread/653328); neither [WWDC22 MapKit](https://developer.apple.com/videos/play/wwdc2022/10035/) nor [WWDC25](https://developer.apple.com/videos/play/wwdc2025/204/) exposes EV routing to third parties).
- Current behavior ([iphc5e3a4b4b](https://support.apple.com/en-gb/guide/iphone/iphc5e3a4b4b/ios)): "Maps can track your vehicle's charge. By analyzing elevation changes along the route and other factors, Maps identifies appropriate charging stations along the way… **If you drive until your charge gets too low, you're offered a route to the nearest compatible charging station.**" The mid-drive trigger is an **offer/prompt, not silent re-insertion**.
- Per-stop battery display: Ford's official doc lists "Auto-suggested charging stops, including multi-stop routes… **Battery on-arrival estimate for all route stops.** Dynamic Charger Filtering by speed, charging network, and more" — [Ford support](https://www.ford.ca/support/how-tos/electric-vehicles/other-electric-vehicle-information/how-do-i-use-apple-maps-ev-routing/); route card shows stops with estimated SoC% at each — [AppleInsider on Mach-E](https://appleinsider.com/articles/22/03/21/ford-adds-apple-maps-ev-routing-support-to-mustang-mach-e-models). A persistent SoC readout on the guidance screen itself is UNVERIFIED (no source describes it).
- iOS 17: preferred charging networks at setup; real-time availability as total-vs-occupied stalls; filter by network and plug; only compatible connectors shown — [9to5Mac](https://9to5mac.com/2023/08/19/ios-17-adds-real-time-charging-availability-info-for-ev-drivers/), [TechCrunch WWDC23](https://techcrunch.com/2023/06/07/need-to-charge-your-ev-apple-maps-will-show-open-spots-near-you/).

### 6.2 Google Maps EV

- **Built-in (Android Automotive)** — [help 9773205](https://support.google.com/maps/answer/9773205?hl=en): destination card shows "estimated battery on arrival," which "**continuously updates as you drive**" once navigation starts — the flagship in-drive element. If the destination is unreachable, "charging [is] automatically added along your route," charge time folded into trip duration; each added stop gets "a recommended minimum charging time" computed from arrival battery, the car's Charging Curve, and station speed. Filters: plug type, payment network, speed. ([blog.google 2024](https://blog.google/products-and-platforms/products/maps/new-ways-to-power-up-your-electric-vehicle-adventures-with-google-maps/) adds real-time port availability and AI location summaries.)
- **Android Auto phone-powered, March 2026**: AI battery predictions for 350+ EV models; expected battery usage, recommended stops with charge-time estimates, battery-% on arrival, ETA including charging; stop recalculated/moved earlier if consumption runs high mid-trip — [blog.google announcement](https://blog.google/products-and-platforms/products/maps/google-maps-simplifies-battery-predictions-and-trip-planning-for-350-android-auto-ev-models/).
- **Phone app**: charger *search* only — real-time availability, "very fast" (150–350 kW) filter; no SoC modeling or automatic stops on the phone — [blog.google 2023](https://blog.google/products-and-platforms/products/maps/sustainable-immersive-maps-announcements/), [Electrek](https://electrek.co/2023/02/08/google-maps-enhances-ev-experience-with-new-charging-options/).
- Design reading: a charging stop is a **waypoint with extra metadata** (charge time in the ETA, battery-% annotations); battery-on-arrival is a **continuously updated field on the ETA card**, not a separate screen.

### 6.3 ABRP (public facts and UI observations only)

- Plan view lists, per Charging Stop: departure and arrival SoC, cost, charge duration, distance between Chargers — [tripversed walkthrough](https://tripversed.com/how-to-use-a-better-route-planner/).
- Drive mode, official framing: "just go to driving mode and use ABRP as a real-time plan follow-up tool and even navigator, replan as necessary and get continuously updated information" — [App Store listing](https://apps.apple.com/us/app/a-better-routeplanner-abrp/id1490860521). Per the ABRP 3.3 blog: auto-switches to driving mode when moving; shows a graph of the next Leg with elevation and expected SoC% plus ETA; without car telemetry the user can manually enter actual SoC, "shown as a big battery symbol above the driving mode window" — [ABRP 3.3 blog entry](https://forum.abetterrouteplanner.com/blogs/entry/20-a-better-routeplanner-33/) (page intermittently unavailable; wording via search snippets, low-confidence but widely corroborated).
- Replan triggers: "If traffic builds up or conditions change, we'll automatically find a better route and update your charging stops"; charger status checked before arrival — [abetterrouteplanner.com](https://abetterrouteplanner.com/home). Community reports say live-SoC-driven replan exists but lags a stale start SoC — [featurebase thread](https://abrp.featurebase.app/p/live-soc-at-start).
- Calibration: reference consumption defined at constant 110 km/h in near-perfect conditions; live data yields a per-speed-segment calibrated value — [reference-consumption article](https://abrp.featurebase.app/articles/3305478-reference-consumption).
- CarPlay/Android Auto and live traffic/weather/charger status are Premium; ABRP does its own turn-by-turn ("Route Directions" toggle) rather than handing off — [Premium page](https://abetterrouteplanner.com/premium/).
- An explicit check-off/skip UX at a reached Charging Stop is UNVERIFIED — no public source describes the mechanism.
- Corporate note: Rivian acquired Iternio and integrated its planning natively — [Businesswire](https://www.businesswire.com/news/home/20230621721913/en).

### 6.4 Cross-reference: Tesla (the in-drive benchmark)

- The turn list includes Supercharger stops with recommended charge time each and estimated energy on arrival at each; destination battery-% sits with the ETA info and updates continuously as the car "monitors energy usage" — [Tesla owner's manual, Maps and Navigation](https://www.tesla.com/ownersmanual/model3/en_eu/GUID-01F1A582-99D1-4933-B5FB-B2F0203FFE6F.html) (page blocks fetch; facts via manual snippets + [tesery guide](https://www.tesery.com/blogs/tesla-tips/how-to-use-tesla-trip-planner)).
- A warning tells the driver to slow to a specific speed when energy is marginal (wording UNVERIFIED).
- **While charging, the displayed countdown is "time until you have enough charge to reach the *next* stop"** — the check-off mechanic is energy-driven: leave when it hits zero (manual via snippets).
- Energy app plots predicted vs actual energy along the trip — the closest analog to ABRP's planned-vs-actual SoC curve — [notateslaapp](https://www.notateslaapp.com/news/775/); arrival-SoC target slider added later — [notateslaapp](https://www.notateslaapp.com/news/2392/tesla-improves-trip-planner-arrival-state-of-charge-coming).

### 6.5 The recurring in-drive EV pattern

1. **One number dominates**: battery-%-at-arrival (next Charging Stop and/or destination), continuously re-estimated while driving — on the ETA card (Google built-in), in the directions list (Tesla, Apple/Ford), or as a SoC curve on the Leg graph (ABRP).
2. **Charging Stops live in the route card/directions list**, each with arrival SoC + recommended charge time — not a separate mode. ABRP is the outlier with a dedicated Leg-graph screen.
3. **Check-off is energy/position-driven, not tap-driven**: Tesla's charge-until-you-can-reach-next-stop countdown is the only documented explicit progression mechanic; no app documents a manual check-off/skip gesture.
4. **Replan triggers differ**: Apple prompts (low-charge offer); Google built-in auto-adds when unreachable; ABRP auto-replans on traffic/conditions/charger status; Tesla continuously re-estimates and warns rather than adding stops.
5. **Preview carries the full economics** (per-stop arrival SoC, charge-to, duration, cost); **drive mode collapses to next-stop arrival SoC + ETA**, the rest a tap away.

## 7. Implications for a minimal Wayfinder drive mode

Wayfinder today ends at `ResultCard` over `PlannerMapView` (`app/Wayfinder/Sources/`). The findings suggest:

```
[Plan ready / ResultCard]  ← current dead-end; becomes the Route Preview state
   │  Go button on the card (gate on current location ≈ Plan origin,
   │  else offer the read-only plan view — the Apple "Steps" pattern)
   ▼
[Driving]
   camera: heading-up following ⇄ free-look (pan) + Re-center ⇄ overview toggle
   bottom bar: ETA · time remaining · distance remaining · SoC-at-next-stop
   stop list: Charging Stops with arrival SoC + charge duration, one tap away
   │  off-route ≥ ~50 m sustained → banner + replan via Wayfinder planner
   │  reach Charging Stop (~40 m along-route) → arrived card
   │    (charge-to target + "charge until you can reach the next stop") → advance Leg
   │  tap expanded card → End Route
   ▼
[Arrived] → summary → back to browse
```

- The **Go button** completes the reference apps' universal contract: preview commits, Go starts, nothing re-plans at Go. Options (vehicle, SoC, avoid) stay computation inputs.
- The **drive HUD needs no maneuver layer**: §5's evidence is unanimous that ETA bar, following camera, progress, off-route, and arrival are separable. ABRP ships exactly this mode.
- The **EV differentiator is the SoC-vs-Plan comparison**, not turn instructions: predicted SoC at next Charging Stop, continuously re-estimated, is the one number every reference product elevates.
- **Replan must route through the Wayfinder planner** (Charging Stops re-solved), following HERE/TomTom's detection-decoupled-from-replan pattern — never a bare road reroute.
- **Guidance should be full-screen**, not a sheet ([Sheets HIG](https://developer.apple.com/design/human-interface-guidelines/sheets)); a Live Activity is the natural later extension ([Live Activities HIG](https://developer.apple.com/design/human-interface-guidelines/live-activities)).
- Touch-target caution: NN/g-cited criticism of Google's nav screen — "almost everything on the screen is interactive in multiple ways, causing frequent misclicks" ([roundup](https://www.designstudiouiux.com/blog/mobile-navigation-ux/)) — argues for fewer, larger, single-purpose targets.

## 8. What could not be established from trustworthy sources

- Apple: arrival radius / "You have arrived" card; default heading-up tilted camera as a documented spec; exact bottom-bar layout; maneuver-banner styling; off-route reroute and faster-route prompt wording (community-corroborated only: [discussions thread](https://discussions.apple.com/thread/254942472)); whether arrival SoC is persistently shown on the guidance screen; whether the smallest sheet resting height is a custom detent.
- Google: exact Start-button styling; consumer-app ETA-bar field ordering; auto-accept countdown on faster-route popups; explicit statement that Start never re-plans; phone-app arrival copy.
- ABRP: any explicit stop check-off/skip gesture; exact drive-mode wording (source page intermittently unavailable).
- Tesla: exact slow-down warning wording (manual page blocks fetching; facts via snippets).
- General: no public documentation of Apple's internal turn-by-turn UI implementation; Google's discontinued destination-less driving mode survives only in press coverage.
