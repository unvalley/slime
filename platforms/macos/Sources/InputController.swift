import AppKit
import InputMethodKit
import os

final class SlimeController: IMKInputController {
    private static let performanceLog = OSLog(
        subsystem: "com.unvalley.inputmethod.Slime",
        category: .pointsOfInterest
    )

    private let engine: RustEngine
    private let candidatePanel: CandidatePanel
    private var hasComposition = false
    private var candidateValues: [String] = []
    private var selectedCandidateIndex = 0
    private var appliedOptions: InputRuntimeOptions?

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

        let commandModifiers = event.modifierFlags.intersection([.command, .control, .option])
        if !commandModifiers.isEmpty {
            commitIfNeeded(client: sender)
            return false
        }

        if shouldForwardBackspaceDirectly(
            keyCode: event.keyCode,
            hasComposition: hasComposition
        ) {
            return false
        }

        if let index = candidateSelectionIndex(
            keyCode: event.keyCode,
            candidateCount: candidateValues.count,
            pageStart: (selectedCandidateIndex / 9) * 9
        ) {
            selectCandidate(at: index, commit: true)
            return true
        }

        let mappedEvent: RustEngine.Event?
        switch event.keyCode {
        case 36, 76:
            mappedEvent = .enter
        case 49:
            mappedEvent = .space
        case 48 where !candidateValues.isEmpty:
            mappedEvent = .acceptCandidate
        case 51:
            mappedEvent = .backspace
        case 53:
            mappedEvent = .escape
        case 125 where !candidateValues.isEmpty:
            mappedEvent = .nextCandidate
        case 126 where !candidateValues.isEmpty:
            mappedEvent = .previousCandidate
        default:
            mappedEvent = characterEvent(from: event)
        }

        guard let mappedEvent else {
            if !candidateValues.isEmpty {
                return false
            }
            commitIfNeeded(client: sender)
            return false
        }

        return process(mappedEvent, client: sender)
    }

    override func commitComposition(_ sender: Any!) {
        commitIfNeeded(client: sender)
    }

    override func deactivateServer(_ sender: Any!) {
        hideCandidates()
        commitIfNeeded(client: client())
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

        do {
            let actions = try engine.process(event)
            let forwarded = apply(actions, client: inputClient)
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
    }

    private func apply(
        _ actions: [RustEngine.Action],
        client inputClient: any IMKTextInput & NSObjectProtocol
    ) -> Bool {
        var forwarded = false
        let textClient = IMKTextMutationClient(base: inputClient)
        for action in actions {
            if let compositionState = applyTextMutation(action, client: textClient) {
                hasComposition = compositionState
                continue
            }
            switch action.type {
            case "forward_key":
                forwarded = true
            case "show_candidates":
                showCandidates(
                    action.candidates ?? [],
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
            secureEventInput: secureEventInputIsEnabled()
        )
        guard force || options != appliedOptions else {
            return true
        }

        do {
            let actions = try engine.setOptions(
                liveConversion: options.liveConversion,
                historyCompletion: options.historyCompletion,
                historyLearning: options.historyLearning,
                dictionaryPacks: options.dictionaryPacks
            )
            appliedOptions = options
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

    private func showCandidates(
        _ candidates: [String],
        selected: Int,
        client inputClient: any IMKTextInput & NSObjectProtocol
    ) {
        guard !candidates.isEmpty else {
            hideCandidates()
            return
        }

        candidateValues = candidates
        selectedCandidateIndex = selected
        candidatePanel.show(
            candidates: candidates,
            selected: selected,
            anchor: candidateAnchorRect(client: inputClient)
        )
    }

    private func hideCandidates() {
        candidatePanel.hide()
        candidateValues.removeAll(keepingCapacity: true)
        selectedCandidateIndex = 0
    }

    private func selectCandidate(at index: Int, commit: Bool) {
        guard candidateValues.indices.contains(index) else { return }
        _ = process(.selectCandidate(UInt32(index)), client: client())
        if commit {
            _ = process(.enter, client: client())
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
