import AppKit
import InputMethodKit
import os

final class UnvalleyController: IMKInputController {
    private static let performanceLog = OSLog(
        subsystem: "com.unvalley.inputmethod.Unvalley",
        category: .pointsOfInterest
    )

    private let engine: RustEngine
    private var hasComposition = false

    override init!(server: IMKServer!, delegate: Any!, client inputClient: Any!) {
        guard let engine = try? RustEngine() else {
            return nil
        }
        self.engine = engine
        super.init(server: server, delegate: delegate, client: inputClient)
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

        let mappedEvent: RustEngine.Event?
        switch event.keyCode {
        case 36, 76:
            mappedEvent = .enter
        case 49:
            mappedEvent = .space
        case 51:
            mappedEvent = .backspace
        case 53:
            mappedEvent = .escape
        default:
            mappedEvent = characterEvent(from: event)
        }

        guard let mappedEvent else {
            commitIfNeeded(client: sender)
            return false
        }

        return process(mappedEvent, client: sender)
    }

    override func commitComposition(_ sender: Any!) {
        commitIfNeeded(client: sender)
    }

    override func deactivateServer(_ sender: Any!) {
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

        do {
            let actions = try engine.process(event)
            var forwarded = false
            for action in actions {
                switch action.type {
                case "update_preedit":
                    let text = action.text ?? ""
                    hasComposition = !text.isEmpty
                    inputClient.setMarkedText(
                        text,
                        selectionRange: NSRange(location: text.utf16.count, length: 0),
                        replacementRange: NSRange(location: NSNotFound, length: NSNotFound)
                    )
                case "commit":
                    inputClient.insertText(
                        action.text ?? "",
                        replacementRange: NSRange(location: NSNotFound, length: NSNotFound)
                    )
                    hasComposition = false
                case "clear":
                    inputClient.setMarkedText(
                        "",
                        selectionRange: NSRange(location: 0, length: 0),
                        replacementRange: NSRange(location: NSNotFound, length: NSNotFound)
                    )
                    hasComposition = false
                case "forward_key":
                    forwarded = true
                case "show_candidates", "hide_candidates":
                    break
                default:
                    NSLog("Unvalley IME: unknown action %@", action.type)
                }
            }
            return !forwarded
        } catch {
            NSLog("Unvalley IME: Rust engine error: %@", String(describing: error))
            return false
        }
    }

    private func commitIfNeeded(client sender: Any!) {
        guard hasComposition else { return }
        _ = process(.enter, client: sender)
    }
}
