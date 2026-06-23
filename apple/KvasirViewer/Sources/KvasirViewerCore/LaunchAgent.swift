import Foundation
import ServiceManagement

public enum LaunchAgentStatus: Equatable, Sendable {
    case enabled
    case notRegistered
    case requiresApproval
    case notFound
    case unknown
}

public enum LaunchAgentRegistrationOutcome: Equatable, Sendable {
    case alreadyRegistered
    case registered
    case requiresApproval
}

public protocol LaunchAgentRegistry {
    func status(plistName: String) -> LaunchAgentStatus
    func register(plistName: String) throws
    func unregister(plistName: String) throws
}

public protocol LaunchAgentFingerprintProvider {
    func fingerprint(plistName: String) -> String?
}

public protocol LaunchAgentRegistrationStore {
    func storedFingerprint(plistName: String) -> String?
    func saveFingerprint(_ fingerprint: String, plistName: String)
}

public struct DaemonLaunchAgent {
    public static let plistName = "dev.kvasir.kvasird.plist"

    private let registry: LaunchAgentRegistry
    private let fingerprintProvider: any LaunchAgentFingerprintProvider
    private let registrationStore: any LaunchAgentRegistrationStore

    public init(
        registry: LaunchAgentRegistry = ServiceManagementLaunchAgentRegistry(),
        fingerprintProvider: any LaunchAgentFingerprintProvider = BundleLaunchAgentFingerprintProvider(),
        registrationStore: any LaunchAgentRegistrationStore = UserDefaultsLaunchAgentRegistrationStore()
    ) {
        self.registry = registry
        self.fingerprintProvider = fingerprintProvider
        self.registrationStore = registrationStore
    }

    public func ensureRegistered() throws -> LaunchAgentRegistrationOutcome {
        switch registry.status(plistName: Self.plistName) {
        case .enabled:
            if registrationNeedsRefresh(plistName: Self.plistName) {
                try registry.unregister(plistName: Self.plistName)
                try registry.register(plistName: Self.plistName)
                saveCurrentFingerprint(plistName: Self.plistName)
                return .registered
            }
            return .alreadyRegistered
        case .requiresApproval:
            return .requiresApproval
        case .notRegistered, .notFound, .unknown:
            try registry.register(plistName: Self.plistName)
            saveCurrentFingerprint(plistName: Self.plistName)
            return .registered
        }
    }

    private func registrationNeedsRefresh(plistName: String) -> Bool {
        guard let fingerprint = fingerprintProvider.fingerprint(plistName: plistName) else {
            return false
        }
        return registrationStore.storedFingerprint(plistName: plistName) != fingerprint
    }

    private func saveCurrentFingerprint(plistName: String) {
        guard let fingerprint = fingerprintProvider.fingerprint(plistName: plistName) else {
            return
        }
        registrationStore.saveFingerprint(fingerprint, plistName: plistName)
    }
}

public struct ServiceManagementLaunchAgentRegistry: LaunchAgentRegistry {
    public init() {}

    public func status(plistName: String) -> LaunchAgentStatus {
        switch SMAppService.agent(plistName: plistName).status {
        case .enabled:
            return .enabled
        case .notRegistered:
            return .notRegistered
        case .requiresApproval:
            return .requiresApproval
        case .notFound:
            return .notFound
        @unknown default:
            return .unknown
        }
    }

    public func register(plistName: String) throws {
        try SMAppService.agent(plistName: plistName).register()
    }

    public func unregister(plistName: String) throws {
        try SMAppService.agent(plistName: plistName).unregister()
    }
}

public struct BundleLaunchAgentFingerprintProvider: LaunchAgentFingerprintProvider {
    public init() {}

    public func fingerprint(plistName: String) -> String? {
        Bundle.main.object(forInfoDictionaryKey: "KvasirLaunchAgentFingerprint") as? String
    }
}

public struct UserDefaultsLaunchAgentRegistrationStore: LaunchAgentRegistrationStore {
    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    public func storedFingerprint(plistName: String) -> String? {
        defaults.string(forKey: key(plistName: plistName))
    }

    public func saveFingerprint(_ fingerprint: String, plistName: String) {
        defaults.set(fingerprint, forKey: key(plistName: plistName))
    }

    private func key(plistName: String) -> String {
        "dev.kvasir.launch-agent-fingerprint.\(plistName)"
    }
}
