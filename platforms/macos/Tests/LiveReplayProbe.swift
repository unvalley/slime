// Offline replay probe; compiled separately from the macOS application.
import Foundation
@main struct LiveReplayProbe {
    static func main() throws {
        let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        let inputs = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[2]))) as! [[String: Any]]
        let directLiveCommit = CommandLine.arguments.dropFirst(3).contains("--direct-live-enter")
        let finalLiveOnly = CommandLine.arguments.dropFirst(3).contains("--final-live-only")
        let anchor = try RustEngine(dataDirectory: root.appendingPathComponent("anchor"))
        defer { precondition(anchor.hasNeuralReranker) }
        var results: [[String: Any]] = []
        for (index, item) in inputs.enumerated() {
            let engine = try RustEngine(dataDirectory: root.appendingPathComponent(String(index)))
            precondition(engine.hasNeuralReranker)
            _ = try engine.setOptions(liveConversion: true, historyCompletion: false, historyLearning: false)
            try engine.setExternalLeftContext(item["context_text"] as! String)
            try engine.setLiveNeuralRerankingEnabled(true)
            var preedit = ""
            var statuses: [String: Int] = [:]
            var visibleChanges = 0
            let inputScalars = (item["input"] as! String).unicodeScalars
            let lastScalarOffset = inputScalars.count - 1
            for (offset, scalar) in inputScalars.enumerated() {
                let actions = try engine.process(.character(scalar))
                precondition(!actions.contains(where: {$0.type == "commit"}))
                preedit = actions.last(where: {$0.type == "update_preedit"})?.text ?? preedit
                if (!finalLiveOnly || offset == lastScalarOffset),
                   let task = engine.makeLiveNeuralTask(minimumSwitchMargin: 0.2, longReadingMinimumSwitchMargin: 0.3, numericBaseSwitchMargin: 0.1, longReadingLambda: 0.6) {
                    let previous = preedit
                    if task.run() {
                        let actions = try engine.apply(task)
                        precondition(!actions.contains(where: {$0.type == "commit"}))
                        preedit = actions.last(where: {$0.type == "update_preedit"})?.text ?? preedit
                    }
                    statuses[String(task.runStatus ?? 999), default: 0] += 1
                    if preedit != previous { visibleChanges += 1 }
                }
            }
            let live = preedit
            if !directLiveCommit {
                let explicit = try engine.process(.space)
                preedit = explicit.last(where: {$0.type == "update_preedit"})?.text ?? preedit
            }
            let committed = try engine.process(.enter)
            precondition(committed.filter {$0.type == "commit"}.map(\.text) == [preedit], "commit mismatch at \(item["index"]!)")
            var result: [String: Any] = ["index":item["index"]!, "live":live, "expected":item["expected_output"]!, "statuses":statuses, "visible_changes":visibleChanges]
            result["ranking_schedule"] = finalLiveOnly ? "after-input" : "after-each-scalar"
            if directLiveCommit {
                result["committed"] = preedit
                result["commit_mode"] = "direct-live-enter"
            } else {
                result["explicit"] = preedit
            }
            results.append(result)
            if (index + 1) % 25 == 0 || index + 1 == inputs.count {
                FileHandle.standardError.write(Data("replayed \(index + 1)/\(inputs.count)\n".utf8))
            }
        }
        print(String(decoding: try JSONSerialization.data(withJSONObject: results, options: [.prettyPrinted, .sortedKeys]), as: UTF8.self))
    }
}
