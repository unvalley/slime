import AppKit
import Combine
import Foundation
import Security

enum SlimeBillingTerms {
    static let monthlyPriceJPY = 200
    static let trialLengthInDays = 14
    static let offlineGracePeriodInDays = 7
}

struct SlimeBillingConfiguration: Equatable, Sendable {
    let organizationID: String
    let benefitID: String
    let validationEndpoint: URL
    let checkoutURL: URL
    let keychainService: String
    let defaultsPrefix: String

    enum Environment: String {
        case development
        case production
        case sandbox
    }

    enum Resolution: Equatable {
        case development
        case configured(SlimeBillingConfiguration)
        case invalid(String)
    }

    static func resolve(
        info: [String: Any] = Bundle.main.infoDictionary ?? [:],
        processEnvironment: [String: String] = ProcessInfo.processInfo.environment
    ) -> Resolution {
        let environmentValue = value(
            environmentName: "SLIME_BILLING_ENVIRONMENT",
            infoName: "SlimeBillingEnvironment",
            info: info,
            processEnvironment: processEnvironment
        )?.lowercased() ?? Environment.development.rawValue

        guard let environment = Environment(rawValue: environmentValue) else {
            return .invalid("課金環境「\(environmentValue)」は使用できません。")
        }
        guard environment != .development else { return .development }

        let names = [
            ("SLIME_POLAR_ORGANIZATION_ID", "SlimePolarOrganizationID"),
            ("SLIME_POLAR_BENEFIT_ID", "SlimePolarBenefitID"),
            ("SLIME_POLAR_CHECKOUT_URL", "SlimePolarCheckoutURL"),
        ]
        let values = Dictionary(uniqueKeysWithValues: names.compactMap { environmentName, infoName in
            value(
                environmentName: environmentName,
                infoName: infoName,
                info: info,
                processEnvironment: processEnvironment
            ).map { (environmentName, $0) }
        })
        let missing = names.map(\.0).filter { values[$0] == nil }
        guard missing.isEmpty else {
            return .invalid("Polarの設定が不足しています: \(missing.joined(separator: ", "))")
        }
        guard
            let organizationID = values["SLIME_POLAR_ORGANIZATION_ID"],
            let benefitID = values["SLIME_POLAR_BENEFIT_ID"],
            let checkoutValue = values["SLIME_POLAR_CHECKOUT_URL"],
            let checkoutURL = secureURL(checkoutValue)
        else {
            return .invalid("Polar Checkout URLはHTTPSで指定してください。")
        }

        let defaultEndpoint = environment == .sandbox
            ? "https://sandbox-api.polar.sh/v1/customer-portal/license-keys/validate"
            : "https://api.polar.sh/v1/customer-portal/license-keys/validate"
        let endpointValue = value(
            environmentName: "SLIME_POLAR_VALIDATION_ENDPOINT",
            infoName: "SlimePolarValidationEndpoint",
            info: info,
            processEnvironment: processEnvironment
        ) ?? defaultEndpoint
        guard let validationEndpoint = secureURL(endpointValue) else {
            return .invalid("Polar validation endpointはHTTPSで指定してください。")
        }

        let suffix = environment == .sandbox ? ".sandbox" : ""
        return .configured(SlimeBillingConfiguration(
            organizationID: organizationID,
            benefitID: benefitID,
            validationEndpoint: validationEndpoint,
            checkoutURL: checkoutURL,
            keychainService: "com.unvalley.inputmethod.Slime.license\(suffix)",
            defaultsPrefix: "billing\(suffix)"
        ))
    }

    private static func value(
        environmentName: String,
        infoName: String,
        info: [String: Any],
        processEnvironment: [String: String]
    ) -> String? {
        // A packaged build's signed Info.plist is authoritative. Process
        // values are only a fallback for previews, tests, and unpackaged tools.
        let raw = info[infoName] as? String ?? processEnvironment[environmentName]
        let trimmed = raw?.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed?.isEmpty == false ? trimmed : nil
    }

    private static func secureURL(_ value: String) -> URL? {
        guard let url = URL(string: value), url.scheme == "https", url.host != nil else {
            return nil
        }
        return url
    }
}

enum SlimeAccessStatus: Equatable, Sendable {
    case localDevelopment
    case needsLicense
    case validating(allowsInput: Bool)
    case licensed(lastValidatedAt: Date)
    case offlineGrace(until: Date)
    case connectionRequired
    case configurationError(String)

    var allowsInput: Bool {
        switch self {
        case .localDevelopment, .licensed, .offlineGrace:
            true
        case let .validating(allowsInput):
            allowsInput
        case .needsLicense, .connectionRequired, .configurationError:
            false
        }
    }
}

enum SlimeCachedAccessPolicy {
    static func status(
        hasKey: Bool,
        lastValidatedAt: Date?,
        now: Date,
        calendar: Calendar = .current,
        gracePeriodInDays: Int = SlimeBillingTerms.offlineGracePeriodInDays
    ) -> SlimeAccessStatus {
        guard hasKey, let lastValidatedAt else { return .needsLicense }
        let graceEnd = calendar.date(
            byAdding: .day,
            value: gracePeriodInDays,
            to: lastValidatedAt
        ) ?? lastValidatedAt.addingTimeInterval(Double(gracePeriodInDays) * 86_400)
        return now < graceEnd ? .offlineGrace(until: graceEnd) : .connectionRequired
    }
}

protocol SlimeLicenseValidating: Sendable {
    func validate(key: String) async throws -> Bool
}

struct PolarLicenseValidator: SlimeLicenseValidating {
    let configuration: SlimeBillingConfiguration
    let session: URLSession

    init(configuration: SlimeBillingConfiguration, session: URLSession = .shared) {
        self.configuration = configuration
        self.session = session
    }

    func validate(key: String) async throws -> Bool {
        var request = URLRequest(url: configuration.validationEndpoint)
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = try JSONEncoder().encode(ValidationRequest(
            key: key,
            organizationID: configuration.organizationID,
            benefitID: configuration.benefitID
        ))

        let (data, response) = try await session.data(for: request)
        guard let response = response as? HTTPURLResponse else {
            throw ValidationError.invalidResponse
        }
        if response.statusCode == 404 { return false }
        guard (200..<300).contains(response.statusCode) else {
            throw ValidationError.httpStatus(response.statusCode)
        }

        // Polar currently treats a successful response as validation. Older
        // responses also included status and benefit_id, so keep enforcing
        // them when present while sending benefit_id in the request itself.
        guard !data.isEmpty else { return true }
        let validation = try JSONDecoder().decode(ValidationResponse.self, from: data)
        if let status = validation.status, status != "granted" { return false }
        if let benefitID = validation.benefitID,
           benefitID != configuration.benefitID
        {
            return false
        }
        if let expiresAt = validation.expiresAt, expiresAt <= Date() { return false }
        return true
    }

    private struct ValidationRequest: Encodable {
        let key: String
        let organizationID: String
        let benefitID: String

        enum CodingKeys: String, CodingKey {
            case key
            case organizationID = "organization_id"
            case benefitID = "benefit_id"
        }
    }

    private struct ValidationResponse: Decodable {
        let status: String?
        let benefitID: String?
        let expiresAt: Date?

        enum CodingKeys: String, CodingKey {
            case status
            case benefitID = "benefit_id"
            case expiresAt = "expires_at"
        }

        init(from decoder: Decoder) throws {
            let container = try decoder.container(keyedBy: CodingKeys.self)
            status = try container.decodeIfPresent(String.self, forKey: .status)
            benefitID = try container.decodeIfPresent(String.self, forKey: .benefitID)
            if let raw = try container.decodeIfPresent(String.self, forKey: .expiresAt) {
                let formatter = ISO8601DateFormatter()
                formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
                expiresAt = formatter.date(from: raw)
                    ?? ISO8601DateFormatter().date(from: raw)
            } else {
                expiresAt = nil
            }
        }
    }

    enum ValidationError: LocalizedError {
        case invalidResponse
        case httpStatus(Int)

        var errorDescription: String? {
            switch self {
            case .invalidResponse:
                "Polarから正しい応答を受け取れませんでした。"
            case let .httpStatus(status):
                "Polarとの通信に失敗しました（HTTP \(status)）。"
            }
        }
    }
}

protocol SlimeLicenseKeyStoring: Sendable {
    func load() throws -> String?
    func save(_ key: String) throws
    func remove() throws
}

struct SlimeKeychainLicenseStore: SlimeLicenseKeyStoring {
    let service: String
    let account = "polar"

    func load() throws -> String? {
        var query = baseQuery
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess else { throw StoreError.status(status) }
        guard let data = result as? Data, let key = String(data: data, encoding: .utf8) else {
            throw StoreError.invalidData
        }
        return key
    }

    func save(_ key: String) throws {
        let attributes = [kSecValueData as String: Data(key.utf8)]
        let updateStatus = SecItemUpdate(baseQuery as CFDictionary, attributes as CFDictionary)
        if updateStatus == errSecSuccess { return }
        guard updateStatus == errSecItemNotFound else { throw StoreError.status(updateStatus) }
        var query = baseQuery
        query[kSecValueData as String] = Data(key.utf8)
        let addStatus = SecItemAdd(query as CFDictionary, nil)
        guard addStatus == errSecSuccess else { throw StoreError.status(addStatus) }
    }

    func remove() throws {
        let status = SecItemDelete(baseQuery as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else {
            throw StoreError.status(status)
        }
    }

    private var baseQuery: [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
    }

    enum StoreError: Error {
        case invalidData
        case status(OSStatus)
    }
}

@MainActor
final class SlimeAccessController: ObservableObject {
    static let shared = SlimeAccessController(resolution: SlimeBillingConfiguration.resolve())

    @Published private(set) var status: SlimeAccessStatus {
        didSet { scheduleOfflineExpirationIfNeeded() }
    }
    @Published private(set) var message: String?
    @Published private(set) var isValidating = false

    var checkoutURL: URL? { configuration?.checkoutURL }
    var allowsInput: Bool { status.allowsInput }

    private let configuration: SlimeBillingConfiguration?
    private let validator: (any SlimeLicenseValidating)?
    private let keyStore: (any SlimeLicenseKeyStoring)?
    private let defaults: UserDefaults
    private let now: @Sendable () -> Date
    private var validationGeneration: UInt64 = 0
    private var offlineExpirationTask: Task<Void, Never>?

    init(
        resolution: SlimeBillingConfiguration.Resolution,
        validator: (any SlimeLicenseValidating)? = nil,
        keyStore: (any SlimeLicenseKeyStoring)? = nil,
        defaults: UserDefaults = .standard,
        now: @escaping @Sendable () -> Date = { Date() }
    ) {
        self.defaults = defaults
        self.now = now
        switch resolution {
        case .development:
            configuration = nil
            self.validator = nil
            self.keyStore = nil
            status = .localDevelopment
        case let .invalid(message):
            configuration = nil
            self.validator = nil
            self.keyStore = nil
            status = .configurationError(message)
        case let .configured(configuration):
            self.configuration = configuration
            self.validator = validator ?? PolarLicenseValidator(configuration: configuration)
            self.keyStore = keyStore ?? SlimeKeychainLicenseStore(
                service: configuration.keychainService
            )
            let hasKey = (try? self.keyStore?.load()) != nil
            status = SlimeCachedAccessPolicy.status(
                hasKey: hasKey,
                lastValidatedAt: defaults.object(
                    forKey: "\(configuration.defaultsPrefix).lastValidatedAt"
                ) as? Date,
                now: now()
            )
        }
        scheduleOfflineExpirationIfNeeded()
    }

    func refreshStoredLicense() async {
        guard let configuration, let validator, let keyStore else { return }
        guard !isValidating else { return }
        let key: String?
        do {
            key = try keyStore.load()
        } catch {
            message = "保存済みライセンスキーを読み込めませんでした。"
            return
        }
        guard let key else {
            status = .needsLicense
            return
        }
        _ = await validate(key: key, shouldSave: false, configuration: configuration,
                           validator: validator, keyStore: keyStore)
    }

    @discardableResult
    func activate(_ enteredKey: String) async -> Bool {
        guard let configuration, let validator, let keyStore else { return false }
        let key = enteredKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty else {
            message = "ライセンスキーを入力してください。"
            return false
        }
        return await validate(key: key, shouldSave: true, configuration: configuration,
                              validator: validator, keyStore: keyStore)
    }

    func removeLicense() {
        guard let configuration, let keyStore else { return }
        try? keyStore.remove()
        defaults.removeObject(forKey: "\(configuration.defaultsPrefix).lastValidatedAt")
        status = .needsLicense
        message = nil
    }

    func openCheckout() {
        guard let checkoutURL else { return }
        NSWorkspace.shared.open(checkoutURL)
    }

    private func validate(
        key: String,
        shouldSave: Bool,
        configuration: SlimeBillingConfiguration,
        validator: any SlimeLicenseValidating,
        keyStore: any SlimeLicenseKeyStoring
    ) async -> Bool {
        validationGeneration &+= 1
        let generation = validationGeneration
        let previousStatus = status
        let previouslyAllowed = status.allowsInput
        status = .validating(allowsInput: previouslyAllowed)
        isValidating = true
        message = nil
        defer {
            if generation == validationGeneration { isValidating = false }
        }

        do {
            let isValid = try await validator.validate(key: key)
            guard generation == validationGeneration else { return false }
            guard isValid else {
                if !shouldSave {
                    try? keyStore.remove()
                    defaults.removeObject(forKey: "\(configuration.defaultsPrefix).lastValidatedAt")
                    status = .needsLicense
                } else {
                    // A typo while replacing a working key must not revoke the
                    // already verified key on this Mac.
                    status = previouslyAllowed ? previousStatus : .needsLicense
                }
                message = "このライセンスキーは有効ではありません。"
                return false
            }
            if shouldSave { try keyStore.save(key) }
            let validatedAt = now()
            defaults.set(validatedAt, forKey: "\(configuration.defaultsPrefix).lastValidatedAt")
            status = .licensed(lastValidatedAt: validatedAt)
            message = shouldSave ? "ライセンスを有効にしました。" : nil
            return true
        } catch {
            guard generation == validationGeneration else { return false }
            let hasKey = (try? keyStore.load()) != nil
            status = SlimeCachedAccessPolicy.status(
                hasKey: hasKey,
                lastValidatedAt: defaults.object(
                    forKey: "\(configuration.defaultsPrefix).lastValidatedAt"
                ) as? Date,
                now: now()
            )
            message = error.localizedDescription
            return false
        }
    }

    private func scheduleOfflineExpirationIfNeeded() {
        offlineExpirationTask?.cancel()
        guard case let .offlineGrace(until) = status else { return }
        let delay = max(0, until.timeIntervalSince(now()))
        offlineExpirationTask = Task { [weak self] in
            do {
                try await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
            } catch {
                return
            }
            guard !Task.isCancelled else { return }
            self?.status = .connectionRequired
        }
    }
}
