import Foundation

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
        let selected: Int?
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

    init(dataDirectory: URL = UserDataStore.shared.directoryURL) throws {
        let path = Array(dataDirectory.path.utf8)
        let createdHandle = path.withUnsafeBufferPointer { buffer in
            slime_create_with_data_dir(buffer.baseAddress, buffer.count)
        }
        guard let handle = createdHandle else {
            throw EngineError.creationFailed
        }
        self.handle = handle
    }

    deinit {
        slime_destroy(handle)
    }

    func process(_ event: Event) throws -> [Action] {
        let buffer = slime_process(handle, event.rawValue, event.scalar)
        return try decode(buffer)
    }

    func setOptions(
        liveConversion: Bool,
        historyCompletion: Bool,
        historyLearning: Bool? = nil,
        dictionaryPacks: UInt32 = 0
    ) throws -> [Action] {
        let buffer = slime_set_options_v3(
            handle,
            liveConversion,
            historyCompletion,
            historyLearning ?? historyCompletion,
            dictionaryPacks
        )
        return try decode(buffer)
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
