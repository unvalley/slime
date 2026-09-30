import Foundation
import CoreGraphics

enum InputVerification {
    static let defaultsKey = "inputVerificationToken"
    static let modeDefaultsKey = "inputVerificationMode"
    static let targetProcessDefaultsKey = "inputVerificationTargetProcess"

    enum Mode: String {
        case character
        case candidateSelection = "candidate-selection"
        case candidateNumber = "candidate-number"
        case candidateClick = "candidate-click"
        case reconversion
        case privacyPrivate = "privacy-private"
        case privacySecure = "privacy-secure"
        case privacyResume = "privacy-resume"

        var selectionMethod: CandidateSelectionMethod? {
            switch self {
            case .character, .reconversion, .privacyPrivate, .privacySecure, .privacyResume:
                nil
            case .candidateSelection:
                .arrow
            case .candidateNumber:
                .number
            case .candidateClick:
                .click
            }
        }

        func matchesPrivacyState(privateMode: Bool, secureEventInput: Bool) -> Bool {
            switch self {
            case .privacyPrivate:
                privateMode && !secureEventInput
            case .privacySecure:
                secureEventInput
            case .privacyResume:
                !privateMode && !secureEventInput
            case .character, .candidateSelection, .candidateNumber, .candidateClick, .reconversion:
                false
            }
        }
    }

    struct Request: Equatable {
        let token: String
        let mode: Mode
        let targetProcessIdentifier: Int32
    }

    enum CandidateSelectionMethod: Equatable {
        case arrow
        case number
        case click

        var step: CandidateStep {
            switch self {
            case .arrow:
                .candidateArrow
            case .number:
                .candidateNumber
            case .click:
                .candidateClick
            }
        }
    }

    enum CandidateEvent: Equatable {
        case character
        case candidates(selected: Int)
        case selection(CandidateSelectionMethod)
        case accept
        case commit
    }

    enum CandidateStep: String, Equatable {
        case character
        case candidateShown = "candidate-shown"
        case candidateArrow = "candidate-arrow"
        case candidateNumber = "candidate-number"
        case candidateClick = "candidate-click"
        case candidateMoved = "candidate-moved"
        case candidateCommitted = "candidate-committed"
    }

    struct CandidateSelectionState {
        private var receivedCharacter = false
        private var lastSelection: Int?
        private var selectionMethod: CandidateSelectionMethod?
        private var movedSelection = false
        private var acceptedSelection = false
        private var completed = false

        mutating func record(_ event: CandidateEvent) -> CandidateStep? {
            guard !completed else { return nil }
            switch event {
            case .character:
                guard !receivedCharacter else { return nil }
                receivedCharacter = true
                return .character
            case let .candidates(selected):
                guard receivedCharacter else { return nil }
                guard let previousSelection = lastSelection else {
                    lastSelection = selected
                    return .candidateShown
                }
                guard selected != previousSelection,
                      selectionMethod != nil,
                      !movedSelection
                else {
                    return nil
                }
                lastSelection = selected
                movedSelection = true
                return .candidateMoved
            case let .selection(method):
                guard lastSelection != nil,
                      selectionMethod == nil,
                      !movedSelection
                else {
                    return nil
                }
                selectionMethod = method
                return method.step
            case .accept:
                guard movedSelection, !acceptedSelection else { return nil }
                acceptedSelection = true
                return nil
            case .commit:
                guard movedSelection, acceptedSelection else { return nil }
                completed = true
                return .candidateCommitted
            }
        }
    }

    enum ReconversionEvent: Equatable {
        case requested
        case started
        case candidates
        case accept
        case commit
    }

    enum ReconversionStep: String, Equatable {
        case requested = "reconversion-requested"
        case started = "reconversion-started"
        case candidates = "candidate-shown"
        case committed = "reconversion-committed"
    }

    struct ReconversionState {
        private var requested = false
        private var started = false
        private var showedCandidates = false
        private var acceptedCandidate = false
        private var completed = false

        mutating func record(_ event: ReconversionEvent) -> ReconversionStep? {
            guard !completed else { return nil }
            switch event {
            case .requested:
                guard !requested else { return nil }
                requested = true
                return .requested
            case .started:
                guard requested, !started else { return nil }
                started = true
                return .started
            case .candidates:
                guard started, !showedCandidates else { return nil }
                showedCandidates = true
                return .candidates
            case .accept:
                guard showedCandidates, !acceptedCandidate else { return nil }
                acceptedCandidate = true
                return nil
            case .commit:
                guard showedCandidates, acceptedCandidate else { return nil }
                completed = true
                return .committed
            }
        }
    }

    enum PrivacyEvent: Equatable {
        case character
        case requestCandidates
        case candidates
        case accept
        case commit
    }

    enum PrivacyStep: String, Equatable {
        case character
        case candidates = "candidate-shown"
        case committed = "privacy-committed"
    }

    struct PrivacyState {
        private var receivedCharacter = false
        private var requestedCandidates = false
        private var showedCandidates = false
        private var acceptedCandidate = false
        private var completed = false

        mutating func record(_ event: PrivacyEvent) -> PrivacyStep? {
            guard !completed else { return nil }
            switch event {
            case .character:
                guard !receivedCharacter else { return nil }
                receivedCharacter = true
                return .character
            case .requestCandidates:
                guard receivedCharacter, !requestedCandidates else { return nil }
                requestedCandidates = true
                return nil
            case .candidates:
                guard requestedCandidates, !showedCandidates else { return nil }
                showedCandidates = true
                return .candidates
            case .accept:
                guard showedCandidates, !acceptedCandidate else { return nil }
                acceptedCandidate = true
                return nil
            case .commit:
                guard showedCandidates, acceptedCandidate else { return nil }
                completed = true
                return .committed
            }
        }
    }

    static func normalizedToken(_ rawValue: String?) -> String? {
        guard let rawValue,
              let uuid = UUID(uuidString: rawValue),
              uuid.uuidString.caseInsensitiveCompare(rawValue) == .orderedSame
        else {
            return nil
        }
        return uuid.uuidString.lowercased()
    }

    static func pendingToken(defaults: UserDefaults = .standard) -> String? {
        normalizedToken(defaults.string(forKey: defaultsKey))
    }

    static func pendingRequest(defaults: UserDefaults = .standard) -> Request? {
        guard let token = pendingToken(defaults: defaults) else { return nil }
        let rawTarget = defaults.integer(forKey: targetProcessDefaultsKey)
        guard let targetProcessIdentifier = Int32(exactly: rawTarget),
              targetProcessIdentifier > 0
        else {
            return nil
        }
        let mode: Mode
        if let rawMode = defaults.string(forKey: modeDefaultsKey) {
            guard let parsedMode = Mode(rawValue: rawMode) else { return nil }
            mode = parsedMode
        } else {
            mode = .character
        }
        return Request(
            token: token,
            mode: mode,
            targetProcessIdentifier: targetProcessIdentifier
        )
    }

    static func isVerificationCharacter(
        _ characters: String?,
        hasDisallowedModifiers: Bool
    ) -> Bool {
        guard !hasDisallowedModifiers,
              let characters,
              characters.unicodeScalars.count == 1,
              let scalar = characters.unicodeScalars.first
        else {
            return false
        }
        return (0x41 ... 0x5A).contains(scalar.value)
            || (0x61 ... 0x7A).contains(scalar.value)
    }

    static func isHardwareEventSource(_ sourceStateID: Int64?) -> Bool {
        sourceStateID == Int64(CGEventSourceStateID.hidSystemState.rawValue)
    }

    static func consume(_ token: String, defaults: UserDefaults = .standard) {
        guard pendingToken(defaults: defaults) == token else { return }
        defaults.removeObject(forKey: defaultsKey)
        defaults.removeObject(forKey: modeDefaultsKey)
        defaults.removeObject(forKey: targetProcessDefaultsKey)
        _ = defaults.synchronize()
    }
}
