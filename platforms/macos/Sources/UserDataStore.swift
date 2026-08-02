import AppKit
import Foundation

extension Notification.Name {
    static let unvalleyPreferencesDidChange = Notification.Name(
        "com.unvalley.inputmethod.preferences-did-change"
    )
    static let unvalleyUserDataDidChange = Notification.Name(
        "com.unvalley.inputmethod.user-data-did-change"
    )
}

enum IMEPreferences {
    private static let liveConversionKey = "liveConversion"
    private static let historyCompletionKey = "historyCompletion"
    private static let historyLearningKey = "historyLearning"
    private static let dictionaryPacksKey = "dictionaryPacks"
    private static let dateCandidateFormatsKey = "dateCandidateFormats"

    static var liveConversion: Bool {
        get { UserDefaults.standard.bool(forKey: liveConversionKey) }
        set {
            UserDefaults.standard.set(newValue, forKey: liveConversionKey)
            NotificationCenter.default.post(name: .unvalleyPreferencesDidChange, object: nil)
        }
    }

    static var historyCompletion: Bool {
        get { UserDefaults.standard.bool(forKey: historyCompletionKey) }
        set {
            if UserDefaults.standard.object(forKey: historyLearningKey) == nil {
                UserDefaults.standard.set(newValue, forKey: historyLearningKey)
            }
            UserDefaults.standard.set(newValue, forKey: historyCompletionKey)
            NotificationCenter.default.post(name: .unvalleyPreferencesDidChange, object: nil)
        }
    }

    static var historyLearning: Bool {
        get {
            guard UserDefaults.standard.object(forKey: historyLearningKey) != nil else {
                return historyCompletion
            }
            return UserDefaults.standard.bool(forKey: historyLearningKey)
        }
        set {
            UserDefaults.standard.set(newValue, forKey: historyLearningKey)
            NotificationCenter.default.post(name: .unvalleyPreferencesDidChange, object: nil)
        }
    }

    static var dictionaryPacks: UInt32 {
        get { UInt32(truncatingIfNeeded: UserDefaults.standard.integer(forKey: dictionaryPacksKey)) }
        set {
            UserDefaults.standard.set(Int(newValue), forKey: dictionaryPacksKey)
            NotificationCenter.default.post(name: .unvalleyPreferencesDidChange, object: nil)
        }
    }

    static var dateCandidateFormats: UInt32 {
        get {
            guard UserDefaults.standard.object(forKey: dateCandidateFormatsKey) != nil else {
                return DateCandidateFormat.allMask
            }
            return UInt32(
                truncatingIfNeeded: UserDefaults.standard.integer(forKey: dateCandidateFormatsKey)
            )
        }
        set {
            UserDefaults.standard.set(Int(newValue), forKey: dateCandidateFormatsKey)
            NotificationCenter.default.post(name: .unvalleyPreferencesDidChange, object: nil)
        }
    }
}

enum DateCandidateFormat: UInt32, CaseIterable, Identifiable {
    case shortNumeric = 1
    case isoNumeric = 2
    case monthDayWeekday = 4
    case longGregorian = 8
    case longReiwa = 16
    case shortReiwa = 32
    case weekday = 64

    static let allMask = allCases.reduce(UInt32(0)) { $0 | $1.rawValue }

    var id: UInt32 { rawValue }

    var name: String {
        return switch self {
        case .shortNumeric: "月/日"
        case .isoNumeric: "年/月/日"
        case .monthDayWeekday: "月日と曜日"
        case .longGregorian: "西暦の年月日"
        case .longReiwa: "和暦の年月日"
        case .shortReiwa: "和暦の短縮表記"
        case .weekday: "曜日"
        }
    }

    var example: String {
        let calendar = Calendar(identifier: .gregorian)
        let components = calendar.dateComponents([.year, .month, .day, .weekday], from: Date())
        let year = components.year ?? 1970
        let month = components.month ?? 1
        let day = components.day ?? 1
        let weekdayIndex = max(0, min(6, (components.weekday ?? 1) - 1))
        let shortWeekday = ["日", "月", "火", "水", "木", "金", "土"][weekdayIndex]
        let weekday = [
            "日曜日", "月曜日", "火曜日", "水曜日", "木曜日", "金曜日", "土曜日",
        ][weekdayIndex]
        let reiwaYear = max(1, year - 2018)
        return switch self {
        case .shortNumeric: "\(month)/\(day)"
        case .isoNumeric: String(format: "%04d/%02d/%02d", year, month, day)
        case .monthDayWeekday: "\(month)月\(day)日(\(shortWeekday))"
        case .longGregorian: "\(year)年\(month)月\(day)日"
        case .longReiwa:
            "令和\(reiwaYear == 1 ? "元" : String(reiwaYear))年\(month)月\(day)日"
        case .shortReiwa: String(format: "R%02d/%02d/%02d", reiwaYear, month, day)
        case .weekday: weekday
        }
    }
}

struct UserDictionaryEntry: Identifiable, Hashable {
    let id: UUID
    var reading: String
    var surface: String

    init(id: UUID = UUID(), reading: String, surface: String) {
        self.id = id
        self.reading = reading
        self.surface = surface
    }
}

struct DomainDictionaryWord: Decodable, Identifiable, Equatable {
    let reading: String
    let surface: String
    var id: String { "\(reading)\u{0}\(surface)" }
}

struct InstalledDictionaryPack: Decodable, Identifiable, Equatable {
    let id: String
    let formatVersion: Int?
    let name: String
    let version: String
    let license: String
    let minimumSlimeVersion: String?
    let publishedAt: String?
    let provenance: String?
    let entriesSHA256: String?
    let entryCount: Int

    init(
        id: String,
        formatVersion: Int? = nil,
        name: String,
        version: String,
        license: String,
        minimumSlimeVersion: String? = nil,
        publishedAt: String? = nil,
        provenance: String? = nil,
        entriesSHA256: String? = nil,
        entryCount: Int
    ) {
        self.id = id
        self.formatVersion = formatVersion
        self.name = name
        self.version = version
        self.license = license
        self.minimumSlimeVersion = minimumSlimeVersion
        self.publishedAt = publishedAt
        self.provenance = provenance
        self.entriesSHA256 = entriesSHA256
        self.entryCount = entryCount
    }
}

struct DictionaryPackLoadIssue: Decodable, Equatable {
    let file: String
    let message: String
}

struct InstalledDictionaryPackCatalog: Decodable, Equatable {
    let packs: [InstalledDictionaryPack]
    let errors: [DictionaryPackLoadIssue]
}

struct InputHistoryEntry: Identifiable, Hashable {
    var id: String { "\(reading)\u{0}\(surface)" }
    let reading: String
    let surface: String
    let count: UInt32
    let lastUsed: Date

    var isUsefulForCompletion: Bool {
        let readingLength = reading.unicodeScalars.count
        let surfaceLength = surface.unicodeScalars.count
        return (3 ... 64).contains(readingLength)
            && (2 ... 128).contains(surfaceLength)
            && reading != surface
            && reading.unicodeScalars.contains { scalar in
                (0x3040 ... 0x30FF).contains(scalar.value)
                    || (0x3400 ... 0x9FFF).contains(scalar.value)
            }
    }
}

enum UserDataStoreError: LocalizedError {
    case malformedFile(String)
    case externallyModified
    case invalidEntry

    var errorDescription: String? {
        switch self {
        case let .malformedFile(name):
            "\(name)の形式が壊れています。元のファイルは変更していません。"
        case .externallyModified:
            "データが入力中または別の場所で変更されました。再読み込みしてから操作してください。"
        case .invalidEntry:
            "読みと単語を入力してください。タブや改行は使用できません。"
        }
    }
}

final class UserDataStore {
    static let shared = UserDataStore()

    let directoryURL: URL
    let dictionaryURL: URL
    let historyURL: URL

    private let dictionaryHeader = "# slime-user-dictionary-v1\n"
    private let historyHeader = "# slime-history-v1\n"

    private convenience init(fileManager: FileManager = .default) {
        let applicationSupport = fileManager.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first!
        self.init(directoryURL: applicationSupport.appendingPathComponent(
            "Slime",
            isDirectory: true
        ))
    }

    init(directoryURL: URL) {
        self.directoryURL = directoryURL
        dictionaryURL = directoryURL.appendingPathComponent("user_dictionary.tsv")
        historyURL = directoryURL.appendingPathComponent("history.tsv")
    }

    func loadDictionary() throws -> (entries: [UserDictionaryEntry], base: Data?) {
        guard FileManager.default.fileExists(atPath: dictionaryURL.path) else {
            return ([], nil)
        }
        let data = try Data(contentsOf: dictionaryURL)
        guard let text = String(data: data, encoding: .utf8) else {
            throw UserDataStoreError.malformedFile(dictionaryURL.lastPathComponent)
        }

        var entries: [UserDictionaryEntry] = []
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let value = String(line)
            if value.isEmpty || value == dictionaryHeader.trimmingCharacters(in: .newlines) {
                continue
            }
            let columns = value.split(separator: "\t", omittingEmptySubsequences: false)
            guard columns.count == 2, !columns[0].isEmpty, !columns[1].isEmpty else {
                throw UserDataStoreError.malformedFile(dictionaryURL.lastPathComponent)
            }
            entries.append(UserDictionaryEntry(
                reading: String(columns[0]),
                surface: String(columns[1])
            ))
        }
        return (entries, data)
    }

    @discardableResult
    func saveDictionary(
        _ entries: [UserDictionaryEntry],
        replacing base: Data?
    ) throws -> Data {
        let current = try currentData(at: dictionaryURL)
        guard current == base else {
            throw UserDataStoreError.externallyModified
        }
        guard entries.allSatisfy({ isValid($0.reading) && isValid($0.surface) }) else {
            throw UserDataStoreError.invalidEntry
        }

        try FileManager.default.createDirectory(
            at: directoryURL,
            withIntermediateDirectories: true
        )
        let data = try dictionaryData(entries)
        try data.write(to: dictionaryURL, options: .atomic)
        NotificationCenter.default.post(name: .unvalleyUserDataDidChange, object: nil)
        return data
    }

    func dictionaryData(_ entries: [UserDictionaryEntry]) throws -> Data {
        guard entries.allSatisfy({ isValid($0.reading) && isValid($0.surface) }) else {
            throw UserDataStoreError.invalidEntry
        }
        var text = dictionaryHeader
        for entry in entries {
            text += "\(entry.reading)\t\(entry.surface)\n"
        }
        return Data(text.utf8)
    }

    func loadHistory() throws -> [InputHistoryEntry] {
        try loadHistorySnapshot().entries
    }

    func loadHistorySnapshot() throws -> (entries: [InputHistoryEntry], base: Data?) {
        guard FileManager.default.fileExists(atPath: historyURL.path) else {
            return ([], nil)
        }
        let data = try Data(contentsOf: historyURL)
        guard let text = String(data: data, encoding: .utf8) else {
            throw UserDataStoreError.malformedFile(historyURL.lastPathComponent)
        }

        var entries: [InputHistoryEntry] = []
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let value = String(line)
            if value.isEmpty || value == historyHeader.trimmingCharacters(in: .newlines) {
                continue
            }
            let columns = value.split(separator: "\t", omittingEmptySubsequences: false)
            guard columns.count == 4,
                  let count = UInt32(columns[2]),
                  let timestamp = TimeInterval(columns[3])
            else {
                throw UserDataStoreError.malformedFile(historyURL.lastPathComponent)
            }
            entries.append(InputHistoryEntry(
                reading: String(columns[0]),
                surface: String(columns[1]),
                count: count,
                lastUsed: Date(timeIntervalSince1970: timestamp)
            ))
        }
        let sorted = entries.sorted {
            if $0.lastUsed != $1.lastUsed { return $0.lastUsed > $1.lastUsed }
            return $0.count > $1.count
        }
        return (sorted, data)
    }

    @discardableResult
    func removeHistoryEntry(
        _ removed: InputHistoryEntry,
        from entries: [InputHistoryEntry],
        replacing base: Data?
    ) throws -> Data {
        let remaining = entries.filter { $0.id != removed.id }
        return try saveHistory(remaining, replacing: base)
    }

    @discardableResult
    func clearHistory(replacing base: Data?) throws -> Data {
        try saveHistory([], replacing: base)
    }

    @discardableResult
    func compactHistory(
        _ entries: [InputHistoryEntry],
        replacing base: Data?
    ) throws -> Data {
        try saveHistory(entries.filter { $0.isUsefulForCompletion }, replacing: base)
    }

    private func saveHistory(
        _ entries: [InputHistoryEntry],
        replacing base: Data?
    ) throws -> Data {
        let current = try currentData(at: historyURL)
        guard current == base else {
            throw UserDataStoreError.externallyModified
        }
        try FileManager.default.createDirectory(
            at: directoryURL,
            withIntermediateDirectories: true
        )
        var text = historyHeader
        for entry in entries {
            text += "\(entry.reading)\t\(entry.surface)\t\(entry.count)\t"
            text += "\(UInt64(entry.lastUsed.timeIntervalSince1970))\n"
        }
        let data = Data(text.utf8)
        try data.write(to: historyURL, options: .atomic)
        NotificationCenter.default.post(name: .unvalleyUserDataDidChange, object: nil)
        return data
    }

    func revealDictionary() {
        try? FileManager.default.createDirectory(
            at: directoryURL,
            withIntermediateDirectories: true
        )
        if !FileManager.default.fileExists(atPath: dictionaryURL.path) {
            try? Data(dictionaryHeader.utf8).write(to: dictionaryURL, options: .atomic)
        }
        NSWorkspace.shared.activateFileViewerSelecting([dictionaryURL])
    }

    private func isValid(_ value: String) -> Bool {
        !value.isEmpty && !value.contains("\t") && !value.contains("\n") && !value.contains("\r")
    }

    private func currentData(at url: URL) throws -> Data? {
        guard FileManager.default.fileExists(atPath: url.path) else { return nil }
        return try Data(contentsOf: url)
    }
}

func normalizedDictionaryReading(_ value: String) -> String {
    let trimmed = value.trimmingCharacters(
        in: CharacterSet.whitespacesAndNewlines.union(
            CharacterSet(charactersIn: "\u{FEFF}")
        )
    )
    let scalars = trimmed.unicodeScalars.map { scalar -> Unicode.Scalar in
        if (0x30A1 ... 0x30F6).contains(scalar.value),
           let hiragana = Unicode.Scalar(scalar.value - 0x60)
        {
            return hiragana
        }
        return scalar
    }
    return String(String.UnicodeScalarView(scalars))
}
