// `--autotest route-geometry-smoke` (wayfinder #84): asserts RouteGeometry's pure functions
// directly -- no UI, no map. Builds a straight, synthetic 6-point "road" (constant latitude, so
// its perpendicular direction is pure latitude and the fixture's math is trivial to eyeball) and
// checks: a 30 m perpendicular origin offset trims the display polyline to the projection point
// with a dashed connector drawn (its ~30 m gap is over the 10 m threshold); an 80 m offset is
// past the 50 m trim threshold, so nothing is trimmed and no connector is drawn; a 5 m offset
// still trims but is under the connector's 10 m gap threshold, so no connector; the destination
// end behaves symmetrically against the trailing edge; and a degenerate 2-point polyline is
// returned completely untouched.
//
// `map-demo-route` (same file) is the REVIEWER-FACING visual-verification mode (same pattern as
// AutotestSocChart.swift's chart-demo-plan/-drive): stages the golden plan on the map, fits the
// camera to the route's first ~3 km at a zoom where road-following fidelity (the DP-simplified
// polyline hugging the road, not cutting corners) is visible, and prints a READY line instead of
// calling `finish` -- it never terminates the app itself, so the 15-mode sweep excludes it.
//
// A SEPARATE FILE, same reasoning as AutotestObdSmoke.swift/AutotestSocChart.swift's own headers:
// new autotest code reproducibly breaks the Swift 6 strict-concurrency checker once appended to
// the ~2500-line Autotest.swift.
import CoreLocation
import Foundation
import MapLibre
import PlannerKit

extension Autotest {
    @MainActor
    static func runRouteGeometrySmoke() async {
        var ok = true

        // A straight, 6-point synthetic "road" along constant latitude, ~717 m per segment
        // (~3.6 km total) -- long enough to exercise the leading/trailing 2 km search windows
        // with room to spare.
        let roadLat = 50.0
        let lonStepDeg = 0.01
        let road = (0..<6).map { CLLocationCoordinate2D(latitude: roadLat, longitude: 5.00 + Double($0) * lonStepDeg) }

        // A point directly "above" `road[index]` (same longitude), offset `offsetM` metres of
        // pure latitude -- perpendicular to this east-west road by construction.
        func perpendicularPoint(aboveIndex index: Int, offsetM: Double) -> CLLocationCoordinate2D {
            CLLocationCoordinate2D(latitude: roadLat + offsetM / 111_320.0, longitude: road[index].longitude)
        }
        func coordinatesClose(_ a: CLLocationCoordinate2D, _ b: CLLocationCoordinate2D) -> Bool {
            abs(a.latitude - b.latitude) < 1e-9 && abs(a.longitude - b.longitude) < 1e-9
        }
        // A point nowhere near `road`, so it never triggers a trim on the end it's NOT testing.
        let irrelevant = CLLocationCoordinate2D(latitude: 0, longitude: 0)

        // 1: a 30 m perpendicular origin offset (above road[1], so the projection foot IS
        // road[1] exactly) trims road[0] off the front and connects with a dashed stub.
        let origin30 = perpendicularPoint(aboveIndex: 1, offsetM: 30)
        let trim30 = RouteGeometry.trimmedDisplayPolyline(road, origin: origin30, destination: irrelevant)
        let originTrimOk = trim30.display.count == road.count - 1 && coordinatesClose(trim30.display[0], road[1])
        report("origin-trim-30m", originTrimOk, "display.count=\(trim30.display.count)")
        ok = ok && originTrimOk

        let originConnectorPresentOk = trim30.originConnector != nil
            && trim30.originConnector.map { $0.count == 2 && coordinatesClose($0[1], road[1]) } == true
        report("origin-connector-present", originConnectorPresentOk, "connector=\(String(describing: trim30.originConnector))")
        ok = ok && originConnectorPresentOk

        // 2: an 80 m offset is past the 50 m trim threshold -- untouched, no connector.
        let origin80 = perpendicularPoint(aboveIndex: 1, offsetM: 80)
        let trim80 = RouteGeometry.trimmedDisplayPolyline(road, origin: origin80, destination: irrelevant)
        let originNoTrimOk = trim80.display.count == road.count && coordinatesClose(trim80.display[0], road[0])
            && trim80.originConnector == nil
        report("origin-no-trim-80m", originNoTrimOk, "display.count=\(trim80.display.count)")
        ok = ok && originNoTrimOk

        // 3: a 5 m offset still trims (under 50 m) but the resulting gap is under the 10 m
        // connector threshold -- trimmed, no connector.
        let origin5 = perpendicularPoint(aboveIndex: 1, offsetM: 5)
        let trim5 = RouteGeometry.trimmedDisplayPolyline(road, origin: origin5, destination: irrelevant)
        let originConnectorAbsentOk = trim5.display.count == road.count - 1 && trim5.originConnector == nil
        report("origin-connector-absent", originConnectorAbsentOk, "connector=\(String(describing: trim5.originConnector))")
        ok = ok && originConnectorAbsentOk

        // 4-6: the destination end, symmetric against the trailing edge (above road[4], one
        // short of the last vertex, so trimming drops road[5]).
        let dest30 = perpendicularPoint(aboveIndex: 4, offsetM: 30)
        let destTrim30 = RouteGeometry.trimmedDisplayPolyline(road, origin: irrelevant, destination: dest30)
        let destTrimOk = destTrim30.display.count == road.count - 1
            && coordinatesClose(destTrim30.display[destTrim30.display.count - 1], road[4])
        report("destination-trim-30m", destTrimOk, "display.count=\(destTrim30.display.count)")
        ok = ok && destTrimOk

        let destConnectorPresentOk = destTrim30.destinationConnector != nil
            && destTrim30.destinationConnector.map { $0.count == 2 && coordinatesClose($0[1], road[4]) } == true
        report(
            "destination-connector-present", destConnectorPresentOk,
            "connector=\(String(describing: destTrim30.destinationConnector))"
        )
        ok = ok && destConnectorPresentOk

        let dest80 = perpendicularPoint(aboveIndex: 4, offsetM: 80)
        let destTrim80 = RouteGeometry.trimmedDisplayPolyline(road, origin: irrelevant, destination: dest80)
        let destNoTrimOk = destTrim80.display.count == road.count
            && coordinatesClose(destTrim80.display[destTrim80.display.count - 1], road[5])
            && destTrim80.destinationConnector == nil
        report("destination-no-trim-80m", destNoTrimOk, "display.count=\(destTrim80.display.count)")
        ok = ok && destNoTrimOk

        let dest5 = perpendicularPoint(aboveIndex: 4, offsetM: 5)
        let destTrim5 = RouteGeometry.trimmedDisplayPolyline(road, origin: irrelevant, destination: dest5)
        let destConnectorAbsentOk = destTrim5.display.count == road.count - 1 && destTrim5.destinationConnector == nil
        report(
            "destination-connector-absent", destConnectorAbsentOk,
            "connector=\(String(describing: destTrim5.destinationConnector))"
        )
        ok = ok && destConnectorAbsentOk

        // 7: a degenerate 2-point polyline is returned completely untouched, regardless of how
        // close origin/destination are to it.
        let twoPoint = [road[0], road[1]]
        let degenerate = RouteGeometry.trimmedDisplayPolyline(
            twoPoint, origin: perpendicularPoint(aboveIndex: 0, offsetM: 5),
            destination: perpendicularPoint(aboveIndex: 1, offsetM: 5)
        )
        let degenerateOk = degenerate.display.count == 2
            && coordinatesClose(degenerate.display[0], twoPoint[0])
            && coordinatesClose(degenerate.display[1], twoPoint[1])
            && degenerate.originConnector == nil && degenerate.destinationConnector == nil
        report("degenerate-untouched", degenerateOk, "display.count=\(degenerate.display.count)")
        ok = ok && degenerateOk

        await finish(ok: ok)
    }

    // MARK: map-demo-route (wayfinder #84, visual verification -- see header)

    /// Stages the golden LU -> Amsterdam plan on the map, fitted to the route's first ~3 km at a
    /// zoom where road-following fidelity is visible, for the reviewer's own screenshot.
    @MainActor
    static func runMapDemoRoute(store: PlanStore) async {
        store.setOrigin(CLLocationCoordinate2D(latitude: 49.6116, longitude: 6.1319))
        store.vehicle = .ioniq5Lr2wd
        store.load()
        let ready = await waitWithTimeout(seconds: 30) { store.plannerStatus == .ready }
        report("planner-ready", ready)
        guard ready else { await finish(ok: false, sleepSeconds: 8) }

        store.setDestination(name: "Amsterdam", coordinate: CLLocationCoordinate2D(latitude: 52.3702, longitude: 4.8952))
        let planLanded = await waitWithTimeout(seconds: 30) { store.planVersion == 1 }
        report("plan-landed", planLanded)

        // Center on the ORIGIN, not a bounds-fit of the leading stretch: the search sheet covers
        // the top half of the screen in this staged state, and the origin trim + dashed connector
        // (the user-visible half of wayfinder #84) must land in the visible mid-band. Zoom 17 is
        // street level -- where the user's own screenshots showed off-track chords and kinked
        // curves -- so the reviewer judges the line exactly where the defect lived.
        store.mapView.setCenter(store.originCoordinate, zoomLevel: 17, animated: false)

        print("AUTOTEST map-demo-route READY")
    }
}
