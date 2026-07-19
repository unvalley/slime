import AppKit

func shouldForwardBackspaceDirectly(keyCode: UInt16, hasComposition: Bool) -> Bool {
    keyCode == 51 && !hasComposition
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

    return Unicode.Scalar(String(scalar).lowercased()) ?? scalar
}
