import AppKit

enum FixedInputAction {
    case engine(RustEngine.Event)
    case reconvert
}

func fixedInputAction(
    from event: NSEvent,
    hasComposition: Bool,
    hasCandidates: Bool
) -> FixedInputAction? {
    let modifiers = event.modifierFlags.intersection([.shift, .control, .option, .command])
    if event.keyCode == 15,
       modifiers.contains([.shift, .control]),
       !modifiers.contains(.option),
       !modifiers.contains(.command),
       !hasComposition
    {
        return .reconvert
    }

    if modifiers.intersection([.control, .option, .command]).isEmpty {
        switch event.keyCode {
        case 97: return .engine(.transformHiragana)
        case 98: return .engine(.transformFullKatakana)
        case 100: return .engine(.transformHalfKatakana)
        case 101: return .engine(.transformFullAlphanumeric)
        case 109: return .engine(.transformHalfAlphanumeric)
        case 123 where hasComposition:
            return .engine(modifiers.contains(.shift) ? .shrinkSegment : .previousSegment)
        case 124 where hasComposition:
            return .engine(modifiers.contains(.shift) ? .expandSegment : .nextSegment)
        default: break
        }
    }

    guard modifiers.intersection([.control, .option, .command]).isEmpty else {
        return nil
    }
    switch event.keyCode {
    case 36, 76: return .engine(.enter)
    case 49: return .engine(.space)
    case 48 where hasCandidates: return .engine(.acceptCandidate)
    case 51: return .engine(.backspace)
    case 53: return .engine(.escape)
    case 125 where hasCandidates: return .engine(.nextCandidate)
    case 126 where hasCandidates: return .engine(.previousCandidate)
    default: return nil
    }
}

func shouldForwardBackspaceDirectly(keyCode: UInt16, hasComposition: Bool) -> Bool {
    keyCode == 51 && !hasComposition
}

func candidateSelectionIndex(keyCode: UInt16, candidateCount: Int, pageStart: Int) -> Int? {
    let selectionKeyCodes: [UInt16] = [18, 19, 20, 21, 23, 22, 26, 28, 25]
    guard
        let visibleIndex = selectionKeyCodes.firstIndex(of: keyCode),
        pageStart >= 0,
        pageStart + visibleIndex < candidateCount
    else {
        return nil
    }
    return pageStart + visibleIndex
}

func printableInputScalar(from event: NSEvent) -> Unicode.Scalar? {
    guard event.type == .keyDown,
          let characters = event.characters,
          characters.unicodeScalars.count == 1,
          let scalar = characters.unicodeScalars.first,
          (33 ... 126).contains(scalar.value)
    else {
        return nil
    }

    return scalar
}
