import AppKit

@main
enum SettingsPreview {
    static func main() {
        let application = NSApplication.shared
        application.setActivationPolicy(.regular)
        if ProcessInfo.processInfo.environment["SLIME_SETTINGS_APPEARANCE"] == "dark" {
            application.appearance = NSAppearance(named: .darkAqua)
        }
        if ProcessInfo.processInfo.environment["SLIME_SETTINGS_SAMPLE_PACKS"] == "1" {
            InstalledDictionaryPackBridge.catalogLoader = {
                InstalledDictionaryPackCatalog(
                    packs: [
                        InstalledDictionaryPack(
                            id: "sample-pro",
                            name: "専門用語 Plus",
                            version: "2026.07.1",
                            license: "Proprietary",
                            entryCount: 51
                        ),
                        InstalledDictionaryPack(
                            id: "proper-nouns-pro",
                            name: "固有名詞 Plus",
                            version: "2026.07.1",
                            license: "Proprietary",
                            entryCount: 33
                        ),
                    ],
                    errors: []
                )
            }
            InstalledDictionaryPackBridge.wordsLoader = { _ in
                [DomainDictionaryWord(reading: "よみ", surface: "表示語")]
            }
        }
        application.finishLaunching()
        let controller = SettingsWindowController.shared
        let tab = ProcessInfo.processInfo.environment["SLIME_SETTINGS_TAB"]
            .flatMap(SettingsTab.init(rawValue:)) ?? .general
        controller.present(initialTab: tab)
        if let height = ProcessInfo.processInfo.environment["SLIME_SETTINGS_HEIGHT"]
            .flatMap(Double.init)
        {
            controller.window?.setContentSize(NSSize(width: 680, height: height))
        }
        print("Settings preview visible: \(controller.window?.isVisible == true)")
        if let snapshotPath = ProcessInfo.processInfo.environment["SLIME_SETTINGS_SNAPSHOT_PATH"] {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                do {
                    try capture(window: controller.window, at: URL(fileURLWithPath: snapshotPath))
                    print("Settings preview snapshot: \(snapshotPath)")
                    application.terminate(nil)
                } catch {
                    fputs("Settings preview snapshot failed: \(error)\n", stderr)
                    application.terminate(nil)
                }
            }
        }
        application.run()
        withExtendedLifetime(controller) {}
    }

    private static func capture(window: NSWindow?, at url: URL) throws {
        guard let view = window?.contentView,
              let source = view.bitmapImageRepForCachingDisplay(in: view.bounds)
        else {
            throw CocoaError(.fileWriteUnknown)
        }
        view.cacheDisplay(in: view.bounds, to: source)
        guard let representation = NSBitmapImageRep(
            bitmapDataPlanes: nil,
            pixelsWide: source.pixelsWide,
            pixelsHigh: source.pixelsHigh,
            bitsPerSample: 8,
            samplesPerPixel: 4,
            hasAlpha: true,
            isPlanar: false,
            colorSpaceName: .deviceRGB,
            bytesPerRow: 0,
            bitsPerPixel: 0
        ), let context = NSGraphicsContext(bitmapImageRep: representation)
        else {
            throw CocoaError(.fileWriteUnknown)
        }

        NSGraphicsContext.saveGraphicsState()
        NSGraphicsContext.current = context
        view.effectiveAppearance.performAsCurrentDrawingAppearance {
            NSColor.windowBackgroundColor.setFill()
            NSRect(
                x: 0,
                y: 0,
                width: source.pixelsWide,
                height: source.pixelsHigh
            ).fill()
            source.draw(
                in: NSRect(
                    x: 0,
                    y: 0,
                    width: source.pixelsWide,
                    height: source.pixelsHigh
                ),
                from: .zero,
                operation: .sourceOver,
                fraction: 1,
                respectFlipped: false,
                hints: nil
            )
        }
        NSGraphicsContext.restoreGraphicsState()

        guard let data = representation.representation(using: .png, properties: [:]) else {
            throw CocoaError(.fileWriteUnknown)
        }
        try data.write(to: url, options: .atomic)
    }
}
