import AppKit

@main
enum AdapterTests {
    static func main() throws {
        let engine = try RustEngine()
        var latestPreedit: String?

        for scalar in "nihon".unicodeScalars {
            let actions = try engine.process(.character(scalar))
            latestPreedit = actions.last(where: { $0.type == "update_preedit" })?.text
        }
        try expect(latestPreedit == "にほn", "ambiguous trailing n should remain literal")

        let conversion = try engine.process(.space)
        let candidateAction = conversion.first(where: { $0.type == "show_candidates" })
        try expect(candidateAction?.candidates?.contains("日本") == true, "日本 should be a candidate")

        let commit = try engine.process(.enter)
        try expect(
            commit.contains(where: { $0.type == "commit" && $0.text == "日本" }),
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

        print("macOS Swift adapter tests passed")
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
