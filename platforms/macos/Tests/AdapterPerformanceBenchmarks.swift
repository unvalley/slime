import Foundation

@main
enum AdapterPerformanceBenchmarks {
    static func main() throws {
        let samples = sampleCount()
        let coldStart = ContinuousClock.now
        let engine = try RustEngine()
        let coldCreate = nanoseconds(from: coldStart.duration(to: .now))
        report("ffi/engine_cold_create", samples: [coldCreate])

        let creationSamples = min(samples, 1_000)
        var warmCreate = [UInt64]()
        warmCreate.reserveCapacity(creationSamples)
        for _ in 0..<creationSamples {
            warmCreate.append(try measure {
                _ = try RustEngine()
            })
        }
        report("ffi/engine_warm_create", samples: warmCreate)

        for _ in 0..<1_000 {
            _ = try engine.process(.backspace)
        }

        var idleBackspace = [UInt64]()
        idleBackspace.reserveCapacity(samples)
        for _ in 0..<samples {
            idleBackspace.append(try measure {
                _ = try engine.process(.backspace)
            })
        }

        var composingBackspace = [UInt64]()
        composingBackspace.reserveCapacity(samples)
        for _ in 0..<samples {
            _ = try engine.process(.character("k"))
            composingBackspace.append(try measure {
                _ = try engine.process(.backspace)
            })
        }

        report("ffi/idle_backspace", samples: idleBackspace)
        report("ffi/composing_backspace", samples: composingBackspace)

        let characterEngine = try RustEngine()
        _ = try characterEngine.setOptions(
            liveConversion: false,
            historyCompletion: false,
            historyLearning: false
        )
        var characterInput = [UInt64]()
        characterInput.reserveCapacity(samples)
        for _ in 0..<samples {
            characterInput.append(try measure {
                _ = try characterEngine.process(.character("a"))
            })
            _ = try characterEngine.process(.backspace)
        }
        report("ffi/character_no_live", samples: characterInput)

        let conversionSamples = min(samples, 5_000)
        let conversionEngine = try RustEngine()
        var spaceConversion = [UInt64]()
        spaceConversion.reserveCapacity(conversionSamples)
        for _ in 0..<conversionSamples {
            try type("nihon", into: conversionEngine)
            spaceConversion.append(try measure {
                _ = try conversionEngine.process(.space)
            })
            _ = try conversionEngine.process(.enter)
        }
        report("ffi/space_conversion", samples: spaceConversion)

        let liveSamples = min(samples, 500)
        let liveEngine = try RustEngine()
        _ = try liveEngine.setOptions(
            liveConversion: true,
            historyCompletion: false,
            historyLearning: false
        )
        let liveInput = Array(
            "seidowotakamerukufuuwoshiteikimashouseidowotakameru".unicodeScalars.prefix(50)
        )
        var liveCharacter50 = [UInt64]()
        liveCharacter50.reserveCapacity(liveSamples)
        for _ in 0..<liveSamples {
            for scalar in liveInput.dropLast() {
                _ = try liveEngine.process(.character(scalar))
            }
            liveCharacter50.append(try measure {
                _ = try liveEngine.process(.character(liveInput.last!))
            })
            _ = try liveEngine.process(.enter)
        }
        report("ffi/live_character_50", samples: liveCharacter50)

        let historyDirectory = FileManager.default.temporaryDirectory.appendingPathComponent(
            "slime-history-benchmark-\(ProcessInfo.processInfo.processIdentifier)-\(UUID())",
            isDirectory: true
        )
        try FileManager.default.createDirectory(
            at: historyDirectory,
            withIntermediateDirectories: true
        )
        defer { try? FileManager.default.removeItem(at: historyDirectory) }
        var history = "# slime-history-v1\n"
        for index in 0..<499 {
            history += "れきし\(index)\t履歴\(index)\t1\t\(index)\n"
        }
        history += "ぱふぉーまんす\tパフォーマンス\t8\t1000\n"
        try Data(history.utf8).write(
            to: historyDirectory.appendingPathComponent("history.tsv")
        )
        let historyEngine = try RustEngine(dataDirectory: historyDirectory)
        _ = try historyEngine.setOptions(
            liveConversion: false,
            historyCompletion: true,
            historyLearning: false
        )
        let historySamples = min(samples, 5_000)
        var historyCompletion = [UInt64]()
        historyCompletion.reserveCapacity(historySamples)
        for _ in 0..<historySamples {
            try type("paf", into: historyEngine)
            historyCompletion.append(try measure {
                _ = try historyEngine.process(.character("u"))
            })
            _ = try historyEngine.process(.enter)
        }
        report("ffi/history_completion_500", samples: historyCompletion)
    }

    private static func type(_ value: String, into engine: RustEngine) throws {
        for scalar in value.unicodeScalars {
            _ = try engine.process(.character(scalar))
        }
    }

    private static func measure(_ operation: () throws -> Void) rethrows -> UInt64 {
        let started = ContinuousClock.now
        try operation()
        return nanoseconds(from: started.duration(to: .now))
    }

    private static func nanoseconds(from duration: Duration) -> UInt64 {
        let components = duration.components
        return UInt64(components.seconds) * 1_000_000_000
            + UInt64(components.attoseconds / 1_000_000_000)
    }

    private static func report(_ name: String, samples: [UInt64]) {
        let sorted = samples.sorted()
        let p50 = percentile(0.50, from: sorted)
        let p95 = percentile(0.95, from: sorted)
        print("\(name)\tp50=\(p50)ns\tp95=\(p95)ns\tn=\(samples.count)")
    }

    private static func percentile(_ percentile: Double, from sorted: [UInt64]) -> UInt64 {
        let index = Int((Double(sorted.count - 1) * percentile).rounded(.up))
        return sorted[index]
    }

    private static func sampleCount() -> Int {
        ProcessInfo.processInfo.environment["SLIME_BENCH_SAMPLES"]
            .flatMap(Int.init) ?? 20_000
    }
}
