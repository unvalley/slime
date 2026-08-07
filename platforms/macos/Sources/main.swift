import AppKit
import Carbon
import InputMethodKit

var sharedServer: IMKServer!

final class NSManualApplication: NSApplication {
    private let appDelegate = AppDelegate()

    override init() {
        super.init()
        delegate = appDelegate
    }

    required init?(coder: NSCoder) {
        fatalError("init(coder:) has not been implemented")
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        DomainDictionaryCatalog.loader = RustEngine.domainDictionaryWords(mask:)
        InstalledDictionaryPackBridge.catalogLoader = {
            try RustEngine().installedDictionaryPacks()
        }
        InstalledDictionaryPackBridge.wordsLoader = { id in
            try RustEngine().installedDictionaryPackWords(id: id)
        }
        let bundle = Bundle.main
        guard let connectionName = bundle.object(
            forInfoDictionaryKey: "InputMethodConnectionName"
        ) as? String,
            let bundleIdentifier = bundle.bundleIdentifier,
            let server = IMKServer(name: connectionName, bundleIdentifier: bundleIdentifier)
        else {
            fatalError("Input method bundle configuration is invalid")
        }

        sharedServer = server
        SettingsStatusItem.shared.install()
        Task {
            await SlimeAccessController.shared.refreshStoredLicense()
        }
        let registrationStatus = TISRegisterInputSource(bundle.bundleURL as CFURL)
        if registrationStatus != noErr {
            let message = "TISRegisterInputSource failed: \(registrationStatus)\n"
            FileHandle.standardError.write(Data(message.utf8))
        }
    }
}

let application = NSManualApplication.shared
application.run()
