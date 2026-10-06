import Foundation

struct DictionaryImportResult: Equatable {
    let entries: [UserDictionaryEntry]
    let skippedCount: Int
    let formatName: String
}

enum DictionaryImportError: LocalizedError {
    case unreadableText
    case unsupportedPropertyList
    case noValidEntries
    case tooManyEntries

    var errorDescription: String? {
        switch self {
        case .unreadableText:
            "辞書をUTF-8、UTF-16、またはShift JISのテキストとして読み込めませんでした。"
        case .unsupportedPropertyList:
            "Appleのテキスト置換ファイルとして認識できませんでした。"
        case .noValidEntries:
            "読み込める単語がありませんでした。ファイル形式を確認してください。"
        case .tooManyEntries:
            "一度に読み込める単語は100,000件までです。辞書を分割してください。"
        }
    }
}

enum DictionaryImporter {
    private enum TextFormat {
        case plainTabSeparated
        case singleBangHeader
        case doubleBangHeader
        case quotedCSV

        var name: String {
            switch self {
            case .plainTabSeparated: "タブ区切り辞書"
            case .singleBangHeader, .doubleBangHeader: "ヘッダー付きタブ区切り辞書"
            case .quotedCSV: "CSV辞書"
            }
        }
    }

    private static let maximumEntries = 100_000
    private static let trimCharacters = CharacterSet.whitespacesAndNewlines.union(
        CharacterSet(charactersIn: "\u{FEFF}")
    )

    static func parse(data: Data, fileExtension: String) throws -> DictionaryImportResult {
        if fileExtension.lowercased() == "plist" || looksLikePropertyList(data) {
            return try parseApplePropertyList(data)
        }
        return try parseDelimitedText(data)
    }

    private static func parseApplePropertyList(_ data: Data) throws -> DictionaryImportResult {
        let propertyList: Any
        do {
            propertyList = try PropertyListSerialization.propertyList(
                from: data,
                options: [],
                format: nil
            )
        } catch {
            throw DictionaryImportError.unsupportedPropertyList
        }

        let rawItems: [[String: Any]]
        if let items = propertyList as? [[String: Any]] {
            rawItems = items
        } else if let root = propertyList as? [String: Any],
                  let items = root["NSUserDictionaryReplacementItems"] as? [[String: Any]]
        {
            rawItems = items
        } else {
            throw DictionaryImportError.unsupportedPropertyList
        }

        var entries: [UserDictionaryEntry] = []
        var skipped = 0
        for item in rawItems {
            let pair: (reading: String, surface: String)?
            if let shortcut = item["shortcut"] as? String,
               let phrase = item["phrase"] as? String
            {
                pair = (shortcut, phrase)
            } else if let replace = item["replace"] as? String,
                      let with = item["with"] as? String
            {
                pair = (replace, with)
            } else {
                pair = nil
            }

            guard let pair,
                  let entry = normalizedEntry(reading: pair.reading, surface: pair.surface)
            else {
                skipped += 1
                continue
            }
            entries.append(entry)
            if entries.count > maximumEntries {
                throw DictionaryImportError.tooManyEntries
            }
        }

        return try finalized(entries: entries, skipped: skipped, formatName: "Macユーザ辞書")
    }

    private static func parseDelimitedText(_ data: Data) throws -> DictionaryImportResult {
        let text: String? = if let utf8 = String(data: data, encoding: .utf8) {
            utf8
        } else if data.starts(with: [0xFF, 0xFE]) || data.starts(with: [0xFE, 0xFF]) {
            String(data: data, encoding: .utf16)
        } else {
            String(data: data, encoding: .shiftJIS)
        }
        guard let text else {
            throw DictionaryImportError.unreadableText
        }

        var entries: [UserDictionaryEntry] = []
        var skipped = 0
        let format = detectTextFormat(text)
        for rawLine in text.components(separatedBy: .newlines) {
            let line = rawLine.trimmingCharacters(in: trimCharacters)
            if line.isEmpty
                || line.hasPrefix("#")
                || (format == .quotedCSV && line.hasPrefix("//"))
            {
                continue
            }
            if line.hasPrefix("!") && format != .plainTabSeparated {
                continue
            }

            let columns: [String] = if format == .quotedCSV {
                parseCSVLine(rawLine) ?? []
            } else {
                rawLine.split(separator: "\t", omittingEmptySubsequences: false).map(String.init)
            }
            guard columns.count >= 2,
                  let entry = normalizedEntry(
                      reading: columns[0],
                      surface: columns[1]
                  )
            else {
                skipped += 1
                continue
            }
            entries.append(entry)
            if entries.count > maximumEntries {
                throw DictionaryImportError.tooManyEntries
            }
        }

        return try finalized(
            entries: entries,
            skipped: skipped,
            formatName: format.name
        )
    }

    private static func detectTextFormat(_ text: String) -> TextFormat {
        let lines = text.components(separatedBy: .newlines)
            .map { $0.trimmingCharacters(in: trimCharacters) }
            .filter { !$0.isEmpty }
        let firstLine = lines.first ?? ""
        let lowercased = firstLine.lowercased()
        if lowercased.hasPrefix("!microsoft ime") {
            return .singleBangHeader
        }
        if lowercased.hasPrefix("!!atok_tango_text_header")
            || lowercased.hasPrefix("!!dicut")
        {
            return .doubleBangHeader
        }
        let firstDataLine = lines.first { !$0.hasPrefix("//") } ?? firstLine
        if firstDataLine.hasPrefix("\"")
            && firstDataLine.hasSuffix("\"")
            && !firstDataLine.contains("\t")
        {
            return .quotedCSV
        }
        return .plainTabSeparated
    }

    private static func parseCSVLine(_ line: String) -> [String]? {
        var values: [String] = []
        var value = ""
        var isQuoted = false
        var index = line.startIndex
        while index < line.endIndex {
            let character = line[index]
            if character == "\"" {
                let next = line.index(after: index)
                if isQuoted, next < line.endIndex, line[next] == "\"" {
                    value.append("\"")
                    index = line.index(after: next)
                    continue
                }
                isQuoted.toggle()
            } else if character == ",", !isQuoted {
                values.append(value)
                value = ""
            } else {
                value.append(character)
            }
            index = line.index(after: index)
        }
        guard !isQuoted else { return nil }
        values.append(value)
        return values
    }

    private static func finalized(
        entries: [UserDictionaryEntry],
        skipped: Int,
        formatName: String
    ) throws -> DictionaryImportResult {
        var seen = Set<String>()
        let uniqueEntries = entries.filter { entry in
            seen.insert("\(entry.reading)\u{0}\(entry.surface)").inserted
        }
        let duplicateCount = entries.count - uniqueEntries.count
        guard !uniqueEntries.isEmpty else {
            throw DictionaryImportError.noValidEntries
        }
        return DictionaryImportResult(
            entries: uniqueEntries,
            skippedCount: skipped + duplicateCount,
            formatName: formatName
        )
    }

    private static func normalizedEntry(
        reading: String,
        surface: String
    ) -> UserDictionaryEntry? {
        let normalizedReading = normalizedDictionaryReading(reading)
        let normalizedSurface = surface.trimmingCharacters(in: trimCharacters)
        guard !normalizedReading.isEmpty,
              !normalizedSurface.isEmpty,
              !normalizedReading.contains("\t"),
              !normalizedSurface.contains("\t"),
              !normalizedReading.contains("\n"),
              !normalizedSurface.contains("\n")
        else {
            return nil
        }
        return UserDictionaryEntry(reading: normalizedReading, surface: normalizedSurface)
    }

    private static func looksLikePropertyList(_ data: Data) -> Bool {
        data.starts(with: Data("bplist".utf8))
            || data.starts(with: Data("<?xml".utf8))
    }
}
