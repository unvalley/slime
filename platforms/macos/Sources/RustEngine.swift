import Foundation

final class LiveNeuralTask: @unchecked Sendable {
    fileprivate let handle: OpaquePointer
    private(set) var runStatus: UInt32?
    /// Read once at creation: `run()` mutates the snapshot on a worker.
    let readingCharacterCount: Int

    fileprivate init(handle: OpaquePointer) {
        self.handle = handle
        readingCharacterCount = Int(slime_live_neural_task_reading_character_count(handle))
    }

    deinit {
        slime_live_neural_task_destroy(handle)
    }

    @discardableResult
    func run() -> Bool {
        let status = slime_live_neural_task_run(handle)
        runStatus = status
        return status == SLIME_STATUS_OK.rawValue
    }
}

final class RustEngine {
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
            let kind: SlimeEventKind = switch self {
            case .character: SLIME_EVENT_CHARACTER
            case .space: SLIME_EVENT_SPACE
            case .enter: SLIME_EVENT_ENTER
            case .escape: SLIME_EVENT_ESCAPE
            case .backspace: SLIME_EVENT_BACKSPACE
            case .nextCandidate: SLIME_EVENT_NEXT_CANDIDATE
            case .previousCandidate: SLIME_EVENT_PREVIOUS_CANDIDATE
            case .selectCandidate: SLIME_EVENT_SELECT_CANDIDATE
            case .acceptCandidate: SLIME_EVENT_ACCEPT_CANDIDATE
            case .transformHiragana: SLIME_EVENT_TRANSFORM_HIRAGANA
            case .transformFullKatakana: SLIME_EVENT_TRANSFORM_FULL_KATAKANA
            case .transformHalfKatakana: SLIME_EVENT_TRANSFORM_HALF_KATAKANA
            case .transformFullAlphanumeric: SLIME_EVENT_TRANSFORM_FULL_ALPHANUMERIC
            case .transformHalfAlphanumeric: SLIME_EVENT_TRANSFORM_HALF_ALPHANUMERIC
            case .nextSegment: SLIME_EVENT_NEXT_SEGMENT
            case .previousSegment: SLIME_EVENT_PREVIOUS_SEGMENT
            case .expandSegment: SLIME_EVENT_EXPAND_SEGMENT
            case .shrinkSegment: SLIME_EVENT_SHRINK_SEGMENT
            }
            return kind.rawValue
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
    private(set) var neuralRerankerStatus: UInt32?

    var hasNeuralReranker: Bool {
        neuralRerankerStatus == SLIME_STATUS_OK.rawValue
    }

    init(
        dataDirectory: URL = UserDataStore.shared.directoryURL,
        dictionaryPackVerificationKeys: String? = nil,
        dictionaryPackVersionFloors: String? = nil,
        neuralModelURL: URL? = nil,
        neuralLambda: Double? = nil,
        neuralMaxCostGap: Int32? = nil,
        loadBundledNeuralReranker: Bool = true
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
        self.handle = handle
        let bundledModelURL = loadBundledNeuralReranker
            ? (Bundle.main.object(
                forInfoDictionaryKey: "SlimeNeuralModelResource"
            ) as? String).flatMap { resource in
                Bundle.main.url(forResource: resource, withExtension: nil)
            }
            : nil
        if let modelURL = neuralModelURL ?? bundledModelURL {
            let modelPath = Array(modelURL.path.utf8)
            let weight = neuralLambda
                ?? (Bundle.main.object(forInfoDictionaryKey: "SlimeNeuralLambda") as? NSNumber)?
                    .doubleValue
                ?? 0.2
            let maxCostGap = neuralMaxCostGap
                ?? (Bundle.main.object(
                    forInfoDictionaryKey: "SlimeNeuralMaxCostGap"
                ) as? NSNumber)?.int32Value
                ?? 1_000
            neuralRerankerStatus = modelPath.withUnsafeBufferPointer { buffer in
                slime_enable_neural_reranker_with_cost_gap(
                    handle,
                    buffer.baseAddress,
                    buffer.count,
                    weight,
                    maxCostGap
                )
            }
            if neuralRerankerStatus == SLIME_STATUS_OK.rawValue,
               neuralLambda == nil,
               neuralMaxCostGap == nil,
               let explicitGap = (Bundle.main.object(
                   forInfoDictionaryKey: "SlimeNeuralExplicitMaxCostGap"
               ) as? NSNumber)?.int32Value {
                _ = slime_set_explicit_neural_cost_gap(handle, explicitGap)
            }
            if neuralRerankerStatus == SLIME_STATUS_OK.rawValue,
               neuralLambda == nil,
               neuralMaxCostGap == nil,
               let confidence = (Bundle.main.object(
                   forInfoDictionaryKey: "SlimeNeuralExplicitConfidenceEnabled"
               ) as? NSNumber)?.boolValue {
                _ = slime_set_explicit_neural_confidence(handle, confidence)
            }
            if neuralRerankerStatus == SLIME_STATUS_OK.rawValue,
               neuralLambda == nil,
               neuralMaxCostGap == nil,
               let agreement = (Bundle.main.object(
                   forInfoDictionaryKey: "SlimeNeuralExplicitLiveAgreementEnabled"
               ) as? NSNumber)?.boolValue {
                _ = slime_set_explicit_live_agreement(handle, agreement)
            }
            if neuralRerankerStatus == SLIME_STATUS_OK.rawValue,
               neuralLambda == nil,
               let explicitWeight = (Bundle.main.object(
                   forInfoDictionaryKey: "SlimeNeuralExplicitLongLambda"
               ) as? NSNumber)?.doubleValue,
               let minimum = (Bundle.main.object(
                   forInfoDictionaryKey: "SlimeNeuralExplicitMinimumCharacters"
               ) as? NSNumber)?.intValue,
               minimum >= 0 {
                // This override applies only to explicit Space conversion.
                // The runtime's existing LIVE weights remain unchanged.
                _ = slime_set_explicit_neural_long_reading_weight(
                    handle, minimum, explicitWeight
                )
            }
            if neuralRerankerStatus == SLIME_STATUS_OK.rawValue,
               neuralLambda == nil,
               let mediumWeight = (Bundle.main.object(
                   forInfoDictionaryKey: "SlimeNeuralExplicitMediumLambda"
               ) as? NSNumber)?.doubleValue {
                _ = slime_set_explicit_neural_medium_reading_weight(handle, mediumWeight)
            }
        } else {
            neuralRerankerStatus = nil
        }
    }

    deinit {
        slime_destroy(handle)
    }

    func process(_ event: Event) throws -> [Action] {
        try collectActions { context in
            slime_process_actions_v2(
                handle,
                event.rawValue,
                event.scalar,
                context,
                collectTypedAction
            )
        }
    }

    func makeLiveNeuralTask(
        minimumSwitchMargin: Double,
        longReadingMinimumSwitchMargin: Double,
        numericBaseSwitchMargin: Double,
        longReadingLambda: Double
    ) -> LiveNeuralTask? {
        guard let task = slime_live_neural_task_create_v2(
            handle,
            minimumSwitchMargin,
            longReadingMinimumSwitchMargin,
            numericBaseSwitchMargin,
            longReadingLambda
        ) else {
            return nil
        }
        return LiveNeuralTask(handle: task)
    }

    func setLiveNeuralRerankingEnabled(_ enabled: Bool) throws {
        let status = slime_set_live_neural_ranking_enabled(handle, enabled)
        guard status == SLIME_STATUS_OK.rawValue else {
            throw EngineError.rejected("live_neural_enabled_status_\(status)")
        }
    }

    func apply(_ task: LiveNeuralTask) throws -> [Action] {
        try collectActions { context in
            slime_live_neural_task_apply_actions_v2(
                handle,
                task.handle,
                context,
                collectTypedAction
            )
        }
    }

    private func collectActions(
        _ operation: (_ context: UnsafeMutableRawPointer) -> UInt32
    ) throws -> [Action] {
        let collector = TypedActionCollector()
        let context = Unmanaged.passUnretained(collector).toOpaque()
        let status = operation(context)
        guard status == SLIME_STATUS_OK.rawValue else {
            throw EngineError.rejected("action_status_\(status)")
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
        let bytes = Array(context.utf8)
        let status = bytes.withUnsafeBufferPointer { buffer in
            slime_set_external_left_context(handle, buffer.baseAddress, buffer.count)
        }
        guard status == 0 else {
            throw EngineError.rejected("external_left_context_status_\(status)")
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
        // size_t imports as Int, so SLIME_NO_SELECTION (SIZE_MAX) reads as -1.
        let hasSelection = UInt(bitPattern: action.selection_start) != UInt.max
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
