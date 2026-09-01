// Drives any TelemetryDialogue through any TelemetryLink (wayfinder #78): writes each
// `takeOutgoing()` chunk, feeds arriving bytes back in, and calls `onTimeout()` when nothing
// arrives within `stepTimeoutS`. Confines every `TelemetryDialogue` method call to this
// function's own execution -- incoming bytes may arrive on an arbitrary queue (CxBleLink's
// private CB queue; see TelemetryLink's header), so rather than requiring every
// `TelemetryDialogue` conformance to be internally thread-safe, incoming chunks are first
// posted to a lock-guarded mailbox and only drained (and fed to the dialogue) from this
// function's own poll loop -- the same "poll a condition on a timer" idiom Autotest's
// `waitWithTimeout` already uses, not hand-rolled continuation/actor plumbing, since this runs a
// handful of times per connection, not on any hot path.
//
// `run` is @MainActor: its only caller is TelemetryLinkStore (also @MainActor), and neither
// TelemetryDialogue nor TelemetryLink is Sendable, so driving them from a nonisolated function
// would require "sending" non-Sendable existentials across an actor boundary that doesn't
// actually need to exist -- staying on the caller's actor is the honest fix, not a workaround.
import Foundation

enum TelemetryPump {
    // Deliberately NOT @MainActor -- unlike `run` below, this must stay callable from whatever
    // arbitrary queue a TelemetryLink's `onIncoming` fires on (CxBleLink: its private CB queue).
    private final class Mailbox: @unchecked Sendable {
        private let lock = NSLock()
        private var chunks: [Data] = []

        func post(_ data: Data) {
            lock.lock()
            chunks.append(data)
            lock.unlock()
        }

        func drain() -> [Data] {
            lock.lock()
            defer { lock.unlock() }
            let result = chunks
            chunks = []
            return result
        }
    }

    /// Drives `dialogue` to completion over `link`. Installs itself as `link.onIncoming` for the
    /// run's duration, CHAINING onto whatever was already there (TelemetryLinkStore's own
    /// permanent chunk-counting subscriber) rather than replacing it, and restores the previous
    /// value on return. Returns whether any bytes were received during the run --
    /// TelemetryLinkStore's CAN-quiet "answered" signal to `TelemetryLinkPolicy.recordPollResult`,
    /// returned directly rather than via a separately-observed counter so the decision can never
    /// race a counter update on another hop.
    @MainActor
    @discardableResult
    static func run(dialogue: TelemetryDialogue, link: TelemetryLink, stepTimeoutS: TimeInterval = 2.0) async -> Bool {
        let mailbox = Mailbox()
        let previous = link.onIncoming
        link.onIncoming = { data in
            previous?(data)
            mailbox.post(data)
        }
        defer { link.onIncoming = previous }

        var receivedAnything = false

        while !dialogue.isFinished {
            if let outgoing = dialogue.takeOutgoing() {
                link.send(outgoing)
                for chunk in mailbox.drain() {
                    receivedAnything = true
                    dialogue.feed(chunk)
                }
                continue
            }
            if dialogue.isFinished { break }

            var receivedThisWait = false
            let deadline = Date().addingTimeInterval(stepTimeoutS)
            while !dialogue.isFinished, Date() < deadline {
                let chunks = mailbox.drain()
                if !chunks.isEmpty {
                    receivedThisWait = true
                    receivedAnything = true
                    for chunk in chunks { dialogue.feed(chunk) }
                    break
                }
                try? await Task.sleep(nanoseconds: 20_000_000)
            }
            if !receivedThisWait, !dialogue.isFinished {
                dialogue.onTimeout()
            }
        }

        return receivedAnything
    }
}
