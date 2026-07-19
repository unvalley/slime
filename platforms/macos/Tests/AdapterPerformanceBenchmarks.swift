import Foundation

@main
enum AdapterPerformanceBenchmarks {
    static func main() throws {
        let samples = sampleCount()
        let engine = try RustEngine()

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
    }

    private static func measure(_ operation: () throws -> Void) rethrows -> UInt64 {
        let started = ContinuousClock.now
        try operation()
        let duration = started.duration(to: .now)
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
        ProcessInfo.processInfo.environment["IME_BENCH_SAMPLES"]
            .flatMap(Int.init) ?? 20_000
    }
}
