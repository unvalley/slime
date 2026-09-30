import Foundation

private let shortLiveNeuralReadingCharacterLimit = 8
private let shortLiveNeuralDebounceLimit: TimeInterval = 0.120

func liveNeuralDebounceDelay(
    configured: TimeInterval,
    readingCharacterCount: Int
) -> TimeInterval {
    if readingCharacterCount <= shortLiveNeuralReadingCharacterLimit {
        return min(configured, shortLiveNeuralDebounceLimit)
    }
    return configured
}
