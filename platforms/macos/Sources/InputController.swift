import AppKit
import InputMethodKit
import os

@MainActor
final class SlimeController: IMKInputController {
    private static let liveNeuralQueue = DispatchQueue(
        label: "com.unvalley.inputmethod.Slime.live-neural",
        qos: .userInitiated
    )
    private static let performanceLog = OSLog(
        subsystem: "com.unvalley.inputmethod.Slime",
        category: .pointsOfInterest
    )
    private static let inputVerificationLog = OSLog(
        subsystem: "com.unvalley.inputmethod.Slime",
        category: "input-verification"
    )

    private let engine: RustEngine
    private let candidatePanel: CandidatePanel
    private let liveNeuralRerankingEnabled: Bool
    private let liveNeuralDebounce: TimeInterval
    private let liveNeuralMinimumSwitchMargin: Double
    private let liveNeuralLongReadingMinimumSwitchMargin: Double
    private let liveNeuralLongReadingLambda: Double
    private let liveNeuralNumericBaseSwitchMargin: Double
    private let inputVerificationToken: String?
    private let inputVerificationMode: InputVerification.Mode?
    private let inputVerificationTargetProcessIdentifier: Int32?
    private var hasComposition = false
    private var isSegmentedConversion = false
    private var candidateValues: [String] = []
    private var selectedCandidateIndex = 0
    private var appliedOptions: InputRuntimeOptions?
    private var replacementRangeOnNextUpdate: NSRange?
    private var didRecordInputVerification = false
    private var candidateVerificationState = InputVerification.CandidateSelectionState()
    private var reconversionVerificationState = InputVerification.ReconversionState()
    private var privacyVerificationState = InputVerification.PrivacyState()
    private var inputContextBoundary = InputContextBoundary()
    private var needsExternalDocumentContext = true
    private var liveNeuralGeneration: UInt64 = 0

    override init!(server: IMKServer!, delegate: Any!, client inputClient: Any!) {
        guard let engine = try? RustEngine() else {
            return nil
        }
        let liveNeuralRerankingEnabled = engine.hasNeuralReranker
            && (Bundle.main.object(
                forInfoDictionaryKey: "SlimeNeuralLiveRerankingEnabled"
            ) as? NSNumber)?.boolValue == true
        guard (try? engine.setLiveNeuralRerankingEnabled(
            liveNeuralRerankingEnabled
        )) != nil else {
            return nil
        }
        self.engine = engine
        candidatePanel = CandidatePanel()
        self.liveNeuralRerankingEnabled = liveNeuralRerankingEnabled
        let configuredDebounce = (Bundle.main.object(
            forInfoDictionaryKey: "SlimeNeuralLiveDebounceMilliseconds"
        ) as? NSNumber)?.doubleValue ?? 180
        liveNeuralDebounce = min(max(configuredDebounce, 50), 1_000) / 1_000
        let configuredMargin = (Bundle.main.object(
            forInfoDictionaryKey: "SlimeNeuralLiveMinSwitchMargin"
        ) as? NSNumber)?.doubleValue ?? 0.2
        liveNeuralMinimumSwitchMargin = configuredMargin.isFinite
            ? max(configuredMargin, 0)
            : 0.2
        let configuredLongReadingMargin = (Bundle.main.object(
            forInfoDictionaryKey: "SlimeNeuralLiveLongReadingMinSwitchMargin"
        ) as? NSNumber)?.doubleValue ?? 0.3
        liveNeuralLongReadingMinimumSwitchMargin = configuredLongReadingMargin.isFinite
            ? max(configuredLongReadingMargin, 0)
            : 0.3
        let configuredNumericBaseMargin = (Bundle.main.object(
            forInfoDictionaryKey: "SlimeNeuralLiveNumericBaseSwitchMargin"
        ) as? NSNumber)?.doubleValue ?? 0.1
        liveNeuralNumericBaseSwitchMargin = configuredNumericBaseMargin.isFinite
            ? max(configuredNumericBaseMargin, 0)
            : 0.1
        let baseLambda = (Bundle.main.object(
            forInfoDictionaryKey: "SlimeNeuralLambda"
        ) as? NSNumber)?.doubleValue ?? 0.2
        let configuredLongReadingLambda = (Bundle.main.object(
            forInfoDictionaryKey: "SlimeNeuralLiveLongReadingLambda"
        ) as? NSNumber)?.doubleValue ?? baseLambda
        liveNeuralLongReadingLambda = configuredLongReadingLambda.isFinite
            ? min(max(configuredLongReadingLambda, 0), 1)
            : min(max(baseLambda, 0), 1)
        let inputVerificationRequest = InputVerification.pendingRequest()
        inputVerificationToken = inputVerificationRequest?.token
        inputVerificationMode = inputVerificationRequest?.mode
        inputVerificationTargetProcessIdentifier =
            inputVerificationRequest?.targetProcessIdentifier
        super.init(server: server, delegate: delegate, client: inputClient)
        _ = synchronizeOptions(force: true)
        candidatePanel.onCandidateClicked = { [weak self] index, event in
            self?.recordCandidateSelectionMethod(.click, event: event)
            self?.selectCandidate(at: index, commit: true, verificationEvent: event)
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
        recordInputVerificationIfNeeded(event)
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
            recordCandidateSelectionMethod(.number, event: event)
            selectCandidate(at: index, commit: true, verificationEvent: event)
            return true
        }

        if let action = fixedInputAction(
            from: event,
            hasComposition: hasComposition,
            hasCandidates: !candidateValues.isEmpty
        ) {
            switch action {
            case let .engine(engineEvent):
                if event.keyCode == 125 || event.keyCode == 126 {
                    recordCandidateSelectionMethod(.arrow, event: event)
                }
                switch engineEvent {
                case .space:
                    recordPrivacyVerificationInput(.requestCandidates, event: event)
                case .enter, .acceptCandidate:
                    recordCandidateAcceptance(event)
                    recordReconversionAcceptance(event)
                    recordPrivacyVerificationInput(.accept, event: event)
                default:
                    break
                }
                return process(engineEvent, client: sender)
            case .reconvert:
                return beginReconversion(client: sender, verificationEvent: event)
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
                resetTransientContext()
                return false
            }
            commitIfNeeded(client: sender)
            resetTransientContext()
            return false
        }

        return process(mappedEvent, client: sender)
    }

    private func recordInputVerificationIfNeeded(_ event: NSEvent) {
        let disallowedModifiers = event.modifierFlags.intersection([
            .command, .control, .option,
        ])
        guard !didRecordInputVerification,
              let inputVerificationToken,
              let inputVerificationMode,
              isVerificationTargetActive(),
              isHardwareInputEvent(event),
              InputVerification.isVerificationCharacter(
                  event.charactersIgnoringModifiers,
                  hasDisallowedModifiers: !disallowedModifiers.isEmpty
              )
        else {
            return
        }
        switch inputVerificationMode {
        case .character:
            didRecordInputVerification = true
            os_log(
                .default,
                log: Self.inputVerificationLog,
                "InputMethodCharacterEvent token=%{public}@",
                inputVerificationToken as NSString
            )
            InputVerification.consume(inputVerificationToken)
        case .candidateSelection, .candidateNumber, .candidateClick:
            if let step = candidateVerificationState.record(.character) {
                recordCandidateVerificationStep(step)
            }
        case .reconversion:
            break
        case .privacyPrivate, .privacySecure, .privacyResume:
            guard privacyVerificationModeMatchesCurrentState() else { return }
            if let step = privacyVerificationState.record(.character) {
                recordPrivacyVerificationStep(step)
            }
        }
    }

    private func recordCandidateVerificationAction(_ action: RustEngine.Action) {
        guard inputVerificationMode?.selectionMethod != nil,
              !didRecordInputVerification,
              isVerificationTargetActive()
        else {
            return
        }
        let step: InputVerification.CandidateStep? = switch action.type {
        case "show_candidates" where !(action.candidates ?? []).isEmpty:
            candidateVerificationState.record(.candidates(selected: action.selected ?? 0))
        case "commit":
            candidateVerificationState.record(.commit)
        default:
            nil
        }
        if let step {
            recordCandidateVerificationStep(step)
        }
    }

    private func recordReconversionVerificationAction(_ action: RustEngine.Action) {
        guard inputVerificationMode == .reconversion,
              !didRecordInputVerification,
              isVerificationTargetActive()
        else {
            return
        }
        let event: InputVerification.ReconversionEvent? = switch action.type {
        case "show_candidates" where !(action.candidates ?? []).isEmpty:
            .candidates
        case "commit":
            .commit
        default:
            nil
        }
        if let event {
            recordReconversionVerificationEvent(event)
        }
    }

    private func recordPrivacyVerificationAction(_ action: RustEngine.Action) {
        guard inputVerificationMode?.matchesPrivacyState(
            privateMode: InputPrivacySession.isPrivate,
            secureEventInput: secureEventInputIsEnabled()
        ) == true,
        !didRecordInputVerification,
        isVerificationTargetActive()
        else {
            return
        }
        let event: InputVerification.PrivacyEvent? = switch action.type {
        case "show_candidates" where !(action.candidates ?? []).isEmpty:
            .candidates
        case "commit":
            .commit
        default:
            nil
        }
        if let event, let step = privacyVerificationState.record(event) {
            recordPrivacyVerificationStep(step)
        }
    }

    private func privacyVerificationModeMatchesCurrentState() -> Bool {
        inputVerificationMode?.matchesPrivacyState(
            privateMode: InputPrivacySession.isPrivate,
            secureEventInput: secureEventInputIsEnabled()
        ) == true
    }

    private func recordPrivacyVerificationStep(_ step: InputVerification.PrivacyStep) {
        guard let inputVerificationToken, let inputVerificationMode else { return }
        os_log(
            .default,
            log: Self.inputVerificationLog,
            "InputMethodPrivacyEvent token=%{public}@ mode=%{public}@ step=%{public}@",
            inputVerificationToken as NSString,
            inputVerificationMode.rawValue as NSString,
            step.rawValue as NSString
        )
        if step == .committed {
            didRecordInputVerification = true
            InputVerification.consume(inputVerificationToken)
        }
    }

    private func recordReconversionVerificationEvent(
        _ event: InputVerification.ReconversionEvent
    ) {
        guard inputVerificationMode == .reconversion,
              !didRecordInputVerification,
              isVerificationTargetActive(),
              let inputVerificationToken,
              let step = reconversionVerificationState.record(event)
        else {
            return
        }
        os_log(
            .default,
            log: Self.inputVerificationLog,
            "InputMethodReconversionEvent token=%{public}@ step=%{public}@",
            inputVerificationToken as NSString,
            step.rawValue as NSString
        )
        if step == .committed {
            didRecordInputVerification = true
            InputVerification.consume(inputVerificationToken)
        }
    }

    private func recordReconversionAcceptance(_ event: NSEvent) {
        guard inputVerificationMode == .reconversion,
              !didRecordInputVerification,
              isVerificationTargetActive(),
              isHardwareInputEvent(event)
        else {
            return
        }
        _ = reconversionVerificationState.record(.accept)
    }

    private func recordPrivacyVerificationInput(
        _ input: InputVerification.PrivacyEvent,
        event: NSEvent
    ) {
        guard privacyVerificationModeMatchesCurrentState(),
              !didRecordInputVerification,
              isVerificationTargetActive(),
              isHardwareInputEvent(event)
        else {
            return
        }
        _ = privacyVerificationState.record(input)
    }

    private func recordCandidateSelectionMethod(
        _ method: InputVerification.CandidateSelectionMethod,
        event: NSEvent
    ) {
        guard inputVerificationMode?.selectionMethod == method,
              !didRecordInputVerification,
              isVerificationTargetActive(),
              isHardwareInputEvent(event),
              let step = candidateVerificationState.record(.selection(method))
        else {
            return
        }
        recordCandidateVerificationStep(step)
    }

    private func recordCandidateAcceptance(_ event: NSEvent) {
        guard inputVerificationMode?.selectionMethod != nil,
              !didRecordInputVerification,
              isVerificationTargetActive(),
              isHardwareInputEvent(event)
        else {
            return
        }
        _ = candidateVerificationState.record(.accept)
    }

    private func isHardwareInputEvent(_ event: NSEvent) -> Bool {
        guard let cgEvent = event.cgEvent else { return false }
        return InputVerification.isHardwareEventSource(
            cgEvent.getIntegerValueField(.eventSourceStateID)
        )
    }

    private func isVerificationTargetActive() -> Bool {
        guard let inputVerificationTargetProcessIdentifier else { return false }
        return NSWorkspace.shared.frontmostApplication?.processIdentifier
            == inputVerificationTargetProcessIdentifier
    }

    private func recordCandidateVerificationStep(_ step: InputVerification.CandidateStep) {
        guard let inputVerificationToken else { return }
        os_log(
            .default,
            log: Self.inputVerificationLog,
            "InputMethodCandidateEvent token=%{public}@ step=%{public}@",
            inputVerificationToken as NSString,
            step.rawValue as NSString
        )
        if step == .candidateCommitted {
            didRecordInputVerification = true
            InputVerification.consume(inputVerificationToken)
        }
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

        invalidateLiveNeuralRerank()
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
            if !forwarded, eventCanScheduleLiveNeuralRerank(event) {
                scheduleLiveNeuralRerank()
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
        invalidateLiveNeuralRerank()
        guard let inputClient = client() else {
            return
        }
        _ = synchronizeOptions(force: true, client: inputClient)
    }

    @objc private func userDataDidChange() {
        invalidateLiveNeuralRerank()
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
            recordCandidateVerificationAction(action)
            recordReconversionVerificationAction(action)
            recordPrivacyVerificationAction(action)
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

    private func beginReconversion(client sender: Any!, verificationEvent: NSEvent) -> Bool {
        if isVerificationTargetActive(), isHardwareInputEvent(verificationEvent) {
            recordReconversionVerificationEvent(.requested)
        }
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
            recordReconversionVerificationEvent(.started)
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
        candidatePanel.show(candidates: items, selected: selected) {
            candidateAnchorRect(client: inputClient)
        }
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

    private func selectCandidate(
        at index: Int,
        commit: Bool,
        verificationEvent: NSEvent? = nil
    ) {
        guard candidateValues.indices.contains(index),
              let inputClient = client()
        else {
            return
        }
        _ = process(.selectCandidate(UInt32(index)), client: inputClient)
        if commit && !isSegmentedConversion {
            if let verificationEvent {
                recordCandidateAcceptance(verificationEvent)
            }
            _ = process(.enter, client: inputClient)
        }
        inputContextBoundary.observe(
            client: inputClient,
            selectedRange: inputClient.selectedRange()
        )
    }

    private func resetTransientContext() {
        invalidateLiveNeuralRerank()
        inputContextBoundary.clear()
        needsExternalDocumentContext = true
        do {
            try engine.resetContext()
        } catch {
            NSLog("Slime: failed to reset transient context %@", String(describing: error))
        }
    }

    private func eventCanScheduleLiveNeuralRerank(_ event: RustEngine.Event) -> Bool {
        switch event {
        case .character, .backspace:
            true
        default:
            false
        }
    }

    private func invalidateLiveNeuralRerank() {
        liveNeuralGeneration &+= 1
    }

    private func scheduleLiveNeuralRerank() {
        guard liveNeuralRerankingEnabled,
              appliedOptions?.liveConversion == true,
              appliedOptions?.privateMode == false,
              hasComposition,
              let inputClient = client(),
              let task = engine.makeLiveNeuralTask(
                  minimumSwitchMargin: liveNeuralMinimumSwitchMargin,
                  longReadingMinimumSwitchMargin: liveNeuralLongReadingMinimumSwitchMargin,
                  numericBaseSwitchMargin: liveNeuralNumericBaseSwitchMargin,
                  longReadingLambda: liveNeuralLongReadingLambda
              )
        else {
            return
        }
        let generation = liveNeuralGeneration
        let selectedRange = inputClient.selectedRange()
        let markedRange = inputClient.markedRange()
        let debounce = liveNeuralDebounceDelay(
            configured: liveNeuralDebounce,
            readingCharacterCount: task.readingCharacterCount
        )
        DispatchQueue.main.asyncAfter(deadline: .now() + debounce) { [weak self] in
            guard let self,
                  generation == liveNeuralGeneration,
                  let inputClient = client(),
                  inputClient.selectedRange() == selectedRange,
                  inputClient.markedRange() == markedRange
            else {
                return
            }
            let signpostID = OSSignpostID(log: Self.performanceLog)
            os_signpost(
                .begin,
                log: Self.performanceLog,
                name: "LiveNeuralRerank",
                signpostID: signpostID
            )
            Self.liveNeuralQueue.async { [weak self] in
                let succeeded = task.run()
                DispatchQueue.main.async { [weak self] in
                    defer {
                        os_signpost(
                            .end,
                            log: Self.performanceLog,
                            name: "LiveNeuralRerank",
                            signpostID: signpostID
                        )
                    }
                    guard let self,
                          succeeded,
                          generation == liveNeuralGeneration,
                          let inputClient = client(),
                          inputClient.selectedRange() == selectedRange,
                          inputClient.markedRange() == markedRange
                    else {
                        return
                    }
                    do {
                        let actions = try engine.apply(task)
                        if !actions.isEmpty {
                            _ = apply(actions, client: inputClient)
                        }
                    } catch {
                        NSLog(
                            "Slime: failed to apply delayed LIVE ranking %@",
                            String(describing: error)
                        )
                    }
                }
            }
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
        let context = precedingDocumentContext(
            selectedRange: inputClient.selectedRange()
        ) { range in
            inputClient.attributedSubstring(from: range)?.string
        } ?? ""
        do {
            try engine.setExternalLeftContext(context)
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
