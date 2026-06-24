import Testing

@testable import KvasirViewerCore

@Test
func daemonLaunchAgentRegistersWhenViewerStartsAndAgentIsMissing() throws {
    let registry = RecordingLaunchAgentRegistry(status: .notRegistered)
    let registrationStore = RecordingLaunchAgentRegistrationStore()
    let launchAgent = DaemonLaunchAgent(
        registry: registry,
        fingerprintProvider: StaticLaunchAgentFingerprintProvider(fingerprint: "current-fingerprint"),
        registrationStore: registrationStore
    )

    let outcome = try launchAgent.ensureRegistered()

    #expect(outcome == .registered)
    #expect(registry.registeredPlistNames == [DaemonLaunchAgent.plistName])
    #expect(registrationStore.savedFingerprints == ["current-fingerprint"])
}

@Test
func daemonLaunchAgentDoesNotRegisterAgainWhenAlreadyEnabled() throws {
    let registry = RecordingLaunchAgentRegistry(status: .enabled)
    let launchAgent = DaemonLaunchAgent(registry: registry)

    let outcome = try launchAgent.ensureRegistered()

    #expect(outcome == .alreadyRegistered)
    #expect(registry.registeredPlistNames.isEmpty)
    #expect(registry.unregisteredPlistNames.isEmpty)
    #expect(registry.terminatedPlistNames.isEmpty)
}

@Test
func daemonLaunchAgentRefreshesRegistrationWhenPackagedHelperChanged() throws {
    let registry = RecordingLaunchAgentRegistry(status: .enabled)
    let registrationStore = RecordingLaunchAgentRegistrationStore(storedFingerprint: "old-fingerprint")
    let launchAgent = DaemonLaunchAgent(
        registry: registry,
        fingerprintProvider: StaticLaunchAgentFingerprintProvider(fingerprint: "new-fingerprint"),
        registrationStore: registrationStore
    )

    let outcome = try launchAgent.ensureRegistered()

    #expect(outcome == .registered)
    #expect(registry.terminatedPlistNames == [DaemonLaunchAgent.plistName])
    #expect(registry.unregisteredPlistNames == [DaemonLaunchAgent.plistName])
    #expect(registry.registeredPlistNames == [DaemonLaunchAgent.plistName])
    #expect(registrationStore.savedFingerprints == ["new-fingerprint"])
}

@Test
func daemonLaunchAgentCanForceRefreshRegistration() throws {
    let registry = RecordingLaunchAgentRegistry(status: .enabled)
    let registrationStore = RecordingLaunchAgentRegistrationStore(storedFingerprint: "current-fingerprint")
    let launchAgent = DaemonLaunchAgent(
        registry: registry,
        fingerprintProvider: StaticLaunchAgentFingerprintProvider(fingerprint: "current-fingerprint"),
        registrationStore: registrationStore
    )

    let outcome = try launchAgent.refreshRegistration()

    #expect(outcome == .registered)
    #expect(registry.terminatedPlistNames == [DaemonLaunchAgent.plistName])
    #expect(registry.unregisteredPlistNames == [DaemonLaunchAgent.plistName])
    #expect(registry.registeredPlistNames == [DaemonLaunchAgent.plistName])
    #expect(registrationStore.savedFingerprints == ["current-fingerprint"])
}

@Test
func daemonLaunchAgentSurfacesApprovalRequirementWithoutRetryingRegistration() throws {
    let registry = RecordingLaunchAgentRegistry(status: .requiresApproval)
    let launchAgent = DaemonLaunchAgent(registry: registry)

    let outcome = try launchAgent.ensureRegistered()

    #expect(outcome == .requiresApproval)
    #expect(registry.registeredPlistNames.isEmpty)
    #expect(registry.unregisteredPlistNames.isEmpty)
}

private final class RecordingLaunchAgentRegistry: LaunchAgentRegistry {
    private let status: LaunchAgentStatus
    private(set) var registeredPlistNames: [String] = []
    private(set) var unregisteredPlistNames: [String] = []
    private(set) var terminatedPlistNames: [String] = []

    init(status: LaunchAgentStatus) {
        self.status = status
    }

    func status(plistName: String) -> LaunchAgentStatus {
        status
    }

    func register(plistName: String) throws {
        registeredPlistNames.append(plistName)
    }

    func unregister(plistName: String) throws {
        unregisteredPlistNames.append(plistName)
    }

    func terminate(plistName: String) {
        terminatedPlistNames.append(plistName)
    }
}

private struct StaticLaunchAgentFingerprintProvider: LaunchAgentFingerprintProvider {
    let fingerprint: String?

    func fingerprint(plistName: String) -> String? {
        fingerprint
    }
}

private final class RecordingLaunchAgentRegistrationStore: LaunchAgentRegistrationStore {
    private let storedFingerprintValue: String?
    private(set) var savedFingerprints: [String] = []

    init(storedFingerprint: String? = nil) {
        storedFingerprintValue = storedFingerprint
    }

    func storedFingerprint(plistName: String) -> String? {
        storedFingerprintValue
    }

    func saveFingerprint(_ fingerprint: String, plistName: String) {
        savedFingerprints.append(fingerprint)
    }
}
