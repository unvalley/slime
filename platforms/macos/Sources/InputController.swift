import AppKit
import InputMethodKit
import os

@MainActor
final class SlimeController: IMKInputController {
    private static let performanceLog = OSLog(
        subsystem: "com.unvalley.inputmethod.Slime",
        category: .pointsOfInterest
    )

    private let engine: RustEngine
    private let candidatePanel: CandidatePanel
    private var hasComposition = false
    private var isSegmentedConversion = false
    private var candidateValues: [String] = []
    private var selectedCandidateIndex = 0
    private var appliedOptions: InputRuntimeOptions?
    private var replacementRangeOnNextUpdate: NSRange?
    private var inputContextBoundary = InputContextBoundary()
    private var needsExternalDocumentContext = true

    override init!(server: IMKServer!, delegate: Any!, client inputClient: Any!) {
        guard let engine = try? RustEngine() else {
            return nil
        }
        self.engine = engine
        candidatePanel = CandidatePanel()
        super.init(server: server, delegate: delegate, client: inputClient)
        _ = synchronizeOptions(force: true)
        candidatePanel.onCandidateClicked = { [weak self] index in
            self?.selectCandidate(at: index, commit: true)
        }
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(preferencesDidChange),
            name: .unvalleyPreferencesDidChange,
            object: nil
        )
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(userDataDidChange),
            name: .unvalleyUserDataDidChange,
            object: nil
        )
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
    }

    override func menu() -> NSMenu! {
        let menu = NSMenu(title: "Slime")
        let settings = NSMenuItem(
            title: "Slime設定…",
            action: #selector(openSettings(_:)),
            keyEquivalent: ","
        )
        settings.target = self
        menu.addItem(settings)
        return menu
    }

    override func handle(_ event: NSEvent!, client sender: Any!) -> Bool {
        guard let event, event.type == .keyDown else { return false }
        guard let inputClient = sender as? (any IMKTextInput & NSObjectProtocol) else {
            return false
        }
        guard SlimeAccessController.shared.allowsInput else {
            DispatchQueue.main.async {
                SettingsWindowController.shared.present(initialTab: .license)
            }
            return true
        }
        if !hasComposition,
           inputContextBoundary.shouldReset(
               client: inputClient,
               selectedRange: inputClient.selectedRange()
           )
        {
            resetTransientContext()
        }
        defer {
            if !hasComposition {
                inputContextBoundary.observe(
                    client: inputClient,
                    selectedRange: inputClient.selectedRange()
                )
            }
        }
        let deleteSignpostID: OSSignpostID? = if event.keyCode == 51 || event.keyCode == 117 {
            OSSignpostID(log: Self.performanceLog)
        } else {
            nil
        }
        if let deleteSignpostID {
            os_signpost(
                .begin,
                log: Self.performanceLog,
                name: "HandleDelete",
                signpostID: deleteSignpostID,
                "composition=%{public}d keyCode=%{public}d",
                hasComposition,
                event.keyCode
            )
        }
        defer {
            if let deleteSignpostID {
                os_signpost(
                    .end,
                    log: Self.performanceLog,
                    name: "HandleDelete",
                    signpostID: deleteSignpostID
                )
            }
        }

        if shouldForwardBackspaceDirectly(
            keyCode: event.keyCode,
            hasComposition: hasComposition
        ) {
            resetTransientContext()
            return false
        }

        let candidateSelectionModifiers = event.modifierFlags.intersection([
            .shift, .command, .control, .option,
        ])
        if candidateSelectionModifiers.isEmpty, let index = candidateSelectionIndex(
            keyCode: event.keyCode,
            candidateCount: candidateValues.count,
            pageStart: (selectedCandidateIndex / 9) * 9
        ) {
            selectCandidate(at: index, commit: true)
            return true
        }

        if let action = fixedInputAction(
            from: event,
            hasComposition: hasComposition,
            hasCandidates: !candidateValues.isEmpty
        ) {
            switch action {
            case let .engine(engineEvent):
                return process(engineEvent, client: sender)
            case .reconvert:
                return beginReconversion(client: sender)
            }
        }

        let commandModifiers = event.modifierFlags.intersection([.command, .control, .option])
        if !commandModifiers.isEmpty {
            commitIfNeeded(client: sender)
            resetTransientContext()
            return false
        }

        guard let mappedEvent = characterEvent(from: event) else {
            if !candidateValues.isEmpty {
                return false
            }
            commitIfNeeded(client: sender)
            resetTransientContext()
            return false
        }

        return process(mappedEvent, client: sender)
    }

    override func commitComposition(_ sender: Any!) {
        commitIfNeeded(client: sender)
        resetTransientContext()
    }

    override func activateServer(_ sender: Any!) {
        resetTransientContext()
        super.activateServer(sender)
    }

    override func deactivateServer(_ sender: Any!) {
        hideCandidates()
        commitIfNeeded(client: client())
        resetTransientContext()
        super.deactivateServer(sender)
    }

    private func characterEvent(from event: NSEvent) -> RustEngine.Event? {
        printableInputScalar(from: event).map(RustEngine.Event.character)
    }

    @discardableResult
    private func process(_ event: RustEngine.Event, client sender: Any!) -> Bool {
        guard let inputClient = sender as? (any IMKTextInput & NSObjectProtocol) else {
            return false
        }

        guard synchronizeOptions(client: inputClient) else {
            return false
        }

        if case .character = event, !hasComposition {
            synchronizeExternalDocumentContextIfNeeded(client: inputClient)
        }

        do {
            let actions = try engine.process(event)
            let forwarded = apply(actions, client: inputClient)
            if forwarded {
                if hasComposition {
                    commitIfNeeded(client: inputClient)
                }
                resetTransientContext()
            }
            return !forwarded
        } catch {
            NSLog("Slime: Rust engine error: %@", String(describing: error))
            return false
        }
    }

    @objc private func openSettings(_ sender: Any?) {
        DispatchQueue.main.async {
            SettingsWindowController.shared.present()
        }
    }

    @objc private func preferencesDidChange() {
        guard let inputClient = client() else {
            return
        }
        _ = synchronizeOptions(force: true, client: inputClient)
    }

    @objc private func userDataDidChange() {
        guard let inputClient = client() else {
            return
        }
        do {
            let actions = try engine.reloadUserData()
            _ = apply(actions, client: inputClient)
        } catch {
            NSLog("Slime: failed to reload user data %@", String(describing: error))
        }
        resetTransientContext()
    }

    private func apply(
        _ actions: [RustEngine.Action],
        client inputClient: any IMKTextInput & NSObjectProtocol
    ) -> Bool {
        var forwarded = false
        let textClient = IMKTextMutationClient(base: inputClient)
        for action in actions {
            if let compositionState = applyTextMutation(
                action,
                client: textClient,
                replacementRange: replacementRangeOnNextUpdate
            ) {
                hasComposition = compositionState
                if action.type == "update_preedit" {
                    isSegmentedConversion = action.selectedStart != nil
                    replacementRangeOnNextUpdate = nil
                } else if !compositionState {
                    isSegmentedConversion = false
                }
                continue
            }
            switch action.type {
            case "forward_key":
                forwarded = true
            case "show_candidates":
                showCandidates(
                    action.candidates ?? [],
                    details: action.candidateDetails,
                    selected: action.selected ?? 0,
                    client: inputClient
                )
            case "hide_candidates":
                hideCandidates()
            case "update_preedit", "commit", "clear":
                assertionFailure("text actions must be handled before UI actions")
            default:
                NSLog("Slime: unknown action %@", action.type)
            }
        }
        return forwarded
    }

    @discardableResult
    private func synchronizeOptions(
        force: Bool = false,
        client inputClient: (any IMKTextInput & NSObjectProtocol)? = nil
    ) -> Bool {
        let options = InputRuntimeOptions(
            liveConversion: IMEPreferences.liveConversion,
            historyCompletion: IMEPreferences.historyCompletion,
            historyLearning: IMEPreferences.historyLearning,
            dictionaryPacks: IMEPreferences.dictionaryPacks,
            secureEventInput: secureEventInputIsEnabled(),
            dateFormatMask: IMEPreferences.dateCandidateFormats
        )
        guard force || options != appliedOptions else {
            return true
        }

        let previousPrivateMode = appliedOptions?.privateMode
        do {
            let actions = try engine.setOptions(
                liveConversion: options.liveConversion,
                historyCompletion: options.historyCompletion,
                historyLearning: options.historyLearning,
                dictionaryPacks: options.dictionaryPacks,
                privateMode: options.privateMode,
                dateFormatMask: options.dateFormatMask
            )
            appliedOptions = options
            if previousPrivateMode != options.privateMode {
                needsExternalDocumentContext = true
            }
            if let inputClient {
                _ = apply(actions, client: inputClient)
            }
            return true
        } catch {
            NSLog("Slime: failed to apply input options %@", String(describing: error))
            return false
        }
    }

    private func commitIfNeeded(client sender: Any!) {
        guard hasComposition else { return }
        _ = process(.enter, client: sender)
    }

    private func beginReconversion(client sender: Any!) -> Bool {
        resetTransientContext()
        guard let inputClient = sender as? (any IMKTextInput & NSObjectProtocol) else {
            return false
        }
        let selectedRange = inputClient.selectedRange()
        guard selectedRange.location != NSNotFound,
              selectedRange.length > 0,
              let selected = inputClient.attributedSubstring(from: selectedRange)?.string,
              !selected.isEmpty
        else {
            return false
        }
        do {
            let actions = try engine.beginReconversion(surface: selected)
            guard !actions.isEmpty else { return false }
            replacementRangeOnNextUpdate = selectedRange
            _ = apply(actions, client: inputClient)
            return true
        } catch {
            NSLog("Slime: failed to begin reconversion %@", String(describing: error))
            return false
        }
    }

    private func showCandidates(
        _ candidates: [String],
        details: [RustEngine.CandidateDetail]?,
        selected: Int,
        client inputClient: any IMKTextInput & NSObjectProtocol
    ) {
        let items = candidatePanelItems(candidates: candidates, details: details)
        guard !items.isEmpty else {
            hideCandidates()
            return
        }

        candidateValues = items.map(\.value)
        selectedCandidateIndex = selected
        candidatePanel.show(
            candidates: items,
            selected: selected,
            anchor: candidateAnchorRect(client: inputClient)
        )
    }

    private func candidatePanelItems(
        candidates: [String],
        details: [RustEngine.CandidateDetail]?
    ) -> [CandidatePanelItem] {
        guard let details, details.count == candidates.count else {
            return candidates.map { CandidatePanelItem(value: $0, annotation: nil) }
        }
        return details.map { detail in
            CandidatePanelItem(
                value: detail.value,
                annotation: candidateAnnotationText(detail)
            )
        }
    }

    private func hideCandidates() {
        candidatePanel.hide()
        candidateValues.removeAll(keepingCapacity: true)
        selectedCandidateIndex = 0
    }

    private func selectCandidate(at index: Int, commit: Bool) {
        guard candidateValues.indices.contains(index), let inputClient = client() else {
            return
        }
        _ = process(.selectCandidate(UInt32(index)), client: inputClient)
        if commit && !isSegmentedConversion {
            _ = process(.enter, client: inputClient)
        }
        inputContextBoundary.observe(
            client: inputClient,
            selectedRange: inputClient.selectedRange()
        )
    }

    private func resetTransientContext() {
        inputContextBoundary.clear()
        needsExternalDocumentContext = true
        do {
            try engine.resetContext()
        } catch {
            NSLog("Slime: failed to reset transient context %@", String(describing: error))
        }
    }

    private func synchronizeExternalDocumentContextIfNeeded(
        client inputClient: any IMKTextInput & NSObjectProtocol
    ) {
        guard needsExternalDocumentContext,
              appliedOptions?.privateMode == false
        else {
            return
        }
        needsExternalDocumentContext = false
        let selectedRange = inputClient.selectedRange()
        let leftContext = precedingDocumentContext(
            selectedRange: selectedRange
        ) { range in
            inputClient.attributedSubstring(from: range)?.string
        } ?? ""
        let rightContext = followingDocumentContext(
            selectedRange: selectedRange,
            documentLength: inputClient.length()
        ) { range in
            inputClient.attributedSubstring(from: range)?.string
        } ?? ""
        do {
            try engine.setExternalContext(left: leftContext, right: rightContext)
        } catch {
            NSLog(
                "Slime: failed to set transient document context %@",
                String(describing: error)
            )
        }
    }

    private func candidateAnchorRect(
        client inputClient: any IMKTextInput & NSObjectProtocol
    ) -> NSRect {
        func isUsable(_ rect: NSRect) -> Bool {
            let point = NSPoint(x: rect.midX, y: rect.midY)
            return rect.origin.x.isFinite
                && rect.origin.y.isFinite
                && rect.width.isFinite
                && rect.height.isFinite
                && (rect.width > 0 || rect.height > 0)
                && NSScreen.screens.contains { $0.frame.contains(point) }
        }

        let markedRange = inputClient.markedRange()
        let selectedRange = inputClient.selectedRange()

        var characterIndexes: [Int] = []
        if markedRange.location != NSNotFound {
            characterIndexes.append(markedRange.location)
        }
        if selectedRange.location != NSNotFound,
           !characterIndexes.contains(selectedRange.location)
        {
            characterIndexes.append(selectedRange.location)
        }
        if !characterIndexes.contains(0) {
            characterIndexes.append(0)
        }

        for characterIndex in characterIndexes {
            var lineHeightRect = NSRect.zero
            inputClient.attributes(
                forCharacterIndex: characterIndex,
                lineHeightRectangle: &lineHeightRect
            )
            if isUsable(lineHeightRect) {
                return lineHeightRect
            }
        }

        var rangeAttempts: [(range: NSRange, useTrailingEdge: Bool)] = []
        if markedRange.location != NSNotFound, markedRange.length > 0 {
            rangeAttempts.append((
                NSRange(location: NSMaxRange(markedRange) - 1, length: 1),
                true
            ))
        }
        if markedRange.location != NSNotFound {
            rangeAttempts.append((
                NSRange(location: NSMaxRange(markedRange), length: 0),
                false
            ))
        }
        if selectedRange.location != NSNotFound, selectedRange.location > 0 {
            rangeAttempts.append((
                NSRange(location: selectedRange.location - 1, length: 1),
                true
            ))
        }
        if selectedRange.location != NSNotFound {
            rangeAttempts.append((
                NSRange(location: selectedRange.location, length: 0),
                false
            ))
        }
        if rangeAttempts.isEmpty {
            rangeAttempts.append((NSRange(location: 0, length: 0), false))
        }

        for attempt in rangeAttempts {
            var actualRange = NSRange(location: NSNotFound, length: 0)
            let rect = inputClient.firstRect(
                forCharacterRange: attempt.range,
                actualRange: &actualRange
            )
            guard isUsable(rect) else {
                continue
            }

            if attempt.useTrailingEdge {
                return NSRect(x: rect.maxX, y: rect.minY, width: 0, height: rect.height)
            }
            return rect
        }

        return .zero
    }
}
