// `--autotest obd-smoke` (wayfinder #78): drives TelemetryLinkStore + StubTelemetryLink directly
// -- CoreBluetooth reports `.unsupported` on the simulator, so CxBleLink itself is proven only on
// real hardware, by the separate driveway-smoke ticket. Proves the 12V-safety gating: a closed
// gate never opens/scans; an open gate reaches `.ready` and a ScriptedDialogue completes
// byte-for-byte, with one of its commands deliberately longer than the stub's configured MTU to
// prove write-chunking; the gate closing mid-session tears down cleanly; an unexpected disconnect
// while the gate stays open triggers exactly one reconnect; and CAN-quiet (the stub gone silent)
// trips TelemetryLinkPolicy's backoff, disconnecting the stub -- checked both through the store
// and, for the backoff schedule's timing itself, directly against TelemetryLinkPolicy with
// synthetic `now` values (no real waiting), proving the reconnect attempt count stays bounded to
// the documented schedule over a simulated multi-minute span rather than storming.
//
// A SEPARATE FILE, unlike every other `--autotest` mode (which live inline in Autotest.swift):
// appending this logic directly into that ~2500-line file was tried first and reproducibly broke
// the Swift 6 strict-concurrency checker -- pre-existing, untouched functions elsewhere in that
// same file (runMapSmoke, runEditorSmoke, etc.) started failing "sending non-Sendable type
// '() -> Bool'" at their own long-standing `waitWithTimeout` calls, even with this file's content
// reduced to a single inert comment. Isolated by bisection: the exact same code compiles cleanly
// as its own file. This reads as a whole-file analysis precision limit in this Swift toolchain
// once the file is large enough, not a real Sendability issue -- `report`/`finish`/
// `waitWithTimeout` were widened from `private` to `internal` in Autotest.swift so this file can
// reuse them instead of duplicating them.
import Foundation

extension Autotest {
    /// ELM-flavored two-step script shared by the stub link and the scripted dialogue (see
    /// StubTelemetryLink.swift's ScriptedExchange header for why one script drives both): a
    /// reset banner, then a flow-control setup command 13 bytes long -- longer than the 8-byte
    /// stub MTU `runObdSmoke` configures, so the very same run that proves "a dialogue completes
    /// byte-for-byte in order" also proves write-chunking (acceptance criteria 2 and 3).
    private static let obdSmokeScript: [ScriptedExchange] = [
        ScriptedExchange(outgoing: Data("ATZ\r".utf8), incoming: Data("ELM327 v1.5\r\r>".utf8)),
        ScriptedExchange(outgoing: Data("ATFCSD300000\r".utf8), incoming: Data("OK\r\r>".utf8)),
    ]

    @MainActor
    static func runObdSmoke() async {
        var ok = true

        // 1. Gate closed -> zero open/scan attempts (the standing "never scan a parked car"
        // constraint, checked unconditionally first in TelemetryLinkPolicy.tick).
        let closedStub = StubTelemetryLink()
        let closedStore = TelemetryLinkStore(link: closedStub)
        let gateClosedOk = closedStub.openCallCount == 0 && closedStore.phase == .idle
        report(
            "gate-closed-no-scan", gateClosedOk,
            "openCallCount=\(closedStub.openCallCount) phase=\(closedStore.phase)"
        )
        ok = ok && gateClosedOk

        // 2 & 3. Gate open -> stub reaches ready; a scripted dialogue completes byte-for-byte;
        // one of its writes exceeds the stub's MTU and arrives as the correct chunk sequence.
        let sessionStub = StubTelemetryLink()
        sessionStub.maxWriteLength = 8
        sessionStub.script = obdSmokeScript
        let sessionStore = TelemetryLinkStore(link: sessionStub)
        sessionStore.gateOpen = true

        let readyOk = await waitWithTimeout(seconds: 5) { sessionStore.phase == .ready }
        report("gate-open-reaches-ready", readyOk, "phase=\(sessionStore.phase)")
        ok = ok && readyOk

        let dialogue = ScriptedDialogue(script: obdSmokeScript)
        await sessionStore.runOnePoll(dialogue: dialogue)
        let dialogueOk = dialogue.isFinished && !dialogue.timedOut
        report("dialogue-completes", dialogueOk, "isFinished=\(dialogue.isFinished) timedOut=\(dialogue.timedOut)")
        ok = ok && dialogueOk

        let expectedChunks = [Data("ATZ\r".utf8), Data("ATFCSD30".utf8), Data("0000\r".utf8)]
        let chunkingOk = sessionStub.sentChunks == expectedChunks
        report(
            "write-chunking", chunkingOk,
            "got \(sessionStub.sentChunks.map { $0.count }), want \(expectedChunks.map { $0.count })"
        )
        ok = ok && chunkingOk

        // 5. Gate closes mid-session -> clean teardown, same session continued from above.
        sessionStore.gateOpen = false
        let teardownOk = await waitWithTimeout(seconds: 5) { sessionStore.phase == .idle && sessionStub.state == .idle }
        report("gate-close-teardown", teardownOk, "phase=\(sessionStore.phase) linkState=\(sessionStub.state)")
        ok = ok && teardownOk

        // 6. Unexpected disconnect while the gate stays open -> exactly one reconnect attempt.
        let reconnectStub = StubTelemetryLink()
        let reconnectStore = TelemetryLinkStore(link: reconnectStub)
        reconnectStore.gateOpen = true
        let initiallyReadyOk = await waitWithTimeout(seconds: 5) { reconnectStore.phase == .ready }
        report("reconnect-initial-ready", initiallyReadyOk, "phase=\(reconnectStore.phase)")
        ok = ok && initiallyReadyOk

        let opensBeforeDisconnect = reconnectStub.openCallCount
        reconnectStub.simulateUnexpectedDisconnect()
        let reconnectedOk = await waitWithTimeout(seconds: 8) {
            reconnectStore.phase == .ready && reconnectStub.openCallCount == opensBeforeDisconnect + 1
        }
        report(
            "reconnect-after-disconnect", reconnectedOk,
            "openCallCount=\(reconnectStub.openCallCount), want \(opensBeforeDisconnect + 1)"
        )
        ok = ok && reconnectedOk

        // No SECOND reconnect attempt once already back to ready -- give one a moment to (not)
        // happen before checking.
        try? await Task.sleep(nanoseconds: 300_000_000)
        let exactlyOneOk = reconnectStub.openCallCount == opensBeforeDisconnect + 1
        report("reconnect-exactly-once", exactlyOneOk, "openCallCount=\(reconnectStub.openCallCount)")
        ok = ok && exactlyOneOk

        // 4. CAN-quiet (connected but silent) -> the policy disconnects and backs off, checked
        // at the store+stub level for the disconnect itself...
        let quietStub = StubTelemetryLink()
        quietStub.script = obdSmokeScript
        let quietStore = TelemetryLinkStore(link: quietStub)
        quietStore.gateOpen = true
        let quietReadyOk = await waitWithTimeout(seconds: 5) { quietStore.phase == .ready }
        report("canquiet-setup-ready", quietReadyOk, "phase=\(quietStore.phase)")
        ok = ok && quietReadyOk

        quietStub.simulateSilence()
        for _ in 0..<TelemetryLinkPolicy.canQuietThreshold {
            await quietStore.runOnePoll(dialogue: ScriptedDialogue(script: obdSmokeScript), stepTimeoutS: 0.2)
        }
        let tripsBackoffOk: Bool
        if case .backingOff(let reason, _) = quietStore.phase {
            tripsBackoffOk = reason == .canQuiet
        } else {
            tripsBackoffOk = false
        }
        report("canquiet-trips-backoff", tripsBackoffOk, "phase=\(quietStore.phase)")
        ok = ok && tripsBackoffOk

        let quietDisconnectedOk = quietStub.closeCallCount >= 1 && quietStub.state != .ready
        report(
            "canquiet-disconnects", quietDisconnectedOk,
            "closeCallCount=\(quietStub.closeCallCount) linkState=\(quietStub.state)"
        )
        ok = ok && quietDisconnectedOk

        // ...and, for the backoff SCHEDULE's own timing (no reconnect storm), directly against
        // TelemetryLinkPolicy with synthetic `now` values -- no real waiting for the ticket's
        // 5/15/30/60s schedule.
        ok = runBackoffScheduleBoundsCheck() && ok

        await finish(ok: ok)
    }

    /// Pure TelemetryLinkPolicy check, no store/stub involved: CAN-quiet trips backoff, and
    /// advancing simulated time by exactly `TelemetryLinkPolicy.backoffScheduleS`'s own deltas
    /// yields exactly one `.open` per elapsed delay -- never early, never more than one per step
    /// -- proving the reconnect attempt count stays bounded across a simulated multi-minute span
    /// instead of storming.
    private static func runBackoffScheduleBoundsCheck() -> Bool {
        var state = TelemetryLinkPolicy.State()
        let input = TelemetryLinkPolicy.Input(gateOpen: true)
        var now: TimeInterval = 0
        var openCount = 0
        var failures: [String] = []

        if TelemetryLinkPolicy.tick(state: &state, input: input, linkState: .idle, now: now) == .open {
            openCount += 1
        } else {
            failures.append("initial tick did not open")
        }
        _ = TelemetryLinkPolicy.tick(state: &state, input: input, linkState: .ready, now: now)

        var lastCommand: TelemetryLinkPolicy.Command?
        for _ in 0..<TelemetryLinkPolicy.canQuietThreshold {
            lastCommand = TelemetryLinkPolicy.recordPollResult(state: &state, answered: false, now: now)
        }
        if lastCommand != .close {
            failures.append("CAN-quiet did not close on the threshold-th unanswered poll")
        }
        if case .backingOff(let reason, _) = state.phase, reason == .canQuiet {
            // expected
        } else {
            failures.append("phase after CAN-quiet trip was \(state.phase), expected backingOff(.canQuiet)")
        }

        for (index, delay) in TelemetryLinkPolicy.backoffScheduleS.enumerated() {
            let tooEarly = TelemetryLinkPolicy.tick(state: &state, input: input, linkState: .idle, now: now + delay - 1)
            if tooEarly != nil {
                failures.append("attempt \(index) opened before its \(delay)s deadline")
            }
            now += delay
            let onTime = TelemetryLinkPolicy.tick(state: &state, input: input, linkState: .idle, now: now)
            if onTime == .open {
                openCount += 1
            } else {
                failures.append("attempt \(index) did not open at its \(delay)s deadline")
            }
            // The retry itself fails again too, so the schedule advances to its next step --
            // proving escalation, not just one retry.
            _ = TelemetryLinkPolicy.tick(state: &state, input: input, linkState: .backoff(reason: .disconnected), now: now)
        }

        let expectedOpenCount = 1 + TelemetryLinkPolicy.backoffScheduleS.count
        if openCount != expectedOpenCount {
            failures.append("openCount=\(openCount), expected \(expectedOpenCount)")
        }

        let boundedOk = failures.isEmpty
        report("backoff-schedule-bounded", boundedOk, failures.joined(separator: "; "))
        return boundedOk
    }
}
