import Foundation
import Testing
import KvasirViewerCore

@testable import KvasirViewer

#if canImport(kvasir_client)
import kvasir_client
#endif

@MainActor
@Test
func productionViewerTargetBuildsOverviewScreenAndFactoryModel() async throws {
    let model = ProductionModelFactory.make()
    _ = OverviewScreen(model: model)

    #if !canImport(kvasir_client)
    do {
        try await model.refreshOverview()
        Issue.record("expected missing kvasir-client error from package-test build")
    } catch {
        #expect(error.localizedDescription.contains("kvasir-client"))
    }
    #endif
}

@Test
func harnessTelemetrySetupConfigUsesProductionDefaultsWhenDaemonOverridesAreEmpty() {
    let home = FileManager.default.homeDirectoryForCurrentUser
    let applicationSupport = FileManager.default.urls(
        for: .applicationSupportDirectory,
        in: .userDomainMask
    ).first ?? home

    let config = ProductionModelFactory.resolvedHarnessTelemetrySetupConfig(environment: [
        "KVASIR_OTLP_BIND": "",
        "KVASIR_DATA_DIR": "",
        "KVASIR_SETUP_SETTINGS": "",
    ])

    #expect(
        config.codexConfigPath == home.appendingPathComponent(".codex", isDirectory: true)
            .appendingPathComponent("config.toml").path
    )
    #expect(
        config.claudeSettingsPath == home.appendingPathComponent(".claude", isDirectory: true)
            .appendingPathComponent("settings.json").path
    )
    #expect(
        config.rawBodyDirectory == applicationSupport
            .appendingPathComponent("dev.kvasir", isDirectory: true)
            .appendingPathComponent("raw-bodies", isDirectory: true)
            .path
    )
    #expect(config.otlpEndpoint == "http://127.0.0.1:4318")
}

@Test
func harnessTelemetrySetupConfigHonorsDaemonEnvironmentOverrides() {
    let config = ProductionModelFactory.resolvedHarnessTelemetrySetupConfig(environment: [
        "KVASIR_OTLP_BIND": "127.0.0.1:54318",
        "KVASIR_DATA_DIR": "/tmp/kvasir-data",
        "KVASIR_SETUP_SETTINGS": "/tmp/kvasir-settings/settings.json",
    ])

    #expect(config.claudeSettingsPath == "/tmp/kvasir-settings/settings.json")
    #expect(config.rawBodyDirectory == "/tmp/kvasir-data/raw-bodies")
    #expect(config.otlpEndpoint == "http://127.0.0.1:54318")
}

@Test
func daemonFallbackOverviewClientStartsDaemonAndRetriesRecoverableFailure() async throws {
    let expected = OverviewRollups(
        tokenRollups: [
            .init(day: .init(year: 2026, month: 6, day: 24), inputTokens: 1, outputTokens: 2, cacheTokens: 3)
        ],
        costRollups: [],
        toolCallRollups: []
    )
    let primary = SequenceOverviewClient(results: [
        .failure(DaemonFallbackTestError.recoverable),
        .success(expected),
    ])
    let starter = RecordingDaemonProcessStarter()
    let client = DaemonFallbackOverviewClient(
        primary: primary,
        starter: starter,
        shouldStartDaemonAfterError: { error in
            error as? DaemonFallbackTestError == .recoverable
        }
    )

    let rollups = try await client.loadOverviewRollups(
        query: .init(start: Date(timeIntervalSince1970: 0), end: Date(timeIntervalSince1970: 1))
    )

    #expect(rollups == expected)
    #expect(await primary.loadCount == 2)
    #expect(starter.startCount == 1)
}

@Test
func daemonFallbackOverviewClientRetriesRecoverableFailuresWhileSpawnedDaemonBecomesReady() async throws {
    let expected = OverviewRollups(
        tokenRollups: [
            .init(day: .init(year: 2026, month: 6, day: 24), inputTokens: 5, outputTokens: 8, cacheTokens: 13)
        ],
        costRollups: [],
        toolCallRollups: []
    )
    let primary = SequenceOverviewClient(results: [
        .failure(DaemonFallbackTestError.recoverable),
        .failure(DaemonFallbackTestError.recoverable),
        .success(expected),
    ])
    let starter = RecordingDaemonProcessStarter()
    let retryDelay = RecordingRetryDelay()
    let client = DaemonFallbackOverviewClient(
        primary: primary,
        starter: starter,
        shouldStartDaemonAfterError: { error in
            error as? DaemonFallbackTestError == .recoverable
        },
        maximumRetryCount: 3,
        retryDelay: retryDelay.sleep
    )

    let rollups = try await client.loadOverviewRollups(
        query: .init(start: Date(timeIntervalSince1970: 0), end: Date(timeIntervalSince1970: 1))
    )

    #expect(rollups == expected)
    #expect(await primary.loadCount == 3)
    #expect(starter.startCount == 1)
    #expect(await retryDelay.attempts == [1])
}

@Test
func daemonFallbackOverviewClientDoesNotStartDaemonForNonrecoverableFailure() async throws {
    let primary = SequenceOverviewClient(results: [
        .failure(DaemonFallbackTestError.nonrecoverable),
    ])
    let starter = RecordingDaemonProcessStarter()
    let client = DaemonFallbackOverviewClient(
        primary: primary,
        starter: starter,
        shouldStartDaemonAfterError: { error in
            error as? DaemonFallbackTestError == .recoverable
        }
    )

    do {
        _ = try await client.loadOverviewRollups(
            query: .init(start: Date(timeIntervalSince1970: 0), end: Date(timeIntervalSince1970: 1))
        )
        Issue.record("expected nonrecoverable error")
    } catch {
        #expect(error as? DaemonFallbackTestError == .nonrecoverable)
    }

    #expect(await primary.loadCount == 1)
    #expect(starter.startCount == 0)
}

@MainActor
@Test
func productionFactoryWiresDaemonFallbackAfterStartupGateOpens() async throws {
    let primary = SequenceOverviewClient(results: [
        .failure(DaemonFallbackTestError.recoverable),
        .success(
            OverviewRollups(
                tokenRollups: [
                    .init(day: .init(year: 2026, month: 6, day: 24), inputTokens: 7, outputTokens: 11, cacheTokens: 13)
                ],
                costRollups: [],
                toolCallRollups: []
            )
        ),
    ])
    let starter = RecordingDaemonProcessStarter()
    let gate = DaemonFallbackGate(enabled: true)
    let model = ProductionModelFactory.make(
        overviewClient: primary,
        daemonStarter: starter,
        daemonFallbackGate: gate,
        shouldStartBundledDaemonAfterOverviewError: { error in
            error as? DaemonFallbackTestError == .recoverable
        }
    )

    try await model.refreshOverview()

    #expect(await primary.loadCount == 2)
    #expect(starter.startCount == 1)
    #expect(model.overviewSnapshot?.totals.totalTokens == 31)
}

@MainActor
@Test
func productionFactoryDoesNotStartBundledDaemonBeforeStartupGateOpens() async throws {
    let primary = SequenceOverviewClient(results: [
        .failure(DaemonFallbackTestError.recoverable),
    ])
    let starter = RecordingDaemonProcessStarter()
    let model = ProductionModelFactory.make(
        overviewClient: primary,
        daemonStarter: starter,
        daemonFallbackGate: DaemonFallbackGate(enabled: false),
        shouldStartBundledDaemonAfterOverviewError: { error in
            error as? DaemonFallbackTestError == .recoverable
        }
    )

    do {
        try await model.refreshOverview()
        Issue.record("expected startup-gated overview failure")
    } catch {
        #expect(error as? DaemonFallbackTestError == .recoverable)
    }

    #expect(await primary.loadCount == 1)
    #expect(starter.startCount == 0)
}

#if canImport(kvasir_client)
@MainActor
@Test
func productionFactoryStartsBundledDaemonForSocketIoAfterStartupGateOpens() async throws {
    let primary = SequenceOverviewClient(results: [
        .failure(KvasirClientError.SocketIo),
        .success(
            OverviewRollups(
                tokenRollups: [
                    .init(day: .init(year: 2026, month: 6, day: 24), inputTokens: 3, outputTokens: 5, cacheTokens: 8)
                ],
                costRollups: [],
                toolCallRollups: []
            )
        ),
    ])
    let starter = RecordingDaemonProcessStarter()
    let model = ProductionModelFactory.make(
        overviewClient: primary,
        daemonStarter: starter,
        daemonFallbackGate: DaemonFallbackGate(enabled: true)
    )

    try await model.refreshOverview()

    #expect(await primary.loadCount == 2)
    #expect(starter.startCount == 1)
    #expect(model.overviewSnapshot?.totals.totalTokens == 16)
}

@MainActor
@Test
func productionFactoryDoesNotStartBundledDaemonForNonSocketClientError() async throws {
    let primary = SequenceOverviewClient(results: [
        .failure(KvasirClientError.RpcSerialization),
    ])
    let starter = RecordingDaemonProcessStarter()
    let model = ProductionModelFactory.make(
        overviewClient: primary,
        daemonStarter: starter,
        daemonFallbackGate: DaemonFallbackGate(enabled: true)
    )

    do {
        try await model.refreshOverview()
        Issue.record("expected non-socket client error")
    } catch {
        #expect(error as? KvasirClientError == .RpcSerialization)
    }

    #expect(await primary.loadCount == 1)
    #expect(starter.startCount == 0)
}
#endif

@Test
func bundledDaemonEnvironmentInjectsHomeWhenMissingOrEmpty() {
    let homeDirectory = URL(fileURLWithPath: "/Users/tester", isDirectory: true)

    #expect(BundledDaemonProcess.daemonEnvironment(
        processEnvironment: ["PATH": "/usr/bin"],
        homeDirectory: homeDirectory
    )["HOME"] == "/Users/tester")
    #expect(BundledDaemonProcess.daemonEnvironment(
        processEnvironment: ["HOME": "", "PATH": "/usr/bin"],
        homeDirectory: homeDirectory
    )["HOME"] == "/Users/tester")
}

@Test
func bundledDaemonEnvironmentPreservesExistingHome() {
    let environment = BundledDaemonProcess.daemonEnvironment(
        processEnvironment: ["HOME": "/custom/home"],
        homeDirectory: URL(fileURLWithPath: "/Users/tester", isDirectory: true)
    )

    #expect(environment["HOME"] == "/custom/home")
}

private enum DaemonFallbackTestError: Error, Equatable {
    case recoverable
    case nonrecoverable
}

private actor SequenceOverviewClient: OverviewClient {
    private var results: [Result<OverviewRollups, any Error>]
    private(set) var loadCount = 0

    init(results: [Result<OverviewRollups, any Error>]) {
        self.results = results
    }

    func loadOverviewRollups(query: OverviewQuery) async throws -> OverviewRollups {
        loadCount += 1
        guard !results.isEmpty else {
            throw DaemonFallbackTestError.nonrecoverable
        }
        return try results.removeFirst().get()
    }
}

private final class RecordingDaemonProcessStarter: DaemonProcessStarter, @unchecked Sendable {
    private let lock = NSLock()
    private var starts = 0

    var startCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return starts
    }

    func startDaemon() throws {
        lock.lock()
        defer { lock.unlock() }
        starts += 1
    }
}

private actor RecordingRetryDelay {
    private(set) var attempts: [Int] = []

    func sleep(attempt: Int) async {
        attempts.append(attempt)
    }
}
