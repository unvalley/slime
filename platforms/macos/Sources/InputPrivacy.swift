import Carbon
import Foundation

struct InputRuntimeOptions: Equatable {
    let liveConversion: Bool
    let historyCompletion: Bool
    let historyLearning: Bool
    let typoCorrectionEnabled: Bool
    let dictionaryPacks: UInt32
    let privateMode: Bool
    let dateFormatMask: UInt32

    init(
        liveConversion: Bool,
        historyCompletion: Bool,
        historyLearning: Bool,
        dictionaryPacks: UInt32,
        secureEventInput: Bool,
        privateMode: Bool = InputPrivacySession.isPrivate,
        dateFormatMask: UInt32 = DateCandidateFormat.allMask,
        typoCorrectionEnabled: Bool = false
    ) {
        self.liveConversion = liveConversion
        self.privateMode = privateMode || secureEventInput
        self.historyCompletion = historyCompletion && !self.privateMode
        self.historyLearning = historyLearning && !self.privateMode
        self.typoCorrectionEnabled = typoCorrectionEnabled
        self.dictionaryPacks = dictionaryPacks
        self.dateFormatMask = dateFormatMask
    }
}

enum InputPrivacySession {
    private(set) static var isPrivate = false

    static func toggle() {
        isPrivate.toggle()
        NotificationCenter.default.post(name: .unvalleyPreferencesDidChange, object: nil)
    }
}

/// HIToolbox documents this process-wide query as not thread-safe. InputMethodKit
/// normally calls the controller on the main thread; if it does not, fail closed
/// and pause learning for that event instead of inspecting unsafe global state.
func secureEventInputIsEnabled() -> Bool {
    guard Thread.isMainThread else {
        return true
    }
    return IsSecureEventInputEnabled()
}
