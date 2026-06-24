import Foundation
import Testing

@testable import KvasirViewerCore

@MainActor
@Test
func viewerStartupRegistersDaemonAndLoadsOverviewForDefaultRange() async throws {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = TimeZone(secondsFromGMT: 0)!
    let now = Date(timeIntervalSince1970: 1_782_259_200)
    let client = RecordingStartupOverviewClient(
        rollups: OverviewRollups(
            tokenRollups: [
                .init(day: .init(year: 2026, month: 6, day: 21), inputTokens: 20, outputTokens: 10, cacheTokens: 5)
            ],
            costRollups: [
                .init(day: .init(year: 2026, month: 6, day: 21), costUsdNanos: 42)
            ],
            toolCallRollups: [
                .init(day: .init(year: 2026, month: 6, day: 21), callCount: 3)
            ]
        )
    )
    let startupEvents = StartupEventRecorder()
    let telemetrySetup = RecordingHarnessTelemetrySetup(events: startupEvents)
    let registry = RecordingStartupLaunchAgentRegistry(status: .notRegistered, events: startupEvents)
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        telemetrySetup: telemetrySetup,
        launchAgent: DaemonLaunchAgent(registry: registry),
        now: { now },
        calendar: calendar
    )

    try await model.start()

    #expect(model.launchAgentOutcome == .registered)
    #expect(startupEvents.events == [.configuredTelemetry, .registeredLaunchAgent])
    #expect(registry.registeredPlistNames == [DaemonLaunchAgent.plistName])
    #expect(client.queries == [
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: calendar).query
    ])
    #expect(model.overviewSnapshot?.totals == .init(totalTokens: 35, costUsdNanos: 42, toolCalls: 3))
}

@MainActor
@Test
func viewerStartupStopsBeforeLaunchAgentRegistrationWhenTelemetrySetupFails() async throws {
    let client = RecordingStartupOverviewClient(
        rollups: OverviewRollups(tokenRollups: [], costRollups: [], toolCallRollups: [])
    )
    let startupEvents = StartupEventRecorder()
    let registry = RecordingStartupLaunchAgentRegistry(status: .notRegistered, events: startupEvents)
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        telemetrySetup: RecordingHarnessTelemetrySetup(
            events: startupEvents,
            error: StartupTestError.transient
        ),
        launchAgent: DaemonLaunchAgent(registry: registry)
    )

    do {
        try await model.start()
        Issue.record("expected telemetry setup failure")
    } catch {
        #expect(error.localizedDescription == "transient")
    }

    #expect(startupEvents.events == [.configuredTelemetry])
    #expect(registry.registeredPlistNames.isEmpty)
    #expect(client.queries.isEmpty)
}

@MainActor
@Test
func viewerDefaultRangeUsesUtcDaysToMatchDaemonRollupBuckets() async throws {
    let now = Date(timeIntervalSince1970: 1_782_259_200)
    let client = RecordingStartupOverviewClient(
        rollups: OverviewRollups(tokenRollups: [], costRollups: [], toolCallRollups: [])
    )
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled)),
        selectedRangePreset: .today,
        now: { now }
    )

    try await model.refreshOverview()

    #expect(client.queries == [
        OverviewRangePreset.today.range(containing: now, calendar: .kvasirRollupUTC).query
    ])
}

@MainActor
@Test
func viewerStartupSurfacesLaunchAgentApprovalAndStillLoadsOverview() async throws {
    let client = RecordingStartupOverviewClient(
        rollups: OverviewRollups(
            tokenRollups: [
                .init(day: .init(year: 2026, month: 6, day: 21), inputTokens: 2, outputTokens: 3, cacheTokens: 5)
            ],
            costRollups: [],
            toolCallRollups: []
        )
    )
    let registry = RecordingStartupLaunchAgentRegistry(status: .requiresApproval)
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: registry)
    )

    try await model.start()

    #expect(model.launchAgentOutcome == .requiresApproval)
    #expect(registry.registeredPlistNames.isEmpty)
    #expect(model.overviewSnapshot?.totals.totalTokens == 10)
}

@MainActor
@Test
func successfulOverviewRefreshClearsPreviousError() async throws {
    let client = RecordingStartupOverviewClient(
        rollups: OverviewRollups(tokenRollups: [], costRollups: [], toolCallRollups: [])
    )
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled))
    )

    model.record(error: StartupTestError.transient)
    try await model.refreshOverview()

    #expect(model.errorMessage == nil)
}

@MainActor
@Test
func staleOverviewLoadsCannotOverwriteNewerRangeSelection() async throws {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = TimeZone(secondsFromGMT: 0)!
    let client = OrderedOverviewClient(
        responses: [
            .init(
                tokenRollups: [
                    .init(day: .init(year: 2026, month: 6, day: 1), inputTokens: 1, outputTokens: 0, cacheTokens: 0)
                ],
                costRollups: [],
                toolCallRollups: []
            ),
            .init(
                tokenRollups: [
                    .init(day: .init(year: 2026, month: 6, day: 2), inputTokens: 2, outputTokens: 0, cacheTokens: 0)
                ],
                costRollups: [],
                toolCallRollups: []
            )
        ]
    )
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled)),
        now: { Date(timeIntervalSince1970: 1_782_259_200) },
        calendar: calendar
    )

    async let older: Void = model.selectRangePreset(.today)
    async let newer: Void = model.selectRangePreset(.lastThirtyDays)

    await client.waitForPendingLoads(count: 2)
    client.completeLoad(at: 1)
    try await newer
    #expect(model.overviewSnapshot?.totals.totalTokens == 2)

    client.completeLoad(at: 0)
    try await older
    #expect(model.selectedRangePreset == .lastThirtyDays)
    #expect(model.overviewSnapshot?.totals.totalTokens == 2)
}

@MainActor
@Test
func staleOverviewFailuresCannotOverwriteNewerSuccessfulRangeSelection() async throws {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = TimeZone(secondsFromGMT: 0)!
    let client = OrderedOverviewResultClient(
        results: [
            .failure(StartupTestError.transient),
            .success(
                OverviewRollups(
                    tokenRollups: [
                        .init(day: .init(year: 2026, month: 6, day: 2), inputTokens: 2, outputTokens: 0, cacheTokens: 0)
                    ],
                    costRollups: [],
                    toolCallRollups: []
                )
            )
        ]
    )
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled)),
        now: { Date(timeIntervalSince1970: 1_782_259_200) },
        calendar: calendar
    )

    async let older: Void = model.selectRangePreset(.today)
    await client.waitForPendingLoads(count: 1)
    async let newer: Void = model.selectRangePreset(.lastThirtyDays)
    await client.waitForPendingLoads(count: 2)

    client.completeLoad(at: 1)
    try await newer
    #expect(model.overviewSnapshot?.totals.totalTokens == 2)

    client.completeLoad(at: 0)
    try await older
    #expect(model.selectedRangePreset == .lastThirtyDays)
    #expect(model.errorMessage == nil)
    #expect(model.overviewSnapshot?.totals.totalTokens == 2)
}

private final class RecordingStartupOverviewClient: OverviewClient, @unchecked Sendable {
    private let rollups: OverviewRollups
    private(set) var queries: [OverviewQuery] = []

    init(rollups: OverviewRollups) {
        self.rollups = rollups
    }

    func loadOverviewRollups(query: OverviewQuery) async throws -> OverviewRollups {
        queries.append(query)
        return rollups
    }
}

private final class OrderedOverviewClient: OverviewClient, @unchecked Sendable {
    private let lock = NSLock()
    private let responses: [OverviewRollups]
    private var pendingContinuations: [CheckedContinuation<Void, Never>?] = []

    init(responses: [OverviewRollups]) {
        self.responses = responses
    }

    func loadOverviewRollups(query: OverviewQuery) async throws -> OverviewRollups {
        let index = lock.withLock {
            let index = pendingContinuations.count
            pendingContinuations.append(nil)
            return index
        }
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            lock.withLock {
                pendingContinuations[index] = continuation
            }
        }
        return responses[index]
    }

    func waitForPendingLoads(count: Int) async {
        while !lock.withLock({ pendingContinuations.count >= count && pendingContinuations.prefix(count).allSatisfy { $0 != nil } }) {
            await Task.yield()
        }
    }

    func completeLoad(at index: Int) {
        let continuation = lock.withLock { pendingContinuations[index] }
        continuation?.resume()
    }
}

private final class OrderedOverviewResultClient: OverviewClient, @unchecked Sendable {
    private let lock = NSLock()
    private let results: [Result<OverviewRollups, any Error>]
    private var pendingContinuations: [CheckedContinuation<Void, Never>?] = []

    init(results: [Result<OverviewRollups, any Error>]) {
        self.results = results
    }

    func loadOverviewRollups(query: OverviewQuery) async throws -> OverviewRollups {
        let index = lock.withLock {
            let index = pendingContinuations.count
            pendingContinuations.append(nil)
            return index
        }
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            lock.withLock {
                pendingContinuations[index] = continuation
            }
        }
        return try results[index].get()
    }

    func waitForPendingLoads(count: Int) async {
        while !lock.withLock({ pendingContinuations.count >= count && pendingContinuations.prefix(count).allSatisfy { $0 != nil } }) {
            await Task.yield()
        }
    }

    func completeLoad(at index: Int) {
        let continuation = lock.withLock { pendingContinuations[index] }
        continuation?.resume()
    }
}

private final class RecordingStartupLaunchAgentRegistry: LaunchAgentRegistry {
    private let launchAgentStatus: LaunchAgentStatus
    private let events: StartupEventRecorder?
    private(set) var registeredPlistNames: [String] = []
    private(set) var unregisteredPlistNames: [String] = []

    init(status: LaunchAgentStatus, events: StartupEventRecorder? = nil) {
        self.launchAgentStatus = status
        self.events = events
    }

    func status(plistName: String) -> LaunchAgentStatus {
        launchAgentStatus
    }

    func register(plistName: String) throws {
        events?.append(.registeredLaunchAgent)
        registeredPlistNames.append(plistName)
    }

    func unregister(plistName: String) throws {
        unregisteredPlistNames.append(plistName)
    }
}

private final class RecordingHarnessTelemetrySetup: HarnessTelemetrySetup, @unchecked Sendable {
    private let events: StartupEventRecorder
    private let error: (any Error)?

    init(events: StartupEventRecorder, error: (any Error)? = nil) {
        self.events = events
        self.error = error
    }

    func ensureConfigured() async throws {
        events.append(.configuredTelemetry)
        if let error {
            throw error
        }
    }
}

private final class StartupEventRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private(set) var events: [StartupEvent] = []

    func append(_ event: StartupEvent) {
        lock.withLock {
            events.append(event)
        }
    }
}

private enum StartupEvent: Equatable {
    case configuredTelemetry
    case registeredLaunchAgent
}

private extension OverviewTimeRange {
    var query: OverviewQuery {
        OverviewQuery(start: start, end: end)
    }
}

private enum StartupTestError: LocalizedError {
    case transient

    var errorDescription: String? {
        "transient"
    }
}
