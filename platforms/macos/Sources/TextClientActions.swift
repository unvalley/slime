import AppKit
import InputMethodKit

protocol TextMutationClient {
    func replaceMarkedText(
        _ string: Any,
        selectionRange: NSRange,
        replacementRange: NSRange
    )
    func insertText(_ string: Any, replacementRange: NSRange)
}

extension NSTextView: TextMutationClient {
    func replaceMarkedText(
        _ string: Any,
        selectionRange: NSRange,
        replacementRange: NSRange
    ) {
        setMarkedText(
            string,
            selectedRange: selectionRange,
            replacementRange: replacementRange
        )
    }
}

struct IMKTextMutationClient: TextMutationClient {
    let base: any IMKTextInput & NSObjectProtocol

    func replaceMarkedText(
        _ string: Any,
        selectionRange: NSRange,
        replacementRange: NSRange
    ) {
        base.setMarkedText(
            string,
            selectionRange: selectionRange,
            replacementRange: replacementRange
        )
    }

    func insertText(_ string: Any, replacementRange: NSRange) {
        base.insertText(string, replacementRange: replacementRange)
    }
}

/// Clients choose how to draw a composition handed over as a plain string,
/// and some (notably web-based editors) fall back to a selection-like
/// highlight. Explicit attributes request the standard thin underline.
private func markedTextAttributes(_ text: String) -> NSAttributedString {
    NSAttributedString(
        string: text,
        attributes: [
            .underlineStyle: NSUnderlineStyle.single.rawValue,
            .markedClauseSegment: 0,
        ]
    )
}

/// Applies text-mutating engine actions. The optional return value is the new
/// composition state; `nil` means the action belongs to another UI surface.
@discardableResult
func applyTextMutation(
    _ action: RustEngine.Action,
    client: some TextMutationClient
) -> Bool? {
    let notFoundRange = NSRange(location: NSNotFound, length: NSNotFound)
    switch action.type {
    case "update_preedit":
        let text = action.text ?? ""
        client.replaceMarkedText(
            markedTextAttributes(text),
            selectionRange: NSRange(location: text.utf16.count, length: 0),
            replacementRange: notFoundRange
        )
        return !text.isEmpty
    case "commit":
        client.insertText(action.text ?? "", replacementRange: notFoundRange)
        return false
    case "clear":
        client.replaceMarkedText(
            "",
            selectionRange: NSRange(location: 0, length: 0),
            replacementRange: notFoundRange
        )
        return false
    default:
        return nil
    }
}
