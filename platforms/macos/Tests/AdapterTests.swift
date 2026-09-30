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

        try runInputContextTests(
            in: testDirectory.appendingPathComponent("external-document-context")
        )

        let verificationSuite = "slime-input-verification-tests-\(UUID().uuidString)"
        let verificationDefaults = try expectValue(
            UserDefaults(suiteName: verificationSuite),
            "input verification should create isolated defaults"
        )
        defer {
            verificationDefaults.removePersistentDomain(forName: verificationSuite)
        }
        let verificationToken = UUID().uuidString
        verificationDefaults.set(verificationToken, forKey: InputVerification.defaultsKey)
        let normalizedVerificationToken = try expectValue(
            InputVerification.pendingToken(defaults: verificationDefaults),
            "a canonical UUID should enable one input verification event"
        )
        try expect(
            normalizedVerificationToken == verificationToken.lowercased(),
            "input verification should normalize its non-sensitive correlation token"
        )
        try expect(
            InputVerification.pendingRequest(defaults: verificationDefaults) == nil,
            "input verification should reject a token that is not bound to one client process"
        )
        let verificationTargetProcess: Int32 = 1234
        verificationDefaults.set(
            Int(verificationTargetProcess),
            forKey: InputVerification.targetProcessDefaultsKey
        )
        try expect(
            InputVerification.pendingRequest(defaults: verificationDefaults)
                == InputVerification.Request(
                    token: normalizedVerificationToken,
                    mode: .character,
                    targetProcessIdentifier: verificationTargetProcess
                ),
            "a token without an explicit mode should preserve the character gate"
        )
        verificationDefaults.set(
            InputVerification.Mode.candidateSelection.rawValue,
            forKey: InputVerification.modeDefaultsKey
        )
        try expect(
            InputVerification.pendingRequest(defaults: verificationDefaults)
                == InputVerification.Request(
                    token: normalizedVerificationToken,
                    mode: .candidateSelection,
                    targetProcessIdentifier: verificationTargetProcess
                ),
            "candidate verification should require an explicit supported mode"
        )
        for mode in [
            InputVerification.Mode.candidateNumber,
            InputVerification.Mode.candidateClick,
            InputVerification.Mode.reconversion,
            InputVerification.Mode.privacyPrivate,
            InputVerification.Mode.privacySecure,
            InputVerification.Mode.privacyResume,
        ] {
            verificationDefaults.set(mode.rawValue, forKey: InputVerification.modeDefaultsKey)
            try expect(
                InputVerification.pendingRequest(defaults: verificationDefaults)
                    == InputVerification.Request(
                        token: normalizedVerificationToken,
                        mode: mode,
                        targetProcessIdentifier: verificationTargetProcess
                    ),
                "each interaction gate should parse its explicit mode"
            )
        }
        InputVerification.consume(
            normalizedVerificationToken,
            defaults: verificationDefaults
        )
        try expect(
            InputVerification.pendingToken(defaults: verificationDefaults) == nil,
            "input verification should remove a consumed token"
        )
        try expect(
            verificationDefaults.string(forKey: InputVerification.modeDefaultsKey) == nil,
            "input verification should remove a consumed mode"
        )
        try expect(
            verificationDefaults.object(
                forKey: InputVerification.targetProcessDefaultsKey
            ) == nil,
            "input verification should remove a consumed client-process binding"
        )
        verificationDefaults.set("not-a-token\nkey-data", forKey: InputVerification.defaultsKey)
        try expect(
            InputVerification.pendingToken(defaults: verificationDefaults) == nil,
            "input verification should reject malformed or log-injectable tokens"
        )
        try expect(
            InputVerification.isVerificationCharacter("a", hasDisallowedModifiers: false)
                && InputVerification.isVerificationCharacter(
                    "Z",
                    hasDisallowedModifiers: false
                ),
            "input verification should accept one unmodified ASCII letter"
        )
        try expect(
            !InputVerification.isVerificationCharacter(" ", hasDisallowedModifiers: false)
                && !InputVerification.isVerificationCharacter(
                    "ab",
                    hasDisallowedModifiers: false
                )
                && !InputVerification.isVerificationCharacter(
                    "a",
                    hasDisallowedModifiers: true
                ),
            "input verification should ignore commands and non-letter events"
        )
        try expect(
            InputVerification.isHardwareEventSource(1)
                && !InputVerification.isHardwareEventSource(0)
                && !InputVerification.isHardwareEventSource(-1)
                && !InputVerification.isHardwareEventSource(nil),
            "physical input evidence should accept only the HID system event source"
        )
        var candidateVerification = InputVerification.CandidateSelectionState()
        try expect(
            candidateVerification.record(.candidates(selected: 0)) == nil,
            "candidate verification should not start from non-character activity"
        )
        try expect(
            candidateVerification.record(.character) == .character
                && candidateVerification.record(.character) == nil,
            "candidate verification should record only the first character step"
        )
        try expect(
            candidateVerification.record(.candidates(selected: 0)) == .candidateShown
                && candidateVerification.record(.candidates(selected: 0)) == nil
                && candidateVerification.record(.candidates(selected: 1)) == nil
                && candidateVerification.record(.commit) == nil,
            "candidate verification should require a recorded interaction before movement"
        )
        try expect(
            candidateVerification.record(.selection(.arrow)) == .candidateArrow
                && candidateVerification.record(.selection(.number)) == nil
                && candidateVerification.record(.candidates(selected: 1)) == .candidateMoved
                && candidateVerification.record(.commit) == nil
                && candidateVerification.record(.accept) == nil
                && candidateVerification.record(.commit) == .candidateCommitted
                && candidateVerification.record(.candidates(selected: 2)) == nil,
            "candidate verification should complete only after show, interaction, move, and commit"
        )
        for (method, expectedStep) in [
            (
                InputVerification.CandidateSelectionMethod.number,
                InputVerification.CandidateStep.candidateNumber
            ),
            (
                InputVerification.CandidateSelectionMethod.click,
                InputVerification.CandidateStep.candidateClick
            ),
        ] {
            var interaction = InputVerification.CandidateSelectionState()
            _ = interaction.record(.character)
            _ = interaction.record(.candidates(selected: 0))
            try expect(
                interaction.record(.selection(method)) == expectedStep
                    && interaction.record(.candidates(selected: 1)) == .candidateMoved
                    && interaction.record(.accept) == nil
                    && interaction.record(.commit) == .candidateCommitted,
                "number and click gates should require their specific interaction path"
            )
        }
        var reconversionVerification = InputVerification.ReconversionState()
        try expect(
            reconversionVerification.record(.started) == nil
                && reconversionVerification.record(.commit) == nil,
            "reconversion verification should not start from internal actions"
        )
        try expect(
            reconversionVerification.record(.requested) == .requested
                && reconversionVerification.record(.requested) == nil
                && reconversionVerification.record(.started) == .started
                && reconversionVerification.record(.candidates) == .candidates
                && reconversionVerification.record(.commit) == nil
                && reconversionVerification.record(.accept) == nil
                && reconversionVerification.record(.commit) == .committed
                && reconversionVerification.record(.commit) == nil,
            "reconversion verification should require request, start, candidates, and commit"
        )
        try expect(
            InputVerification.Mode.privacyPrivate.matchesPrivacyState(
                privateMode: true,
                secureEventInput: false
            )
                && !InputVerification.Mode.privacyPrivate.matchesPrivacyState(
                    privateMode: false,
                    secureEventInput: false
                )
                && InputVerification.Mode.privacySecure.matchesPrivacyState(
                    privateMode: false,
                    secureEventInput: true
                )
                && InputVerification.Mode.privacyResume.matchesPrivacyState(
                    privateMode: false,
                    secureEventInput: false
                )
                && !InputVerification.Mode.privacyResume.matchesPrivacyState(
                    privateMode: true,
                    secureEventInput: false
                ),
            "privacy gates should match only their required runtime state"
        )
        var privacyVerification = InputVerification.PrivacyState()
        try expect(
            privacyVerification.record(.candidates) == nil
                && privacyVerification.record(.commit) == nil
                && privacyVerification.record(.character) == .character
                && privacyVerification.record(.character) == nil
                && privacyVerification.record(.candidates) == nil
                && privacyVerification.record(.requestCandidates) == nil
                && privacyVerification.record(.candidates) == .candidates
                && privacyVerification.record(.commit) == nil
                && privacyVerification.record(.accept) == nil
                && privacyVerification.record(.commit) == .committed
                && privacyVerification.record(.commit) == nil,
            "privacy verification should require physical conversion and acceptance in order"
        )

        let contextClientA = NSObject()
        let contextClientB = NSObject()
        let insertionPoint = NSRange(location: 8, length: 0)
        var contextBoundary = InputContextBoundary()
        try expect(
            !contextBoundary.shouldReset(
                client: contextClientA,
                selectedRange: insertionPoint
            ),
            "the first observed input client should establish a context baseline"
        )
        contextBoundary.observe(client: contextClientA, selectedRange: insertionPoint)
        try expect(
            !contextBoundary.shouldReset(
                client: contextClientA,
                selectedRange: insertionPoint
            ),
            "the same client and caret should preserve left context"
        )
        try expect(
            contextBoundary.shouldReset(
                client: contextClientA,
                selectedRange: NSRange(location: 3, length: 0)
            ),
            "an external caret move should break left context"
        )
        try expect(
            contextBoundary.shouldReset(
                client: contextClientB,
                selectedRange: NSRange(location: 3, length: 0)
            ),
            "changing the input client should break left context"
        )
        contextBoundary.clear()
        let unavailableSelection = NSRange(location: NSNotFound, length: 0)
        try expect(
            !contextBoundary.shouldReset(
                client: contextClientB,
                selectedRange: unavailableSelection
            ) && contextBoundary.shouldReset(
                client: contextClientB,
                selectedRange: unavailableSelection
            ),
            "an unavailable caret may establish a baseline but cannot prove continuity"
        )

        var requestedContextRange: NSRange?
        let boundedContext = precedingDocumentContext(
            selectedRange: NSRange(location: 300, length: 0),
            maximumCharacters: 4
        ) { range in
            requestedContextRange = range
            return "無関係な前置き文章"
        }
        try expect(
            requestedContextRange == NSRange(location: 292, length: 8)
                && boundedContext == "置き文章",
            "document context should request and retain only a bounded caret prefix"
        )
        var fetchedUnavailableContext = false
        let unavailableContext = precedingDocumentContext(
            selectedRange: unavailableSelection
        ) { _ in
            fetchedUnavailableContext = true
            return "読まない"
        }
        try expect(
            unavailableContext == nil && !fetchedUnavailableContext,
            "an unavailable caret must not read document text"
        )
        try expect(
            precedingDocumentContext(
                selectedRange: NSRange(location: 0, length: 0),
                fetch: { _ in "読まない" }
            ) == "",
            "the document start should provide an explicit empty context"
        )

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
                && !secureInputOptions.historyCompletion
                && secureInputOptions.privateMode
                && secureInputOptions.dictionaryPacks == 7,
            "secure event input should use private mode without rewriting conversion or dictionaries"
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
        let explicitPrivateOptions = InputRuntimeOptions(
            liveConversion: true,
            historyCompletion: true,
            historyLearning: true,
            dictionaryPacks: 7,
            secureEventInput: false,
            privateMode: true
        )
        try expect(
            explicitPrivateOptions.privateMode
                && !explicitPrivateOptions.historyCompletion
                && !explicitPrivateOptions.historyLearning,
            "process-private mode should disable both history reads and writes"
        )
        InputPrivacySession.toggle()
        let toggledPrivateOptions = InputRuntimeOptions(
            liveConversion: true,
            historyCompletion: true,
            historyLearning: true,
            dictionaryPacks: 0,
            secureEventInput: false
        )
        try expect(
            toggledPrivateOptions.privateMode,
            "private mode should be process-local and apply without a persistent preference"
        )
        InputPrivacySession.toggle()
        let privacyDirectory = testDirectory.appendingPathComponent(
            "secure-input-history",
            isDirectory: true
        )
        let privacyEngine = try RustEngine(dataDirectory: privacyDirectory)
        _ = try privacyEngine.setOptions(
            liveConversion: secureInputOptions.liveConversion,
            historyCompletion: secureInputOptions.historyCompletion,
            historyLearning: secureInputOptions.historyLearning,
            dictionaryPacks: secureInputOptions.dictionaryPacks,
            privateMode: secureInputOptions.privateMode
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

        let contextResetDirectory = testDirectory.appendingPathComponent(
            "swift-context-reset",
            isDirectory: true
        )
        let contextResetEngine = try RustEngine(dataDirectory: contextResetDirectory)
        _ = try contextResetEngine.setOptions(
            liveConversion: false,
            historyCompletion: true,
            historyLearning: true
        )
        try commitNihon(using: contextResetEngine)
        try contextResetEngine.resetContext()
        try commitNihon(using: contextResetEngine)
        try expect(
            !FileManager.default.fileExists(
                atPath: contextResetDirectory
                    .appendingPathComponent("context_history.tsv")
                    .path
            ),
            "the Swift reset bridge should prevent learning across a caret boundary"
        )

        let shortContextDirectory = testDirectory.appendingPathComponent(
            "swift-short-context",
            isDirectory: true
        )
        do {
            let trainingEngine = try RustEngine(dataDirectory: shortContextDirectory)
            _ = try trainingEngine.setOptions(
                liveConversion: false,
                historyCompletion: true,
                historyLearning: true
            )
            for _ in 0 ..< 2 {
                try convertAndCommit(input: "heya", surface: "部屋", using: trainingEngine)
                try convertAndCommit(input: "shoumei", surface: "照明", using: trainingEngine)
                try convertAndCommit(input: "hon'nin", surface: "本人", using: trainingEngine)
                try convertAndCommit(input: "shoumei", surface: "証明", using: trainingEngine)
            }
        }
        let reloadedContextEngine = try RustEngine(dataDirectory: shortContextDirectory)
        _ = try reloadedContextEngine.setOptions(
            liveConversion: false,
            historyCompletion: true,
            historyLearning: true
        )
        try convertAndCommit(input: "heya", surface: "部屋", using: reloadedContextEngine)
        for scalar in "shoumei".unicodeScalars {
            _ = try reloadedContextEngine.process(.character(scalar))
        }
        let shortContextActions = try reloadedContextEngine.process(.space)
        try expect(
            shortContextActions.first(where: { $0.type == "show_candidates" })?
                .candidates?.first == "照明"
                && shortContextActions.contains(where: {
                    $0.type == "update_preedit" && $0.text == "照明"
                }),
            "the Swift bridge should preserve a learned short-word context after reload"
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

        // SLIME_NO_SELECTION is SIZE_MAX, which Swift's Int reads as -1.
        let selectionEngine = try RustEngine(dataDirectory: testDirectory)
        var plainPreedits: [RustEngine.Action] = []
        for scalar in "watashihanihon".unicodeScalars {
            plainPreedits += try selectionEngine.process(.character(scalar))
                .filter { $0.type == "update_preedit" }
        }
        plainPreedits += try selectionEngine.process(.space)
            .filter { $0.type == "update_preedit" }
        try expect(
            !plainPreedits.isEmpty
                && plainPreedits.allSatisfy { $0.selectedStart == nil && $0.selectedLength == nil },
            "plain and whole-phrase preedits must not report a segment selection"
        )
        let segmentedPreedit = try expectValue(
            try selectionEngine.process(.nextSegment).first(where: { $0.type == "update_preedit" }),
            "segment navigation should update the preedit"
        )
        try expect(
            (segmentedPreedit.selectedStart ?? -1) > 0
                && (segmentedPreedit.selectedLength ?? 0) > 0,
            "segmented preedit should select the active segment in UTF-16 units"
        )

        let typoEngine = try RustEngine(dataDirectory: testDirectory)
        let typoTextView = NSTextView(frame: .zero)
        for scalar in "nihpn".unicodeScalars {
            for action in try typoEngine.process(.character(scalar)) {
                _ = applyTextMutation(action, client: typoTextView)
            }
        }
        let typoActions = try typoEngine.process(.space)
        for action in typoActions {
            _ = applyTextMutation(action, client: typoTextView)
        }
        let typoCandidates = try expectValue(
            typoActions.first(where: { $0.type == "show_candidates" })?.candidates,
            "typo correction candidates should cross the Swift bridge"
        )
        let typoDetails = try expectValue(
            typoActions.first(where: { $0.type == "show_candidates" })?.candidateDetails,
            "typed candidate metadata should cross the Swift bridge"
        )
        let correctionLabel = "日本　（にほんに訂正）"
        try expect(
            typoCandidates.contains(correctionLabel),
            "typo correction guidance should cross the Swift bridge"
        )
        let correctionIndex = try expectValue(
            typoCandidates.firstIndex(of: correctionLabel),
            "typo correction candidate should have a selectable index"
        )
        try expect(
            typoDetails[correctionIndex].value == "日本"
                && typoDetails[correctionIndex].annotation
                    == UInt32(SLIME_CANDIDATE_ANNOTATION_CORRECTION.rawValue)
                && typoDetails[correctionIndex].detail == "にほん"
                && candidateAnnotationText(typoDetails[correctionIndex]) == "にほんに訂正",
            "candidate metadata should keep the committed value separate from localized guidance"
        )
        let typoSelection = try typoEngine.process(.selectCandidate(UInt32(correctionIndex)))
        try expect(
            typoSelection.contains(where: {
                $0.type == "update_preedit" && $0.text == "日本"
            }),
            "selecting correction guidance should update preedit with only the surface"
        )
        try expect(
            typoSelection.contains(where: {
                $0.type == "show_candidates"
                    && $0.selected == correctionIndex
                    && $0.candidates?[correctionIndex] == correctionLabel
            }),
            "selecting correction guidance should keep the UI row synchronized"
        )
        for action in typoSelection {
            _ = applyTextMutation(action, client: typoTextView)
        }
        for action in try typoEngine.process(.enter) {
            _ = applyTextMutation(action, client: typoTextView)
        }
        try expect(
            typoTextView.string == "日本" && typoTextView.markedRange().length == 0,
            "committing correction guidance should insert only the corrected surface"
        )

        let recallEngine = try RustEngine(dataDirectory: testDirectory)
        let recallTextView = NSTextView(frame: .zero)
        for scalar in "asairi".unicodeScalars {
            for action in try recallEngine.process(.character(scalar)) {
                _ = applyTextMutation(action, client: recallTextView)
            }
        }
        let initialRecall = try recallEngine.process(.space)
        let initialRecallCandidates = try expectValue(
            initialRecall.first(where: { $0.type == "show_candidates" })?.candidates,
            "initial recall candidates should cross the Swift bridge"
        )
        try expect(
            !initialRecallCandidates.contains("浅煎り"),
            "the deep compound fixture should begin outside the initial pool"
        )
        var expandedRecallCandidates = initialRecallCandidates
        for _ in initialRecallCandidates.indices {
            let actions = try recallEngine.process(.nextCandidate)
            if let candidates = actions.first(where: {
                $0.type == "show_candidates"
            })?.candidates {
                expandedRecallCandidates = candidates
            }
        }
        let expandedRecallIndex = try expectValue(
            expandedRecallCandidates.firstIndex(of: "浅煎り"),
            "expanded recall candidate should cross the Swift bridge"
        )
        let recallSelection = try recallEngine.process(
            .selectCandidate(UInt32(expandedRecallIndex))
        )
        try expect(
            recallSelection.contains(where: {
                $0.type == "update_preedit" && $0.text == "浅煎り"
            }),
            "selecting expanded recall should update only the candidate surface"
        )
        for action in recallSelection {
            _ = applyTextMutation(action, client: recallTextView)
        }
        for action in try recallEngine.process(.enter) {
            _ = applyTextMutation(action, client: recallTextView)
        }
        try expect(
            recallTextView.string == "浅煎り" && recallTextView.markedRange().length == 0,
            "expanded recall should commit the selected surface through AppKit"
        )

        let transformEngine = try RustEngine(dataDirectory: testDirectory)
        for scalar in "nihongo".unicodeScalars {
            _ = try transformEngine.process(.character(scalar))
        }
        let halfKatakana = try transformEngine.process(.transformHalfKatakana)
        try expect(
            halfKatakana.contains(where: { $0.type == "update_preedit" && $0.text == "ﾆﾎﾝｺﾞ" }),
            "F8 event should expose half-width Katakana through the adapter"
        )

        try expect(
            DateCandidateFormat.allMask == 127,
            "all seven date candidate formats should be enabled by default"
        )
        let dateEngine = try RustEngine(dataDirectory: testDirectory)
        _ = try dateEngine.setOptions(
            liveConversion: false,
            historyCompletion: false,
            dateFormatMask: DateCandidateFormat.shortReiwa.rawValue
        )
        for scalar in "kyou".unicodeScalars {
            _ = try dateEngine.process(.character(scalar))
        }
        let dateActions = try dateEngine.process(.space)
        let configuredDateCandidates = try expectValue(
            dateActions.first(where: { $0.type == "show_candidates" })?.candidates,
            "configured date candidates should cross the Swift bridge"
        )
        let configuredDateDetails = try expectValue(
            dateActions.first(where: { $0.type == "show_candidates" })?.candidateDetails,
            "date candidate metadata should cross the Swift bridge"
        )
        try expect(
            configuredDateCandidates.contains(where: {
                $0.hasPrefix("R") && $0.filter { $0 == "/" }.count == 2
            }),
            "the enabled abbreviated Reiwa format should be offered"
        )
        try expect(
            !configuredDateCandidates.contains(where: {
                $0.count == 10 && $0.dropFirst(4).first == "/"
            }),
            "disabled Gregorian numeric formats should not be offered"
        )
        try expect(
            configuredDateDetails.contains(where: {
                $0.annotation == UInt32(SLIME_CANDIDATE_ANNOTATION_DATE_TIME.rawValue)
                    && candidateAnnotationText($0) == "日付・時刻"
            }),
            "date candidates should carry a localized semantic annotation"
        )

        let reconversionEngine = try RustEngine(dataDirectory: testDirectory)
        let reconversionActions = try reconversionEngine.beginReconversion(surface: "日本")
        try expect(
            reconversionActions.contains(where: {
                $0.type == "show_candidates" && $0.candidates?.contains("日本") == true
            }),
            "selected 日本 should enter conversion through the reverse dictionary"
        )
        let reconversionView = NSTextView(frame: .zero)
        reconversionView.string = "前日本後"
        reconversionView.setSelectedRange(NSRange(location: 1, length: 2))
        var replacement: NSRange? = NSRange(location: 1, length: 2)
        for action in reconversionActions {
            if applyTextMutation(action, client: reconversionView, replacementRange: replacement) != nil,
               action.type == "update_preedit"
            {
                replacement = nil
            }
        }
        try expect(
            reconversionView.string == "前日本後" && reconversionView.markedRange().location == 1,
            "reconversion should mark only the selected replacement range"
        )

        let f7Event = try expectValue(
            NSEvent.keyEvent(
                with: .keyDown,
                location: .zero,
                modifierFlags: [],
                timestamp: 0,
                windowNumber: 0,
                context: nil,
                characters: "",
                charactersIgnoringModifiers: "",
                isARepeat: false,
                keyCode: 98
            ),
            "F7 event should be created"
        )
        guard case .engine(.transformFullKatakana)? = fixedInputAction(
            from: f7Event,
            hasComposition: true,
            hasCandidates: false
        ) else {
            throw TestFailure(message: "fixed keymap should map F7 to full-width Katakana")
        }

        let shiftedRight = try expectValue(
            NSEvent.keyEvent(
                with: .keyDown,
                location: .zero,
                modifierFlags: .shift,
                timestamp: 0,
                windowNumber: 0,
                context: nil,
                characters: "",
                charactersIgnoringModifiers: "",
                isARepeat: false,
                keyCode: 124
            ),
            "Shift-Right event should be created"
        )
        guard case .engine(.expandSegment)? = fixedInputAction(
            from: shiftedRight,
            hasComposition: true,
            hasCandidates: true
        ) else {
            throw TestFailure(message: "fixed keymap should map Shift-Right to segment expansion")
        }

        let reconvertEvent = try expectValue(
            NSEvent.keyEvent(
                with: .keyDown,
                location: .zero,
                modifierFlags: [.control, .shift],
                timestamp: 0,
                windowNumber: 0,
                context: nil,
                characters: "R",
                charactersIgnoringModifiers: "r",
                isARepeat: false,
                keyCode: 15
            ),
            "reconversion shortcut event should be created"
        )
        guard case .reconvert? = fixedInputAction(
            from: reconvertEvent,
            hasComposition: false,
            hasCandidates: false
        ) else {
            throw TestFailure(message: "Ctrl-Shift-R should request selected-text reconversion")
        }

        let engine = try RustEngine(dataDirectory: testDirectory)
        try expect(
            !engine.hasNeuralReranker
                && engine.makeLiveNeuralTask(
                    minimumSwitchMargin: 0.5,
                    longReadingMinimumSwitchMargin: 0.6,
                    numericBaseSwitchMargin: 0.1,
                    longReadingLambda: 0.2
                ) == nil,
            "the default adapter build should keep delayed neural LIVE ranking unavailable"
        )
        try expect(
            liveNeuralDebounceDelay(configured: 0.180, readingCharacterCount: 8) == 0.120,
            "short LIVE readings should start after the lower debounce"
        )
        try expect(
            liveNeuralDebounceDelay(configured: 0.180, readingCharacterCount: 9) == 0.180,
            "long LIVE readings should retain the configured debounce"
        )
        try expect(
            liveNeuralDebounceDelay(configured: 0.080, readingCharacterCount: 8) == 0.080,
            "the short-reading policy must not increase a faster configured debounce"
        )
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

        for scalar in "jishowokakujuusasemashou".unicodeScalars {
            _ = try engine.process(.character(scalar))
        }
        let phraseConversion = try engine.process(.space)
        try expect(
            phraseConversion.contains(where: {
                $0.type == "update_preedit" && $0.text == "辞書を拡充させましょう"
            }),
            "connected phrase conversion should pass through the Swift adapter"
        )

        _ = try engine.process(.enter)
        let conservativeLiveEngine = try RustEngine(dataDirectory: testDirectory)
        _ = try conservativeLiveEngine.setOptions(
            liveConversion: true,
            historyCompletion: false,
            historyLearning: false,
            dictionaryPacks: 0b111
        )
        var livePreedit: String?
        for scalar in "soushimashou".unicodeScalars {
            let actions = try conservativeLiveEngine.process(.character(scalar))
            livePreedit = actions.last(where: { $0.type == "update_preedit" })?.text
            try expect(
                livePreedit?.contains("総") != true && livePreedit?.contains("島") != true,
                "live conversion should defer an ambiguous unfinished phrase"
            )
        }
        try expect(
            livePreedit == "そうしましょう",
            "conservative live conversion should preserve the completed reading"
        )
        _ = try conservativeLiveEngine.process(.enter)

        for scalar in "nihon".unicodeScalars {
            let actions = try conservativeLiveEngine.process(.character(scalar))
            livePreedit = actions.last(where: { $0.type == "update_preedit" })?.text
        }
        try expect(livePreedit == "日本", "live conversion should convert a stable word")
        livePreedit = try conservativeLiveEngine.process(.character("g")).last(where: {
            $0.type == "update_preedit"
        })?.text
        try expect(
            livePreedit == "日本g",
            "unfinished romaji should preserve the surface for the unchanged kana reading"
        )
        livePreedit = try conservativeLiveEngine.process(.character("o")).last(where: {
            $0.type == "update_preedit"
        })?.text
        try expect(livePreedit == "日本語", "resolved kana should extend the live conversion")
        _ = try conservativeLiveEngine.process(.enter)

        for scalar in "raibuhenkan".unicodeScalars {
            let actions = try conservativeLiveEngine.process(.character(scalar))
            livePreedit = actions.last(where: { $0.type == "update_preedit" })?.text
        }
        try expect(livePreedit == "ライブ変換", "live conversion should convert the stable prefix")
        livePreedit = try conservativeLiveEngine.process(.character("d")).last(where: {
            $0.type == "update_preedit"
        })?.text
        try expect(livePreedit == "ライブ変換d", "pending romaji should preserve the stable prefix")
        livePreedit = try conservativeLiveEngine.process(.character("e")).last(where: {
            $0.type == "update_preedit"
        })?.text
        try expect(
            livePreedit == "ライブ変換で",
            "a lattice-confirmed suffix must not roll the full preedit back"
        )
        _ = try conservativeLiveEngine.process(.enter)

        for scalar in "raibuhenkannno".unicodeScalars {
            let actions = try conservativeLiveEngine.process(.character(scalar))
            livePreedit = actions.last(where: { $0.type == "update_preedit" })?.text
        }
        try expect(
            livePreedit == "ライブ変換の",
            "a new n-syllable after ん must not create a phantom ん in live conversion"
        )
        _ = try conservativeLiveEngine.process(.enter)

        for scalar in "henkanga".unicodeScalars {
            let actions = try conservativeLiveEngine.process(.character(scalar))
            livePreedit = actions.last(where: { $0.type == "update_preedit" })?.text
        }
        try expect(livePreedit == "変換が", "live conversion should establish a stable prefix")
        for scalar in "tsu".unicodeScalars {
            let actions = try conservativeLiveEngine.process(.character(scalar))
            livePreedit = actions.last(where: { $0.type == "update_preedit" })?.text
        }
        try expect(
            livePreedit == "変換がつ",
            "a competitive full-lattice extension must not roll the whole preedit back"
        )
        for scalar in "duku".unicodeScalars {
            let actions = try conservativeLiveEngine.process(.character(scalar))
            livePreedit = actions.last(where: { $0.type == "update_preedit" })?.text
        }
        try expect(
            livePreedit == "変換が続く",
            "a stable bunsetsu should leave the target while its suffix continues converting"
        )
        _ = try conservativeLiveEngine.process(.enter)

        for scalar in "tashika".unicodeScalars {
            let actions = try conservativeLiveEngine.process(.character(scalar))
            livePreedit = actions.last(where: { $0.type == "update_preedit" })?.text
        }
        try expect(livePreedit == "確か", "live conversion should convert tashika")
        livePreedit = try conservativeLiveEngine.process(.character("n")).last(where: {
            $0.type == "update_preedit"
        })?.text
        try expect(livePreedit == "確かn", "ambiguous n should not roll the surface back")
        livePreedit = try conservativeLiveEngine.process(.character("a")).last(where: {
            $0.type == "update_preedit"
        })?.text
        try expect(livePreedit == "確かな", "resolved na should rerank the full reading")
        _ = try conservativeLiveEngine.process(.enter)

        for scalar in "kyouhaii".unicodeScalars {
            let actions = try conservativeLiveEngine.process(.character(scalar))
            livePreedit = actions.last(where: { $0.type == "update_preedit" })?.text
        }
        try expect(
            livePreedit == "今日はいい",
            "the full best path should confirm a literal suffix extension"
        )
        _ = try conservativeLiveEngine.process(.enter)

        livePreedit = nil
        for scalar in "ichigachigau".unicodeScalars {
            let actions = try conservativeLiveEngine.process(.character(scalar))
            livePreedit = actions.last(where: { $0.type == "update_preedit" })?.text
        }
        try expect(
            livePreedit == "いちがちがう",
            "live conversion must not combine 1勝ち with the suffix がう"
        )
        _ = try conservativeLiveEngine.process(.enter)

        livePreedit = nil
        for scalar in "nihongo".unicodeScalars {
            let actions = try conservativeLiveEngine.process(.character(scalar))
            livePreedit = actions.last(where: { $0.type == "update_preedit" })?.text
        }
        try expect(livePreedit == "日本語", "live conversion should produce a stable word")
        _ = try conservativeLiveEngine.process(.escape)
        livePreedit = try conservativeLiveEngine.process(.character("w")).last(where: {
            $0.type == "update_preedit"
        })?.text
        livePreedit = try conservativeLiveEngine.process(.character("o")).last(where: {
            $0.type == "update_preedit"
        })?.text
        try expect(
            livePreedit == "にほんごを",
            "Escape should suppress live conversion until the composition ends"
        )

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
        try testSignedDictionaryPackConstructor(
            in: testDirectory.appendingPathComponent("signed-pack-constructor")
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

    private static func convertAndCommit(
        input: String,
        surface: String,
        using engine: RustEngine
    ) throws {
        for scalar in input.unicodeScalars {
            _ = try engine.process(.character(scalar))
        }
        let conversion = try engine.process(.space)
        let candidates = try expectValue(
            conversion.first(where: { $0.type == "show_candidates" })?.candidates,
            "conversion candidates should cross the Swift bridge"
        )
        let index = try expectValue(
            candidates.firstIndex(of: surface),
            "conversion candidates for \(input) should contain \(surface)"
        )
        let selection = try engine.process(.selectCandidate(UInt32(index)))
        try expect(
            selection.contains(where: {
                $0.type == "update_preedit" && $0.text == surface
            }),
            "candidate selection should update the Swift preedit"
        )
        let commit = try engine.process(.enter)
        try expect(
            commit.contains(where: { $0.type == "commit" && $0.text == surface }),
            "candidate selection should commit through the Swift bridge"
        )
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
        let contextHistoryData = Data(
            (
                "# slime-context-history-v1\n"
                    + "ぶんしょう\t文章\tにほん\t日本\t2\t100\n"
                    + "さいてきか\t最適化\tぱふぉーまんす\tパフォーマンス\t2\t200\n"
            ).utf8
        )
        try contextHistoryData.write(to: store.contextHistoryURL, options: .atomic)
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
        let remainingContext = try String(
            contentsOf: store.contextHistoryURL,
            encoding: .utf8
        )
        try expect(
            !remainingContext.contains("日本") && remainingContext.contains("パフォーマンス"),
            "individual history deletion should remove related context only"
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

        let beforeClear = try store.loadHistorySnapshot()
        _ = try store.clearHistory(replacing: beforeClear.base)
        let clearedContext = try String(
            contentsOf: store.contextHistoryURL,
            encoding: .utf8
        )
        try expect(
            clearedContext == "# slime-context-history-v1\n",
            "clearing history should also clear contextual learning"
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
        let plainTabSeparated = Data(
            "\u{FEFF}# exported dictionary\nパフォーマンス\tPerformance\t名詞\nぱふぇ\tパフェ\t名詞\nぱふぇ\tパフェ\t名詞\ninvalid\n".utf8
        )
        let plainTabResult = try DictionaryImporter.parse(
            data: plainTabSeparated,
            fileExtension: "txt"
        )
        try expect(plainTabResult.formatName == "タブ区切り辞書", "plain tab format name")
        try expect(
            plainTabResult.entries.map(\.reading) == ["ぱふぉーまんす", "ぱふぇ"],
            "tab-separated readings should normalize and preserve order"
        )
        try expect(
            plainTabResult.skippedCount == 2,
            "invalid and duplicate tab-separated rows should be reported"
        )

        let singleBangHeader = Data(
            "!Microsoft IME Dictionary Tool\nにほん\t日本\t名詞\n".utf8
        )
        let singleBangResult = try DictionaryImporter.parse(
            data: singleBangHeader,
            fileExtension: "txt"
        )
        try expect(
            singleBangResult.formatName == "ヘッダー付きタブ区切り辞書",
            "single-bang header should be detected"
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

        let doubleBangHeader = Data(
            "!!ATOK_TANGO_TEXT_HEADER 1\nりんぎしょ\t稟議書\t固有人一般\n".utf8
        )
        let doubleBangResult = try DictionaryImporter.parse(
            data: doubleBangHeader,
            fileExtension: "txt"
        )
        try expect(
            doubleBangResult.formatName == "ヘッダー付きタブ区切り辞書",
            "double-bang header should be detected"
        )
        try expect(
            doubleBangResult.entries.first?.surface == "稟議書",
            "double-bang rows should import"
        )

        let quotedCSV = Data(
            "// exported dictionary\n\"いんよう\",\"「引用」\",\"普通名詞\"\n\"だぶる\",\"二重\"\"引用\",\"普通名詞\"\n".utf8
        )
        let quotedCSVResult = try DictionaryImporter.parse(
            data: quotedCSV,
            fileExtension: "txt"
        )
        try expect(
            quotedCSVResult.formatName == "CSV辞書",
            "quoted CSV should be detected"
        )
        try expect(
            quotedCSVResult.entries.map(\.surface) == ["「引用」", "二重\"引用"],
            "CSV quoting should be decoded"
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
        let packDirectory = directory.appendingPathComponent(
            "dictionary-packs",
            isDirectory: true
        )
        try FileManager.default.createDirectory(
            at: packDirectory,
            withIntermediateDirectories: true
        )
        let packEntries = "てすとようご\t試験用語\n"
        let packManifest = """
            # slime-dictionary-pack-v2
            # id: sample-general
            # name: 一般語彙サンプル
            # version: 2026.08.1
            # license: Example-Test-Only
            # minimum-slime-version: 0.1.0
            # published-at: 2026-08-01
            # provenance: fixture/generated/sample-general
            # entries-sha256: 735e22698dc5079e2214898a42397cae6c0ce86aecf740011b3440946086947f
            # entries
            """ + "\n" + packEntries
        try Data(packManifest.utf8).write(
            to: packDirectory.appendingPathComponent("sample.slime-dict")
        )

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

        let catalog = try engine.installedDictionaryPacks()
        try expect(
            catalog.packs == [
                InstalledDictionaryPack(
                    id: "sample-general",
                    formatVersion: 2,
                    name: "一般語彙サンプル",
                    version: "2026.08.1",
                    license: "Example-Test-Only",
                    minimumSlimeVersion: "0.1.0",
                    publishedAt: "2026-08-01",
                    provenance: "fixture/generated/sample-general",
                    entriesSHA256: "735e22698dc5079e2214898a42397cae6c0ce86aecf740011b3440946086947f",
                    packSHA256: "4c599a843d7261c5b52fb8558bde5d54b2de30a739280c8c4977e59040606b31",
                    entryCount: 1,
                    contextRuleCount: 0
                ),
            ] && catalog.errors.isEmpty,
            "installed dictionary metadata should cross the Swift/C/Rust boundary: \(catalog)"
        )
        let installedWords = try engine.installedDictionaryPackWords(id: "sample-general")
        try expect(
            installedWords.contains(where: {
                $0.reading == "てすとようご" && $0.surface == "試験用語"
            }),
            "installed dictionary words should cross the Swift/C/Rust boundary"
        )

        let installedEngine = try RustEngine(dataDirectory: directory)
        for scalar in "てすとようご".unicodeScalars {
            _ = try installedEngine.process(.character(scalar))
        }
        let installedActions = try installedEngine.process(.space)
        try expect(
            installedActions.contains(where: {
                $0.type == "show_candidates" && $0.candidates?.contains("試験用語") == true
            }),
            "installed dictionary should participate in conversion"
        )
    }

    private static func testSignedDictionaryPackConstructor(in directory: URL) throws {
        let keys = "fixture-2026-a\t"
            + "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a\n"
        _ = try RustEngine(
            dataDirectory: directory,
            dictionaryPackVerificationKeys: keys,
            dictionaryPackVersionFloors: "sample-general\t2026.08.1\n"
        )
        do {
            _ = try RustEngine(
                dataDirectory: directory,
                dictionaryPackVerificationKeys: "fixture-2026-a\tinvalid\n"
            )
            throw TestFailure(message: "invalid pack verification keys should fail creation")
        } catch RustEngine.EngineError.creationFailed {
            // Expected.
        }
        do {
            _ = try RustEngine(
                dataDirectory: directory,
                dictionaryPackVerificationKeys: keys,
                dictionaryPackVersionFloors: "sample-general\t2026.08\n"
            )
            throw TestFailure(message: "invalid pack version floors should fail creation")
        } catch RustEngine.EngineError.creationFailed {
            // Expected.
        }
        do {
            _ = try RustEngine(
                dataDirectory: directory,
                dictionaryPackVersionFloors: "sample-general\t2026.08.1\n"
            )
            throw TestFailure(message: "version floors without trusted keys should fail creation")
        } catch RustEngine.EngineError.creationFailed {
            // Expected.
        }
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
