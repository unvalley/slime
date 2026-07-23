import AppKit

@main
enum AdapterTests {
    static func main() throws {
        let testDirectory = FileManager.default.temporaryDirectory.appendingPathComponent(
            "slime-adapter-tests-\(ProcessInfo.processInfo.processIdentifier)-\(UUID().uuidString)",
            isDirectory: true
        )
        try FileManager.default.createDirectory(
            at: testDirectory,
            withIntermediateDirectories: true
        )
        defer { try? FileManager.default.removeItem(at: testDirectory) }

        let ordinaryInputOptions = InputRuntimeOptions(
            liveConversion: true,
            historyCompletion: true,
            historyLearning: true,
            dictionaryPacks: 7,
            secureEventInput: false
        )
        try expect(
            ordinaryInputOptions.historyLearning,
            "ordinary input should preserve the user's history learning setting"
        )
        let secureInputOptions = InputRuntimeOptions(
            liveConversion: true,
            historyCompletion: true,
            historyLearning: true,
            dictionaryPacks: 7,
            secureEventInput: true
        )
        try expect(
            !secureInputOptions.historyLearning,
            "secure event input should pause history learning"
        )
        try expect(
            secureInputOptions.liveConversion
                && secureInputOptions.historyCompletion
                && secureInputOptions.dictionaryPacks == 7,
            "secure event input should not rewrite unrelated preferences"
        )
        let userDisabledLearning = InputRuntimeOptions(
            liveConversion: false,
            historyCompletion: false,
            historyLearning: false,
            dictionaryPacks: 0,
            secureEventInput: false
        )
        try expect(
            !userDisabledLearning.historyLearning,
            "leaving secure input should not override a disabled user preference"
        )
        let privacyDirectory = testDirectory.appendingPathComponent(
            "secure-input-history",
            isDirectory: true
        )
        let privacyEngine = try RustEngine(dataDirectory: privacyDirectory)
        _ = try privacyEngine.setOptions(
            liveConversion: secureInputOptions.liveConversion,
            historyCompletion: secureInputOptions.historyCompletion,
            historyLearning: secureInputOptions.historyLearning,
            dictionaryPacks: secureInputOptions.dictionaryPacks
        )
        try commitNihon(using: privacyEngine)
        let privacyHistoryURL = privacyDirectory.appendingPathComponent("history.tsv")
        try expect(
            !FileManager.default.fileExists(atPath: privacyHistoryURL.path),
            "secure input should not persist a committed conversion"
        )
        _ = try privacyEngine.setOptions(
            liveConversion: ordinaryInputOptions.liveConversion,
            historyCompletion: ordinaryInputOptions.historyCompletion,
            historyLearning: ordinaryInputOptions.historyLearning,
            dictionaryPacks: ordinaryInputOptions.dictionaryPacks
        )
        try commitNihon(using: privacyEngine)
        let resumedHistory = try String(contentsOf: privacyHistoryURL, encoding: .utf8)
        try expect(
            resumedHistory.contains("にほん\t日本"),
            "leaving secure input should resume learning without changing the user setting"
        )

        let appKitEngine = try RustEngine(dataDirectory: testDirectory)
        let textView = NSTextView(frame: .zero)
        for scalar in "nihon".unicodeScalars {
            for action in try appKitEngine.process(.character(scalar)) {
                _ = applyTextMutation(action, client: textView)
            }
        }
        try expect(
            textView.string == "にほn" && textView.markedRange().location == 0,
            "engine preedit actions should create marked text in an AppKit text client"
        )
        let appKitConversion = try appKitEngine.process(.space)
        let appKitCandidate = try expectValue(
            appKitConversion.first(where: { $0.type == "update_preedit" })?.text,
            "conversion should update the AppKit preedit"
        )
        for action in appKitConversion {
            _ = applyTextMutation(action, client: textView)
        }
        for action in try appKitEngine.process(.enter) {
            _ = applyTextMutation(action, client: textView)
        }
        try expect(
            textView.string == appKitCandidate && textView.markedRange().length == 0,
            "commit actions should replace AppKit marked text with the selected candidate"
        )

        let engine = try RustEngine(dataDirectory: testDirectory)
        var latestPreedit: String?

        for scalar in "nihon".unicodeScalars {
            let actions = try engine.process(.character(scalar))
            latestPreedit = actions.last(where: { $0.type == "update_preedit" })?.text
        }
        try expect(latestPreedit == "にほn", "ambiguous trailing n should remain literal")

        let conversion = try engine.process(.space)
        let candidateAction = conversion.first(where: { $0.type == "show_candidates" })
        try expect(candidateAction?.candidates?.contains("日本") == true, "日本 should be a candidate")

        let candidates = try expectValue(candidateAction?.candidates, "candidate list should be present")
        let selectedCandidate = candidates[1]
        let selection = try engine.process(.selectCandidate(1))
        try expect(
            selection.contains(where: {
                $0.type == "update_preedit" && $0.text == selectedCandidate
            }),
            "candidate selection should update the preedit"
        )

        let commit = try engine.process(.enter)
        try expect(
            commit.contains(where: { $0.type == "commit" && $0.text == selectedCandidate }),
            "selected candidate should be committed"
        )

        for scalar in "seidowotakamerukufuuwoshiteikimashou".unicodeScalars {
            _ = try engine.process(.character(scalar))
        }
        let phraseConversion = try engine.process(.space)
        try expect(
            phraseConversion.contains(where: {
                $0.type == "update_preedit" && $0.text == "精度を高める工夫をしていきましょう"
            }),
            "connected phrase conversion should pass through the Swift adapter"
        )

        _ = try engine.process(.enter)
        for scalar in "kikan".unicodeScalars {
            _ = try engine.process(.character(scalar))
        }
        let katakanaConversion = try engine.process(.space)
        try expect(
            katakanaConversion.contains(where: {
                $0.type == "show_candidates"
                    && $0.candidates?.count ?? 0 > 9
                    && $0.candidates?[1] == "キカン"
            }),
            "Katakana should be visible on the first candidate page through the Swift adapter"
        )
        _ = try engine.process(.escape)
        _ = try engine.process(.escape)

        var symbolPreedit: String?
        for scalar in "123,.!?()".unicodeScalars {
            let actions = try engine.process(.character(scalar))
            symbolPreedit = actions.last(where: { $0.type == "update_preedit" })?.text
        }
        try expect(
            symbolPreedit == "１２３、。！？（）",
            "ASCII numbers and symbols should become Japanese full-width text"
        )

        let shiftedKeys: [(base: String, shifted: String, keyCode: UInt16)] = [
            ("1", "!", 18),
            ("8", "(", 28),
            ("/", "?", 44),
        ]
        for key in shiftedKeys {
            let event = try expectValue(
                NSEvent.keyEvent(
                    with: .keyDown,
                    location: .zero,
                    modifierFlags: .shift,
                    timestamp: 0,
                    windowNumber: 0,
                    context: nil,
                    characters: key.shifted,
                    charactersIgnoringModifiers: key.base,
                    isARepeat: false,
                    keyCode: key.keyCode
                ),
                "shifted key event should be created"
            )
            try expect(
                printableInputScalar(from: event) == key.shifted.unicodeScalars.first,
                "Shift+symbol should use the modified character"
            )
        }

        try expect(
            shouldForwardBackspaceDirectly(keyCode: 51, hasComposition: false),
            "idle Backspace should bypass the Rust engine"
        )
        try expect(
            !shouldForwardBackspaceDirectly(keyCode: 51, hasComposition: true),
            "Backspace should edit an active composition"
        )
        try expect(
            !shouldForwardBackspaceDirectly(keyCode: 117, hasComposition: false),
            "forward Delete should keep its separate routing"
        )

        try expect(
            candidateSelectionIndex(keyCode: 49, candidateCount: 4, pageStart: 0) == nil,
            "Space should remain a conversion key while candidates are visible"
        )
        try expect(
            candidateSelectionIndex(keyCode: 18, candidateCount: 4, pageStart: 0) == 0,
            "number keys should resolve to candidate indices"
        )
        try expect(
            candidateSelectionIndex(keyCode: 21, candidateCount: 4, pageStart: 0) == 3,
            "number selection should respect the available candidates"
        )
        try expect(
            candidateSelectionIndex(keyCode: 23, candidateCount: 4, pageStart: 0) == nil,
            "out-of-range number keys should remain normal input"
        )
        try expect(
            candidateSelectionIndex(keyCode: 18, candidateCount: 12, pageStart: 9) == 9,
            "number keys should select from the visible candidate page"
        )

        let visibleFrame = NSRect(x: 0, y: 0, width: 800, height: 600)
        let panelAboveInput = candidatePanelFrame(
            anchor: NSRect(x: 300, y: 12, width: 0, height: 20),
            preferredWidth: 112,
            visibleCount: 3,
            visibleFrame: visibleFrame
        )
        try expect(
            panelAboveInput.minY == 36,
            "candidate panel should move above input near the bottom screen edge"
        )

        let panelBelowInput = candidatePanelFrame(
            anchor: NSRect(x: 300, y: 400, width: 0, height: 20),
            preferredWidth: 112,
            visibleCount: 3,
            visibleFrame: visibleFrame
        )
        try expect(
            panelBelowInput.maxY == 396,
            "candidate panel should remain below input when there is enough space"
        )

        try testUserDataStore(in: testDirectory.appendingPathComponent("settings"))
        try testDictionaryImports()
        try testDomainDictionary(
            in: testDirectory.appendingPathComponent("domain-dictionary")
        )
        try testUserDictionaryAndHistoryCompletion(
            in: testDirectory.appendingPathComponent("engine-user-data")
        )

        print("macOS Swift adapter tests passed")
    }

    private static func commitNihon(using engine: RustEngine) throws {
        for scalar in "nihon".unicodeScalars {
            _ = try engine.process(.character(scalar))
        }
        _ = try engine.process(.space)
        _ = try engine.process(.enter)
    }

    private static func testUserDataStore(in directory: URL) throws {
        let store = UserDataStore(directoryURL: directory)
        let externallyCreatedDirectory = directory.appendingPathComponent(
            "externally-created",
            isDirectory: true
        )
        let externallyCreatedStore = UserDataStore(directoryURL: externallyCreatedDirectory)
        let absentDictionary = try externallyCreatedStore.loadDictionary()
        try FileManager.default.createDirectory(
            at: externallyCreatedDirectory,
            withIntermediateDirectories: true
        )
        let createdDictionary = Data(
            "# slime-user-dictionary-v1\nそと\t外部作成\n".utf8
        )
        try createdDictionary.write(to: externallyCreatedStore.dictionaryURL)
        do {
            _ = try externallyCreatedStore.saveDictionary(
                [UserDictionaryEntry(reading: "ほげ", surface: "HOGE")],
                replacing: absentDictionary.base
            )
            throw TestFailure(message: "external dictionary creation must block replacement")
        } catch UserDataStoreError.externallyModified {
            let preserved = try Data(contentsOf: externallyCreatedStore.dictionaryURL)
            try expect(
                preserved == createdDictionary,
                "externally created dictionary bytes should be preserved"
            )
        }

        let first = try store.saveDictionary(
            [UserDictionaryEntry(reading: "ほげ", surface: "HOGE")],
            replacing: nil
        )
        let loaded = try store.loadDictionary()
        try expect(loaded.entries.map(\.surface) == ["HOGE"], "saved dictionary should reload")
        try expect(
            normalizedDictionaryReading(" パフォーマンス ") == "ぱふぉーまんす",
            "dictionary readings should normalize Katakana to Hiragana"
        )

        let external = Data("# slime-user-dictionary-v1\nそと\t外部変更\n".utf8)
        try external.write(to: store.dictionaryURL, options: .atomic)
        do {
            _ = try store.saveDictionary(
                [UserDictionaryEntry(reading: "ほげ", surface: "変更")],
                replacing: first
            )
            throw TestFailure(message: "external dictionary changes must block replacement")
        } catch UserDataStoreError.externallyModified {
            let preserved = try Data(contentsOf: store.dictionaryURL)
            try expect(
                preserved == external,
                "external dictionary bytes should be preserved"
            )
        }

        let historyData = Data(
            "# slime-history-v1\nにほん\t日本\t2\t100\nぱふぉーまんす\tパフォーマンス\t1\t200\n".utf8
        )
        try historyData.write(to: store.historyURL, options: .atomic)
        let history = try store.loadHistorySnapshot()
        let removed = try expectValue(
            history.entries.first(where: { $0.surface == "日本" }),
            "history fixture should contain 日本"
        )
        _ = try store.removeHistoryEntry(
            removed,
            from: history.entries,
            replacing: history.base
        )
        let remaining = try store.loadHistory()
        try expect(
            remaining.map(\.surface) == ["パフォーマンス"],
            "individual history deletion should preserve other entries"
        )

        let compactFixture = Data(
            (
                "# slime-history-v1\nに\t二\t3\t50\nかな\tかな\t2\t60\n"
                    + "nihon\t日本\t2\t65\nにほん\t日本\t1\t70\n"
                    + "\(String(repeating: "あ", count: 65))\t長すぎる読み\t1\t80\n"
                    + "ながすぎるひょうき\t\(String(repeating: "亜", count: 129))\t1\t90\n"
            ).utf8
        )
        try compactFixture.write(to: store.historyURL, options: .atomic)
        let beforeCompaction = try store.loadHistorySnapshot()
        _ = try store.compactHistory(beforeCompaction.entries, replacing: beforeCompaction.base)
        let compacted = try store.loadHistory()
        try expect(
            compacted.map(\.surface) == ["日本"],
            "history compaction should remove only entries excluded by learning rules"
        )

        let stale = try store.loadHistorySnapshot()
        let externallyChanged = Data(
            "# slime-history-v1\nそと\t外部変更\t1\t300\n".utf8
        )
        try externallyChanged.write(to: store.historyURL, options: .atomic)
        do {
            _ = try store.clearHistory(replacing: stale.base)
            throw TestFailure(message: "external history changes must block replacement")
        } catch UserDataStoreError.externallyModified {
            let preservedHistory = try Data(contentsOf: store.historyURL)
            try expect(
                preservedHistory == externallyChanged,
                "external history bytes should be preserved"
            )
        }

        let absentHistory = try externallyCreatedStore.loadHistorySnapshot()
        let createdHistory = Data(
            "# slime-history-v1\nそと\t外部作成\t1\t400\n".utf8
        )
        try createdHistory.write(to: externallyCreatedStore.historyURL)
        do {
            _ = try externallyCreatedStore.clearHistory(replacing: absentHistory.base)
            throw TestFailure(message: "external history creation must block replacement")
        } catch UserDataStoreError.externallyModified {
            let preserved = try Data(contentsOf: externallyCreatedStore.historyURL)
            try expect(
                preserved == createdHistory,
                "externally created history bytes should be preserved"
            )
        }
    }

    private static func testDictionaryImports() throws {
        let google = Data(
            "\u{FEFF}# exported dictionary\nパフォーマンス\tPerformance\t名詞\nぱふぇ\tパフェ\t名詞\nぱふぇ\tパフェ\t名詞\ninvalid\n".utf8
        )
        let googleResult = try DictionaryImporter.parse(data: google, fileExtension: "txt")
        try expect(googleResult.formatName == "Google日本語入力辞書", "Google format name")
        try expect(
            googleResult.entries.map(\.reading) == ["ぱふぉーまんす", "ぱふぇ"],
            "Google readings should normalize and preserve order"
        )
        try expect(
            googleResult.skippedCount == 2,
            "invalid and duplicate Google rows should be reported"
        )

        let microsoft = Data(
            "!Microsoft IME Dictionary Tool\nにほん\t日本\t名詞\n".utf8
        )
        let microsoftResult = try DictionaryImporter.parse(
            data: microsoft,
            fileExtension: "txt"
        )
        try expect(
            microsoftResult.formatName == "Microsoft IME辞書",
            "Microsoft header should be detected"
        )

        let shiftJISText = "!Microsoft IME Dictionary Tool\nとうきょう\t東京\t地名\n"
        let shiftJIS = try expectValue(
            shiftJISText.data(using: .shiftJIS),
            "Shift JIS fixture should encode"
        )
        let shiftJISResult = try DictionaryImporter.parse(
            data: shiftJIS,
            fileExtension: "txt"
        )
        try expect(
            shiftJISResult.entries.first?.surface == "東京",
            "Shift JIS dictionaries should import"
        )

        let atok = Data(
            "!!ATOK_TANGO_TEXT_HEADER 1\nりんぎしょ\t稟議書\t固有人一般\n".utf8
        )
        let atokResult = try DictionaryImporter.parse(data: atok, fileExtension: "txt")
        try expect(atokResult.formatName == "ATOK辞書", "ATOK header should be detected")
        try expect(atokResult.entries.first?.surface == "稟議書", "ATOK rows should import")

        let kotoeri = Data(
            "// Kotoeri dictionary\n\"いんよう\",\"「引用」\",\"普通名詞\"\n\"だぶる\",\"二重\"\"引用\",\"普通名詞\"\n".utf8
        )
        let kotoeriResult = try DictionaryImporter.parse(
            data: kotoeri,
            fileExtension: "txt"
        )
        try expect(
            kotoeriResult.formatName == "旧Mac日本語入力辞書",
            "quoted CSV should be detected as Kotoeri"
        )
        try expect(
            kotoeriResult.entries.map(\.surface) == ["「引用」", "二重\"引用"],
            "Kotoeri CSV quoting should be decoded"
        )

        let appleObject: [[String: String]] = [
            ["shortcut": "ぱふぉーまんす", "phrase": "パフォーマンス"],
            ["replace": "にほん", "with": "日本"],
        ]
        let apple = try PropertyListSerialization.data(
            fromPropertyList: appleObject,
            format: .xml,
            options: 0
        )
        let appleResult = try DictionaryImporter.parse(data: apple, fileExtension: "plist")
        try expect(
            appleResult.entries.map(\.surface) == ["パフォーマンス", "日本"],
            "both current and legacy Apple replacement keys should import"
        )

        do {
            _ = try DictionaryImporter.parse(data: Data("invalid".utf8), fileExtension: "txt")
            throw TestFailure(message: "files without valid entries should fail")
        } catch DictionaryImportError.noValidEntries {
            // Expected.
        }
    }

    private static func testDomainDictionary(in directory: URL) throws {
        let engine = try RustEngine(dataDirectory: directory)
        _ = try engine.setOptions(
            liveConversion: false,
            historyCompletion: false,
            dictionaryPacks: 1
        )
        for scalar in "suwifutoyu-ai".unicodeScalars {
            _ = try engine.process(.character(scalar))
        }
        let actions = try engine.process(.space)
        try expect(
            actions.contains(where: {
                $0.type == "show_candidates" && $0.candidates?.contains("SwiftUI") == true
            }),
            "technology dictionary should cross the Swift/C/Rust boundary"
        )
    }

    private static func testUserDictionaryAndHistoryCompletion(in directory: URL) throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try Data("# slime-user-dictionary-v1\nほげ\tHOGE\n".utf8).write(
            to: directory.appendingPathComponent("user_dictionary.tsv")
        )
        try Data(
            "# slime-history-v1\nぱふぉーまんす\tパフォーマンス\t5\t10\n".utf8
        ).write(to: directory.appendingPathComponent("history.tsv"))

        let engine = try RustEngine(dataDirectory: directory)
        _ = try engine.setOptions(liveConversion: false, historyCompletion: true)
        for scalar in "hoge".unicodeScalars {
            _ = try engine.process(.character(scalar))
        }
        let dictionaryActions = try engine.process(.space)
        try expect(
            dictionaryActions.contains(where: { $0.type == "update_preedit" && $0.text == "HOGE" }),
            "user dictionary entries should rank first"
        )

        _ = try engine.process(.enter)
        for scalar in "pafo".unicodeScalars {
            _ = try engine.process(.character(scalar))
        }
        let completion = try engine.process(.character("r"))
        try expect(
            completion.contains(where: {
                $0.type == "show_candidates"
                    && $0.candidates?.contains("パフォーマンス") == true
            }),
            "history should provide prefix completions through the Swift adapter"
        )
    }

    private static func expect(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        guard condition() else {
            throw TestFailure(message: message)
        }
    }

    private static func expectValue<T>(_ value: T?, _ message: String) throws -> T {
        guard let value else {
            throw TestFailure(message: message)
        }
        return value
    }

    private struct TestFailure: Error, CustomStringConvertible {
        let message: String
        var description: String { message }
    }
}
