import Foundation

func precedingDocumentContext(
    selectedRange: NSRange,
    maximumCharacters: Int = 128,
    fetch: (NSRange) -> String?
) -> String? {
    guard selectedRange.location != NSNotFound,
          maximumCharacters > 0,
          maximumCharacters <= Int.max / 2
    else {
        return nil
    }
    guard selectedRange.location > 0 else { return "" }

    // IMKTextInput uses UTF-16 offsets while the engine bound is in Characters.
    // Fetch at most two UTF-16 units per requested Character, then cap again
    // after decoding so a client can never return an unbounded document prefix.
    let requestedLength = min(selectedRange.location, maximumCharacters * 2)
    let range = NSRange(
        location: selectedRange.location - requestedLength,
        length: requestedLength
    )
    guard let text = fetch(range) else { return nil }
    return String(text.suffix(maximumCharacters))
}

struct InputContextBoundary {
    private enum Selection: Equatable {
        case range(NSRange)
        case unavailable
    }

    private struct Observation: Equatable {
        let client: ObjectIdentifier
        let selection: Selection
    }

    private var previous: Observation?

    mutating func shouldReset(client: AnyObject, selectedRange: NSRange) -> Bool {
        let current = observation(client: client, selectedRange: selectedRange)
        guard let previous else {
            self.previous = current
            return false
        }
        self.previous = current
        guard previous.client == current.client else {
            return true
        }
        guard case let .range(previousRange) = previous.selection,
              case let .range(currentRange) = current.selection
        else {
            return true
        }
        return previousRange != currentRange
    }

    mutating func observe(client: AnyObject, selectedRange: NSRange) {
        previous = observation(client: client, selectedRange: selectedRange)
    }

    mutating func clear() {
        previous = nil
    }

    private func observation(client: AnyObject, selectedRange: NSRange) -> Observation {
        let selection: Selection = if selectedRange.location == NSNotFound
            || selectedRange.length == NSNotFound
        {
            .unavailable
        } else {
            .range(selectedRange)
        }
        return Observation(client: ObjectIdentifier(client), selection: selection)
    }
}
