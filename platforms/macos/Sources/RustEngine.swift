import Foundation

final class RustEngine {
    enum NeuralProfile: String {
        case balanced
        case highAccuracy = "high-accuracy"

        fileprivate var ffiValue: UInt32 {
            switch self {
            case .balanced: SLIME_NEURAL_PROFILE_BALANCED.rawValue
            case .highAccuracy: SLIME_NEURAL_PROFILE_HIGH_ACCURACY.rawValue
            }
        }
    }

    enum Event {
        case character(Unicode.Scalar)
        case space
        case enter
        case escape
        case backspace
        case nextCandidate
        case previousCandidate
        case selectCandidate(UInt32)
        case acceptCandidate
        case transformHiragana
        case transformFullKatakana
        case transformHalfKatakana
        case transformFullAlphanumeric
        case transformHalfAlphanumeric
        case nextSegment
        case previousSegment
        case expandSegment
        case shrinkSegment

        fileprivate var rawValue: UInt32 {
            switch self {
            case .character: 0
            case .space: 1
            case .enter: 2
            case .escape: 3
            case .backspace: 4
            case .nextCandidate: 5
            case .previousCandidate: 6
            case .selectCandidate: 7
            case .acceptCandidate: 8
            case .transformHiragana: 9
            case .transformFullKatakana: 10
            case .transformHalfKatakana: 11
            case .transformFullAlphanumeric: 12
            case .transformHalfAlphanumeric: 13
            case .nextSegment: 14
            case .previousSegment: 15
            case .expandSegment: 16
            case .shrinkSegment: 17
            }
        }

        fileprivate var scalar: UInt32 {
            switch self {
            case let .character(value): value.value
            case let .selectCandidate(index): index
            default: 0
            }
        }
    }

    struct Action: Decodable, Equatable {
        let type: String
        let text: String?
        let candidates: [String]?
        let candidateDetails: [CandidateDetail]?
        let selected: Int?
        let selectedStart: Int?
        let selectedLength: Int?
    }

    struct CandidateDetail: Decodable, Equatable {
        let value: String
        let annotation: UInt32
        let detail: String?
    }

    enum EngineError: Error, Equatable {
        case creationFailed
        case invalidBuffer
        case rejected(String)
    }

    private struct Response: Decodable {
        let ok: Bool
        let actions: [Action]?
        let error: String?
    }

    private let handle: OpaquePointer

    init(
        dataDirectory: URL = UserDataStore.shared.directoryURL,
        dictionaryPackVerificationKeys: String? = nil,
        dictionaryPackVersionFloors: String? = nil,
        neuralModelURL: URL? = nil,
        neuralProfile: NeuralProfile? = nil
    ) throws {
        let path = Array(dataDirectory.path.utf8)
        let configuredKeys = dictionaryPackVerificationKeys
            ?? (Bundle.main.object(
                forInfoDictionaryKey: "SlimeDictionaryPackVerificationKeys"
            ) as? String)
        let configuredVersionFloors = dictionaryPackVersionFloors
            ?? (Bundle.main.object(
                forInfoDictionaryKey: "SlimeDictionaryPackVersionFloors"
            ) as? String)
        let createdHandle: OpaquePointer? = path.withUnsafeBufferPointer { pathBuffer in
            guard let configuredKeys, !configuredKeys.isEmpty else {
                guard configuredVersionFloors?.isEmpty != false else {
                    return nil
                }
                return slime_create_with_data_dir(pathBuffer.baseAddress, pathBuffer.count)
            }
            let keys = Array(configuredKeys.utf8)
            return keys.withUnsafeBufferPointer { keyBuffer in
                guard let configuredVersionFloors, !configuredVersionFloors.isEmpty else {
                    return slime_create_with_signed_data_dir(
                        pathBuffer.baseAddress,
                        pathBuffer.count,
                        keyBuffer.baseAddress,
                        keyBuffer.count
                    )
                }
                let versionFloors = Array(configuredVersionFloors.utf8)
                return versionFloors.withUnsafeBufferPointer { floorBuffer in
                    slime_create_with_signed_data_dir_and_version_floors(
                        pathBuffer.baseAddress,
                        pathBuffer.count,
                        keyBuffer.baseAddress,
                        keyBuffer.count,
                        floorBuffer.baseAddress,
                        floorBuffer.count
                    )
                }
            }
        }
        guard let handle = createdHandle else {
            throw EngineError.creationFailed
        }
        let configuredNeuralModel = neuralModelURL
            ?? Bundle.main.url(forResource: "SlimeNeuralModel", withExtension: "gguf")
        if let configuredNeuralModel {
            let configuredNeuralProfile = neuralProfile
                ?? (Bundle.main.object(forInfoDictionaryKey: "SlimeNeuralProfile") as? String)
                    .flatMap(NeuralProfile.init(rawValue:))
                ?? .balanced
            let modelPath = Array(configuredNeuralModel.path.utf8)
            let status = modelPath.withUnsafeBufferPointer { buffer in
                slime_enable_neural_rescoring_with_profile(
                    handle,
                    buffer.baseAddress,
                    buffer.count,
                    configuredNeuralProfile.ffiValue
                )
            }
            guard status == SLIME_STATUS_OK.rawValue else {
                slime_destroy(handle)
                throw EngineError.rejected("neural_model_status_\(status)")
            }
        }
        self.handle = handle
    }

    deinit {
        slime_destroy(handle)
    }

    func process(_ event: Event) throws -> [Action] {
        let collector = TypedActionCollector()
        let context = Unmanaged.passUnretained(collector).toOpaque()
        let status = slime_process_actions_v2(
            handle,
            event.rawValue,
            event.scalar,
            context,
            collectTypedAction
        )
        guard status == SLIME_STATUS_OK.rawValue else {
            throw EngineError.rejected("process_status_\(status)")
        }
        if let unsupportedKind = collector.unsupportedKind {
            throw EngineError.rejected("unsupported_action_\(unsupportedKind)")
        }
        return collector.actions
    }

    func setOptions(
        liveConversion: Bool,
        historyCompletion: Bool,
        historyLearning: Bool? = nil,
        dictionaryPacks: UInt32 = 0,
        privateMode: Bool = false,
        dateFormatMask: UInt32 = DateCandidateFormat.allMask
    ) throws -> [Action] {
        let buffer = slime_set_options_v5(
            handle,
            liveConversion,
            historyCompletion,
            historyLearning ?? historyCompletion,
            dictionaryPacks,
            privateMode,
            dateFormatMask
        )
        return try decode(buffer)
    }

    func beginReconversion(surface: String) throws -> [Action] {
        let bytes = Array(surface.utf8)
        let buffer = bytes.withUnsafeBufferPointer { buffer in
            slime_begin_reconversion(handle, buffer.baseAddress, buffer.count)
        }
        return try decode(buffer)
    }

    func resetContext() throws {
        let status = slime_reset_context(handle)
        guard status == 0 else {
            throw EngineError.rejected("reset_context_status_\(status)")
        }
    }

    func setExternalLeftContext(_ context: String) throws {
        try setExternalContext(left: context, right: "")
    }

    func setExternalContext(left: String, right: String) throws {
        let leftBytes = Array(left.utf8)
        let rightBytes = Array(right.utf8)
        let status = leftBytes.withUnsafeBufferPointer { leftBuffer in
            rightBytes.withUnsafeBufferPointer { rightBuffer in
                slime_set_external_context(
                    handle,
                    leftBuffer.baseAddress,
                    leftBuffer.count,
                    rightBuffer.baseAddress,
                    rightBuffer.count
                )
            }
        }
        guard status == 0 else {
            throw EngineError.rejected("external_context_status_\(status)")
        }
    }

    func reloadUserData() throws -> [Action] {
        let buffer = slime_reload_user_data(handle)
        return try decode(buffer)
    }

    static func domainDictionaryWords(mask: UInt32) throws -> [DomainDictionaryWord] {
        struct WordsResponse: Decodable {
            let ok: Bool
            let words: [DomainDictionaryWord]?
            let error: String?
        }

        let buffer = slime_domain_dictionary_words(mask)
        defer { slime_buffer_destroy(buffer) }

        guard let bytes = buffer.data, buffer.len > 0 else {
            throw EngineError.invalidBuffer
        }

        let data = Data(bytes: bytes, count: buffer.len)
        let response = try JSONDecoder().decode(WordsResponse.self, from: data)
        guard response.ok else {
            throw EngineError.rejected(response.error ?? "unknown_error")
        }
        return response.words ?? []
    }

    func installedDictionaryPacks() throws -> InstalledDictionaryPackCatalog {
        struct CatalogResponse: Decodable {
            let ok: Bool
            let packs: [InstalledDictionaryPack]?
            let errors: [DictionaryPackLoadIssue]?
            let error: String?
        }

        let buffer = slime_installed_dictionary_packs(handle)
        defer { slime_buffer_destroy(buffer) }
        guard let bytes = buffer.data, buffer.len > 0 else {
            throw EngineError.invalidBuffer
        }

        let data = Data(bytes: bytes, count: buffer.len)
        let response = try JSONDecoder().decode(CatalogResponse.self, from: data)
        guard response.ok else {
            throw EngineError.rejected(response.error ?? "unknown_error")
        }
        return InstalledDictionaryPackCatalog(
            packs: response.packs ?? [],
            errors: response.errors ?? []
        )
    }

    func installedDictionaryPackWords(id: String) throws -> [DomainDictionaryWord] {
        struct WordsResponse: Decodable {
            let ok: Bool
            let words: [DomainDictionaryWord]?
            let error: String?
        }

        let identifier = Array(id.utf8)
        let buffer = identifier.withUnsafeBufferPointer { bytes in
            slime_installed_dictionary_pack_words(handle, bytes.baseAddress, bytes.count)
        }
        defer { slime_buffer_destroy(buffer) }
        guard let bytes = buffer.data, buffer.len > 0 else {
            throw EngineError.invalidBuffer
        }

        let data = Data(bytes: bytes, count: buffer.len)
        let response = try JSONDecoder().decode(WordsResponse.self, from: data)
        guard response.ok else {
            throw EngineError.rejected(response.error ?? "unknown_error")
        }
        return response.words ?? []
    }

    private func decode(_ buffer: SlimeBuffer) throws -> [Action] {
        defer { slime_buffer_destroy(buffer) }

        guard let bytes = buffer.data, buffer.len > 0 else {
            throw EngineError.invalidBuffer
        }

        let data = Data(bytes: bytes, count: buffer.len)
        let response = try JSONDecoder().decode(Response.self, from: data)
        guard response.ok else {
            throw EngineError.rejected(response.error ?? "unknown_error")
        }
        return response.actions ?? []
    }
}

private final class TypedActionCollector {
    var actions: [RustEngine.Action] = []
    var unsupportedKind: UInt32?
}

private func collectTypedAction(
    context: UnsafeMutableRawPointer?,
    actionPointer: UnsafePointer<SlimeActionViewV2>?
) {
    guard let context, let actionPointer else {
        return
    }
    let collector = Unmanaged<TypedActionCollector>.fromOpaque(context).takeUnretainedValue()
    let action = actionPointer.pointee

    switch action.kind {
    case UInt32(SLIME_ACTION_UPDATE_PREEDIT.rawValue):
        let hasSelection = action.selection_start != .max
        collector.actions.append(
            RustEngine.Action(
                type: "update_preedit",
                text: copyString(action.text),
                candidates: nil,
                candidateDetails: nil,
                selected: nil,
                selectedStart: hasSelection ? action.selection_start : nil,
                selectedLength: hasSelection ? action.selection_length : nil
            )
        )
    case UInt32(SLIME_ACTION_SHOW_CANDIDATES.rawValue):
        let candidateDetails = (0 ..< action.candidate_count).map { index in
            let candidate = action.candidates[index]
            return RustEngine.CandidateDetail(
                value: copyString(candidate.value),
                annotation: candidate.annotation,
                detail: candidate.detail.len == 0 ? nil : copyString(candidate.detail)
            )
        }
        let candidates = (0 ..< action.candidate_count).map { index in
            copyString(action.candidates[index].display)
        }
        collector.actions.append(
            RustEngine.Action(
                type: "show_candidates",
                text: nil,
                candidates: candidates,
                candidateDetails: candidateDetails,
                selected: action.selected,
                selectedStart: nil,
                selectedLength: nil
            )
        )
    case UInt32(SLIME_ACTION_HIDE_CANDIDATES.rawValue):
        collector.actions.append(
            RustEngine.Action(
                type: "hide_candidates",
                text: nil,
                candidates: nil,
                candidateDetails: nil,
                selected: nil,
                selectedStart: nil,
                selectedLength: nil
            )
        )
    case UInt32(SLIME_ACTION_COMMIT.rawValue):
        collector.actions.append(
            RustEngine.Action(
                type: "commit",
                text: copyString(action.text),
                candidates: nil,
                candidateDetails: nil,
                selected: nil,
                selectedStart: nil,
                selectedLength: nil
            )
        )
    case UInt32(SLIME_ACTION_CLEAR.rawValue):
        collector.actions.append(
            RustEngine.Action(
                type: "clear",
                text: nil,
                candidates: nil,
                candidateDetails: nil,
                selected: nil,
                selectedStart: nil,
                selectedLength: nil
            )
        )
    case UInt32(SLIME_ACTION_FORWARD_KEY.rawValue):
        collector.actions.append(
            RustEngine.Action(
                type: "forward_key",
                text: nil,
                candidates: nil,
                candidateDetails: nil,
                selected: nil,
                selectedStart: nil,
                selectedLength: nil
            )
        )
    default:
        collector.unsupportedKind = action.kind
    }
}

private func copyString(_ view: SlimeStringView) -> String {
    String(
        decoding: UnsafeBufferPointer(start: view.data, count: view.len),
        as: UTF8.self
    )
}
