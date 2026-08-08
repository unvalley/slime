import Foundation

func runInputContextTests(in directory: URL) throws {
    let clientA = NSObject()
    let clientB = NSObject()
    let caret = NSRange(location: 12, length: 0)
    var boundary = InputContextBoundary()
    try contextExpect(
        !boundary.shouldReset(client: clientA, selectedRange: caret),
        "the first client observation should establish a baseline"
    )
    boundary.observe(client: clientA, selectedRange: caret)
    try contextExpect(
        !boundary.shouldReset(client: clientA, selectedRange: caret),
        "the same client and caret should preserve context"
    )
    try contextExpect(
        boundary.shouldReset(
            client: clientA,
            selectedRange: NSRange(location: 11, length: 0)
        ),
        "an external caret move should reset context"
    )
    try contextExpect(
        boundary.shouldReset(client: clientB, selectedRange: caret),
        "changing the input client should reset context"
    )

    var fetchedRange: NSRange?
    let source = String(repeating: "前", count: 150) + "😀末尾"
    let bounded = precedingDocumentContext(
        selectedRange: NSRange(location: 300, length: 0)
    ) { range in
        fetchedRange = range
        return source
    }
    try contextExpect(
        fetchedRange?.length == 256 && bounded == String(source.suffix(128)),
        "document context should be bounded by both UTF-16 request and Character count"
    )
    var fetchedUnavailable = false
    let unavailable = precedingDocumentContext(
        selectedRange: NSRange(location: NSNotFound, length: 0)
    ) { _ in
        fetchedUnavailable = true
        return "should not be read"
    }
    try contextExpect(
        unavailable == nil && !fetchedUnavailable,
        "an unavailable caret must not read document text"
    )

    let packDirectory = directory.appendingPathComponent(
        "dictionary-packs",
        isDirectory: true
    )
    try FileManager.default.createDirectory(at: packDirectory, withIntermediateDirectories: true)
    let pack = """
        # slime-dictionary-pack-v3
        # id: sample-context-only
        # name: 文脈のみのサンプル
        # version: 2026.08.1
        # license: Example-Test-Only
        # minimum-slime-version: 0.1.0
        # published-at: 2026-08-08
        # provenance: fixture/generated/sample-context-only
        # payload-sha256: b89af4e2ec6e73c3dc508dbdcb37e6e1cf7ff94ea248e80bac5136774af02145
        # entries
        # context-rules
        文章\tかんじ\t漢字\t0
        """ + "\n"
    try Data(pack.utf8).write(
        to: packDirectory.appendingPathComponent("sample-context-only.slime-dict")
    )

    let baseline = try RustEngine(dataDirectory: directory)
    _ = try baseline.setOptions(liveConversion: false, historyCompletion: false)
    let baselineFirst = try firstCandidate(reading: "kanji", using: baseline)

    let contextual = try RustEngine(dataDirectory: directory)
    _ = try contextual.setOptions(liveConversion: false, historyCompletion: false)
    try contextual.setExternalLeftContext("これは既存の文章")
    let contextualFirst = try firstCandidate(reading: "kanji", using: contextual)
    try contextExpect(
        baselineFirst != "漢字" && contextualFirst == "漢字",
        "external document context should change ranking only through the matching context rule"
    )

    let privateEngine = try RustEngine(dataDirectory: directory)
    _ = try privateEngine.setOptions(
        liveConversion: false,
        historyCompletion: false,
        privateMode: true
    )
    try privateEngine.setExternalLeftContext("これは既存の文章")
    try contextExpect(
        try firstCandidate(reading: "kanji", using: privateEngine) != "漢字",
        "private mode must discard external document context"
    )
}

private func firstCandidate(reading: String, using engine: RustEngine) throws -> String? {
    for scalar in reading.unicodeScalars {
        _ = try engine.process(.character(scalar))
    }
    return try engine.process(.space)
        .first(where: { $0.type == "show_candidates" })?
        .candidates?
        .first
}

private func contextExpect(
    _ condition: @autoclosure () throws -> Bool,
    _ message: String
) throws {
    guard try condition() else {
        throw InputContextTestFailure(message: message)
    }
}

private struct InputContextTestFailure: Error, CustomStringConvertible {
    let message: String
    var description: String { message }
}
