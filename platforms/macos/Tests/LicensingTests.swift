import Foundation

@main
enum LicensingTests {
    static func main() async throws {
        try configurationTests()
        try cachedAccessTests()
        try await validatorTests()
        try await controllerTests()
        print("Licensing tests passed.")
    }

    private static func configurationTests() throws {
        try expect(
            SlimeBillingConfiguration.resolve(info: [:], processEnvironment: [:]) == .development,
            "an unconfigured local build should be development-only"
        )
        let invalid = SlimeBillingConfiguration.resolve(
            info: ["SlimeBillingEnvironment": "production"],
            processEnvironment: [:]
        )
        guard case .invalid = invalid else {
            throw TestFailure("production must fail closed when Polar values are missing")
        }
        let configured = SlimeBillingConfiguration.resolve(
            info: [:],
            processEnvironment: [
                "SLIME_BILLING_ENVIRONMENT": "sandbox",
                "SLIME_POLAR_ORGANIZATION_ID": "organization",
                "SLIME_POLAR_BENEFIT_ID": "benefit",
                "SLIME_POLAR_CHECKOUT_URL": "https://sandbox.example/checkout",
            ]
        )
        guard case let .configured(value) = configured else {
            throw TestFailure("complete sandbox values should resolve")
        }
        try expect(
            value.validationEndpoint.host == "sandbox-api.polar.sh",
            "sandbox must use Polar's sandbox endpoint"
        )
        let packagedProduction = SlimeBillingConfiguration.resolve(
            info: [
                "SlimeBillingEnvironment": "production",
                "SlimePolarOrganizationID": "production-organization",
                "SlimePolarBenefitID": "production-benefit",
                "SlimePolarCheckoutURL": "https://example.com/production",
            ],
            processEnvironment: ["SLIME_BILLING_ENVIRONMENT": "development"]
        )
        guard case .configured = packagedProduction else {
            throw TestFailure("process variables must not bypass a packaged production build")
        }
        try expect(
            SlimeBillingTerms.monthlyPriceJPY == 200
                && SlimeBillingTerms.trialLengthInDays == 14,
            "the public billing terms must remain 200 JPY monthly with a 14-day trial"
        )
    }

    private static func cachedAccessTests() throws {
        let now = Date(timeIntervalSince1970: 2_000_000)
        try expect(
            SlimeCachedAccessPolicy.status(
                hasKey: false,
                lastValidatedAt: nil,
                now: now
            ) == .needsLicense,
            "a missing key should require activation"
        )
        let recent = now.addingTimeInterval(-2 * 86_400)
        guard case .offlineGrace = SlimeCachedAccessPolicy.status(
            hasKey: true,
            lastValidatedAt: recent,
            now: now
        ) else {
            throw TestFailure("a recently verified key should have an offline grace period")
        }
        let stale = now.addingTimeInterval(-8 * 86_400)
        try expect(
            SlimeCachedAccessPolicy.status(
                hasKey: true,
                lastValidatedAt: stale,
                now: now
            ) == .connectionRequired,
            "a stale subscription must reconnect before allowing input"
        )
    }

    private static func validatorTests() async throws {
        let endpoint = URL(string: "https://example.com/validate")!
        let configuration = SlimeBillingConfiguration(
            organizationID: "organization",
            benefitID: "benefit",
            validationEndpoint: endpoint,
            checkoutURL: URL(string: "https://example.com/checkout")!,
            keychainService: "tests",
            defaultsPrefix: "tests"
        )
        let sessionConfiguration = URLSessionConfiguration.ephemeral
        sessionConfiguration.protocolClasses = [URLProtocolStub.self]
        let validator = PolarLicenseValidator(
            configuration: configuration,
            session: URLSession(configuration: sessionConfiguration)
        )
        URLProtocolStub.handler = { request in
            let body = try require(request.bodyData, "validation body")
            let json = try require(
                JSONSerialization.jsonObject(with: body) as? [String: String],
                "validation JSON"
            )
            try expect(
                json == [
                    "key": "SLIME-KEY",
                    "organization_id": "organization",
                    "benefit_id": "benefit",
                ],
                "validation must be scoped to the Slime benefit"
            )
            return (
                HTTPURLResponse(url: endpoint, statusCode: 200, httpVersion: nil,
                                headerFields: nil)!,
                Data(#"{"id":"license-id"}"#.utf8)
            )
        }
        let valid = try await validator.validate(key: "SLIME-KEY")
        try expect(valid, "a Polar 200 should validate")

        URLProtocolStub.handler = { _ in
            (
                HTTPURLResponse(url: endpoint, statusCode: 404, httpVersion: nil,
                                headerFields: nil)!,
                Data()
            )
        }
        let unknownIsValid = try await validator.validate(key: "UNKNOWN")
        try expect(!unknownIsValid, "a Polar 404 must reject")

        URLProtocolStub.handler = { _ in
            (
                HTTPURLResponse(url: endpoint, statusCode: 200, httpVersion: nil,
                                headerFields: nil)!,
                Data(#"{"status":"revoked","benefit_id":"benefit"}"#.utf8)
            )
        }
        let revokedIsValid = try await validator.validate(key: "REVOKED")
        try expect(!revokedIsValid, "an explicitly revoked key must reject")

        URLProtocolStub.handler = { _ in
            (
                HTTPURLResponse(url: endpoint, statusCode: 200, httpVersion: nil,
                                headerFields: nil)!,
                Data(#"{"status":"granted","benefit_id":"another-product"}"#.utf8)
            )
        }
        let otherProductIsValid = try await validator.validate(key: "OTHER")
        try expect(!otherProductIsValid, "a key for another benefit must reject")
        URLProtocolStub.handler = nil
    }

    private static func controllerTests() async throws {
        let configuration = SlimeBillingConfiguration(
            organizationID: "organization",
            benefitID: "benefit",
            validationEndpoint: URL(string: "https://example.com/validate")!,
            checkoutURL: URL(string: "https://example.com/checkout")!,
            keychainService: "tests",
            defaultsPrefix: "controller-tests"
        )
        let suiteName = "slime-licensing-tests-\(UUID().uuidString)"
        let defaults = try require(UserDefaults(suiteName: suiteName), "test defaults")
        defer { defaults.removePersistentDomain(forName: suiteName) }
        let store = MemoryLicenseStore(key: "WORKING-KEY")
        let now = Date(timeIntervalSince1970: 3_000_000)
        defaults.set(now, forKey: "controller-tests.lastValidatedAt")
        let controller = await SlimeAccessController(
            resolution: .configured(configuration),
            validator: FixedValidator(result: false),
            keyStore: store,
            defaults: defaults,
            now: { now }
        )
        let allowedBefore = await controller.allowsInput
        try expect(allowedBefore, "a recently verified stored key should initially allow input")
        let activated = await controller.activate("TYPO")
        try expect(!activated, "an invalid replacement key must reject")
        let allowedAfter = await controller.allowsInput
        try expect(allowedAfter, "an invalid replacement must preserve existing cached access")
        let preservedKey = try store.load()
        try expect(preservedKey == "WORKING-KEY", "an invalid replacement must keep the stored key")

        let emptyStore = MemoryLicenseStore()
        let activationController = await SlimeAccessController(
            resolution: .configured(configuration),
            validator: FixedValidator(result: true),
            keyStore: emptyStore,
            defaults: defaults,
            now: { now }
        )
        let success = await activationController.activate(" NEW-KEY ")
        try expect(success, "a valid Polar key should activate")
        let storedKey = try emptyStore.load()
        try expect(storedKey == "NEW-KEY", "activation should trim and store the key")
        let activatedStatus = await activationController.status
        guard case .licensed = activatedStatus else {
            throw TestFailure("successful activation should publish licensed status")
        }

        let expiringSuiteName = "slime-expiration-tests-\(UUID().uuidString)"
        let expiringDefaults = try require(
            UserDefaults(suiteName: expiringSuiteName),
            "expiration test defaults"
        )
        defer { expiringDefaults.removePersistentDomain(forName: expiringSuiteName) }
        expiringDefaults.set(
            now.addingTimeInterval(-7 * 86_400 + 0.05),
            forKey: "controller-tests.lastValidatedAt"
        )
        let expiringController = await SlimeAccessController(
            resolution: .configured(configuration),
            validator: FixedValidator(result: true),
            keyStore: MemoryLicenseStore(key: "EXPIRING-KEY"),
            defaults: expiringDefaults,
            now: { now }
        )
        let allowedBeforeExpiration = await expiringController.allowsInput
        try expect(allowedBeforeExpiration, "offline access should remain valid until its deadline")
        try await Task.sleep(nanoseconds: 120_000_000)
        let statusAfterExpiration = await expiringController.status
        try expect(
            statusAfterExpiration == .connectionRequired,
            "offline access must expire while the IME process remains running"
        )
    }

    private static func expect(_ condition: @autoclosure () -> Bool, _ message: String) throws {
        guard condition() else { throw TestFailure(message) }
    }

    private static func require<T>(_ value: T?, _ name: String) throws -> T {
        guard let value else { throw TestFailure("missing \(name)") }
        return value
    }
}

private struct FixedValidator: SlimeLicenseValidating {
    let result: Bool
    func validate(key: String) async throws -> Bool { result }
}

private final class MemoryLicenseStore: SlimeLicenseKeyStoring, @unchecked Sendable {
    private let lock = NSLock()
    private var key: String?

    init(key: String? = nil) {
        self.key = key
    }

    func load() throws -> String? {
        lock.lock()
        defer { lock.unlock() }
        return key
    }

    func save(_ key: String) throws {
        lock.lock()
        defer { lock.unlock() }
        self.key = key
    }

    func remove() throws {
        lock.lock()
        defer { lock.unlock() }
        key = nil
    }
}

private struct TestFailure: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

private final class URLProtocolStub: URLProtocol {
    static var handler: ((URLRequest) throws -> (HTTPURLResponse, Data))?

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        do {
            guard let handler = Self.handler else { throw TestFailure("missing URL handler") }
            let (response, data) = try handler(request)
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: data)
            client?.urlProtocolDidFinishLoading(self)
        } catch {
            client?.urlProtocol(self, didFailWithError: error)
        }
    }

    override func stopLoading() {}
}

private extension URLRequest {
    var bodyData: Data? {
        if let httpBody { return httpBody }
        guard let stream = httpBodyStream else { return nil }
        stream.open()
        defer { stream.close() }
        var result = Data()
        let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: 4_096)
        defer { buffer.deallocate() }
        while stream.hasBytesAvailable {
            let count = stream.read(buffer, maxLength: 4_096)
            guard count >= 0 else { return nil }
            if count == 0 { break }
            result.append(buffer, count: count)
        }
        return result
    }
}
