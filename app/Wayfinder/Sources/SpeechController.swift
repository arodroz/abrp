// Speech playback (wayfinder #68): AVSpeechSynthesizer plus the Apple/CarPlay-mandated audio-
// session recipe for navigation voice prompts (docs/research/turn-by-turn.md §4) -- category
// .playback, mode .voicePrompt, ducking + interrupt-and-mix options, TRANSIENT session
// activation (only while a prompt is in flight, deactivated ~1 s after the last utterance so
// back-to-back prompts don't thrash the shared audio session -- Mapbox's own anti-thrash
// pattern). Decides NOTHING about what/when to speak -- that's VoicePromptScheduler's job; this
// class only speaks what it's told, replacing (never queueing) any in-flight utterance, the same
// "newest prompt wins" policy VoicePromptScheduler expresses at the scheduling layer.
//
// Swift 6 strict concurrency: AVSpeechSynthesizerDelegate callbacks are NOT guaranteed to land
// on main (same pressure as DriveStore's CLLocationManagerDelegate) -- each callback is
// `nonisolated` and hops back with `Task { @MainActor in ... }`, comparing utterance identity via
// `ObjectIdentifier` rather than sending the (non-Sendable) `AVSpeechUtterance` across the hop.
import AVFoundation
import Foundation
import os

@MainActor
final class SpeechController: NSObject, AVSpeechSynthesizerDelegate {
    private static let log = Logger(subsystem: "org.anteras.wayfinder", category: "voice")

    /// Persisted (UserDefaults.standard, key "voiceMuted") so a mute set mid-drive survives a
    /// relaunch. Setting `true` also hard-stops any in-flight utterance immediately.
    var muted: Bool {
        get { UserDefaults.standard.bool(forKey: "voiceMuted") }
        set {
            UserDefaults.standard.set(newValue, forKey: "voiceMuted")
            if newValue { stop() }
        }
    }

    private let synthesizer = AVSpeechSynthesizer()
    /// The one utterance whose delegate callbacks matter -- see `didFinish`/`didCancel` below: a
    /// superseded (replaced) utterance's `didCancel` must do nothing, or un-duck would fire
    /// mid-new-prompt.
    private var currentUtterance: AVSpeechUtterance?
    private var deactivateTask: Task<Void, Never>?

    override init() {
        super.init()
        synthesizer.delegate = self
        synthesizer.usesApplicationAudioSession = true
        // Block-based, NOT selector-based: interruption notifications aren't documented to post
        // on main, and a selector into a @MainActor class traps off-main under Swift 6's dynamic
        // isolation checks -- parse the (non-Sendable) userInfo here, hop with only the result.
        NotificationCenter.default.addObserver(
            forName: AVAudioSession.interruptionNotification, object: nil, queue: nil
        ) { [weak self] notification in
            guard let typeValue = notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
                  AVAudioSession.InterruptionType(rawValue: typeValue) == .began
            else { return }
            Task { @MainActor in self?.handleInterruptionBegan() }
        }
    }

    /// Silent no-op when muted or Apple's Siri/call hint (`promptStyle == .none`) says a full
    /// voice prompt would be inappropriate right now.
    func speak(_ text: String) {
        guard !muted, AVAudioSession.sharedInstance().promptStyle != .none else { return }

        deactivateTask?.cancel()
        deactivateTask = nil
        activateSession()

        if synthesizer.isSpeaking {
            synthesizer.stopSpeaking(at: .immediate)
        }

        let utterance = AVSpeechUtterance(string: text)
        utterance.voice = AVSpeechSynthesisVoice(language: "en-US")
        currentUtterance = utterance
        Self.log.log("speak start: \(text, privacy: .public)")
        synthesizer.speak(utterance)
    }

    func stop() {
        synthesizer.stopSpeaking(at: .immediate)
        scheduleDeactivation()
    }

    private func activateSession() {
        let session = AVAudioSession.sharedInstance()
        do {
            try session.setCategory(.playback, mode: .voicePrompt, options: [.duckOthers, .interruptSpokenAudioAndMixWithOthers])
            try session.setActive(true)
            Self.log.log("session activate")
        } catch {
            Self.log.error("session activate failed: \(error.localizedDescription, privacy: .public)")
        }
    }

    /// Mapbox's anti-thrash pattern (research doc §4): deactivate ~1 s after the last utterance
    /// finishes, not immediately, so back-to-back prompts share one activation instead of
    /// flapping the session. Cancelled if a new prompt arrives in that window (see `speak`).
    private func scheduleDeactivation() {
        deactivateTask?.cancel()
        deactivateTask = Task {
            try? await Task.sleep(nanoseconds: 1_000_000_000)
            guard !Task.isCancelled else { return }
            do {
                try AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
                Self.log.log("deferred deactivate")
            } catch {
                Self.log.error("deferred deactivate failed: \(error.localizedDescription, privacy: .public)")
            }
        }
    }

    /// The system already took the session on `.began` -- no deactivation to schedule, it isn't
    /// ours to deactivate.
    private func handleInterruptionBegan() {
        synthesizer.stopSpeaking(at: .immediate)
    }

    // MARK: AVSpeechSynthesizerDelegate (not guaranteed on main -- see header comment)

    nonisolated func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didFinish utterance: AVSpeechUtterance) {
        // `AVSpeechUtterance` isn't Sendable -- compare identities via `ObjectIdentifier` (which
        // is) rather than sending the utterance itself across the actor hop.
        let utteranceID = ObjectIdentifier(utterance)
        Task { @MainActor in
            Self.log.log("didFinish")
            guard self.currentUtterance.map(ObjectIdentifier.init) == utteranceID else { return }
            self.scheduleDeactivation()
        }
    }

    nonisolated func speechSynthesizer(_ synthesizer: AVSpeechSynthesizer, didCancel utterance: AVSpeechUtterance) {
        let utteranceID = ObjectIdentifier(utterance)
        Task { @MainActor in
            let superseded = self.currentUtterance.map(ObjectIdentifier.init) != utteranceID
            Self.log.log("didCancel(superseded: \(superseded, privacy: .public))")
            guard !superseded else { return }
            self.scheduleDeactivation()
        }
    }
}
