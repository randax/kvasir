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
        snapshot: overviewSnapshot(totalTokens: 35, costUsdNanos: 42, toolCalls: 3)
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
func liveOverviewUpdateReloadsDashboardForCurrentSelection() async throws {
    let now = Date(timeIntervalSince1970: 1_782_259_200)
    let updateSource = ManualOverviewUpdateSource()
    let client = RecordingResultOverviewClient(
        results: [
            .success(overviewSnapshot(totalTokens: 35)),
            .success(overviewSnapshot(totalTokens: 42))
        ]
    )
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client, updateSource: updateSource),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled)),
        now: { now }
    )

    try await model.start()
    #expect(model.overviewSnapshot?.totals.totalTokens == 35)

    updateSource.send()
    await client.waitForQueries(count: 2)
    await waitUntil(model.overviewSnapshot?.totals.totalTokens == 42)

    #expect(model.overviewSnapshot?.totals.totalTokens == 42)
    #expect(client.queries == [
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC).query,
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC).query
    ])
}

@MainActor
@Test
func failedLiveOverviewUpdateKeepsSnapshotAndListenerRecovers() async throws {
    let updateSource = ManualOverviewUpdateSource()
    let client = RecordingResultOverviewClient(
        results: [
            .success(overviewSnapshot(totalTokens: 35)),
            .failure(StartupTestError.transient),
            .success(overviewSnapshot(totalTokens: 42))
        ]
    )
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client, updateSource: updateSource),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled))
    )

    try await model.start()
    let previousSnapshot = model.overviewSnapshot

    updateSource.send()
    await client.waitForQueries(count: 2)
    await waitUntil(model.errorMessage == "transient")

    #expect(model.overviewSnapshot == previousSnapshot)
    #expect(model.errorMessage == "transient")

    updateSource.send()
    await client.waitForQueries(count: 3)
    await waitUntil(model.overviewSnapshot?.totals.totalTokens == 42)

    #expect(model.overviewSnapshot?.totals.totalTokens == 42)
    #expect(model.errorMessage == nil)
}

@MainActor
@Test
func liveOverviewListenerDoesNotRetainModel() async throws {
    let updateSource = ManualOverviewUpdateSource()
    let client = RecordingResultOverviewClient(
        results: [
            .success(overviewSnapshot(totalTokens: 35))
        ]
    )
    var model: KvasirViewerModel? = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client, updateSource: updateSource),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled))
    )
    let releasedModel = WeakKvasirViewerModelReference(model)

    try await model?.start()
    #expect(releasedModel.model?.overviewSnapshot?.totals.totalTokens == 35)

    model = nil
    await waitUntil(releasedModel.model == nil)

    #expect(releasedModel.model == nil)
}

@MainActor
@Test
func successfulRefreshAfterStartupFailureStartsLiveOverviewUpdates() async throws {
    let updateSource = ManualOverviewUpdateSource()
    let client = RecordingResultOverviewClient(
        results: [
            .failure(StartupTestError.transient),
            .success(overviewSnapshot(totalTokens: 35)),
            .success(overviewSnapshot(totalTokens: 42))
        ]
    )
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client, updateSource: updateSource),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled))
    )

    do {
        try await model.start()
        Issue.record("expected startup overview failure")
    } catch {
        #expect(error.localizedDescription == "transient")
    }
    #expect(model.overviewSnapshot == nil)

    try await model.refreshOverview()
    #expect(model.overviewSnapshot?.totals.totalTokens == 35)

    updateSource.send()
    await client.waitForQueries(count: 3)
    await waitUntil(model.overviewSnapshot?.totals.totalTokens == 42)

    #expect(model.overviewSnapshot?.totals.totalTokens == 42)
}

@MainActor
@Test
func viewerStartupWarnsAndContinuesWhenTelemetrySetupFails() async throws {
    let client = RecordingStartupOverviewClient(
        snapshot: overviewSnapshot()
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

    try await model.start()

    #expect(startupEvents.events == [.configuredTelemetry, .registeredLaunchAgent])
    #expect(model.setupWarningMessage == "transient")
    #expect(model.launchAgentOutcome == .registered)
    #expect(registry.registeredPlistNames == [DaemonLaunchAgent.plistName])
    #expect(client.queries.count == 1)
    #expect(model.overviewSnapshot?.totals == .init(totalTokens: 0, costUsdNanos: 0, toolCalls: 0))
}

@MainActor
@Test
func viewerDefaultRangeUsesUtcDaysToMatchDaemonRollupBuckets() async throws {
    let now = Date(timeIntervalSince1970: 1_782_259_200)
    let client = RecordingStartupOverviewClient(
        snapshot: overviewSnapshot()
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
        snapshot: overviewSnapshot(totalTokens: 10)
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
func viewerStartupKeepsPostStartupOverviewRecoveryClosedWhenLaunchAgentRequiresApprovalAndOverviewFails() async throws {
    let recoveryGate = RecordingOverviewRecoveryGate()
    let client = GateRecordingResultOverviewClient(
        results: [
            .failure(StartupTestError.transient),
            .failure(StartupTestError.transient),
        ],
        isGateEnabled: { recoveryGate.isEnabled }
    )
    let registry = RecordingStartupLaunchAgentRegistry(status: .requiresApproval)
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: registry),
        shouldRefreshLaunchAgentAfterStartupOverviewError: { _ in true },
        enablePostStartupOverviewRecovery: recoveryGate.enable
    )

    do {
        try await model.start()
        Issue.record("expected startup overview failure")
    } catch {
        #expect(error.localizedDescription == "transient")
    }

    do {
        try await model.refreshOverview()
        Issue.record("expected later overview failure")
    } catch {
        #expect(error.localizedDescription == "transient")
    }

    #expect(model.launchAgentOutcome == .requiresApproval)
    #expect(registry.unregisteredPlistNames.isEmpty)
    #expect(registry.registeredPlistNames.isEmpty)
    #expect(client.gateStates == [false, false])
    #expect(recoveryGate.enableCount == 0)
}

@MainActor
@Test
func viewerStartupRefreshesDaemonRegistrationAndRetriesWhenInitialOverviewFails() async throws {
    let client = RecordingResultOverviewClient(
        results: [
            .failure(StartupTestError.transient),
            .success(
                overviewSnapshot(totalTokens: 12)
            )
        ]
    )
    let registry = RecordingStartupLaunchAgentRegistry(status: .enabled)
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: registry),
        shouldRefreshLaunchAgentAfterStartupOverviewError: { _ in true }
    )

    try await model.start()

    #expect(model.launchAgentOutcome == .registered)
    #expect(registry.unregisteredPlistNames == [DaemonLaunchAgent.plistName])
    #expect(registry.registeredPlistNames == [DaemonLaunchAgent.plistName])
    #expect(client.queries.count == 2)
    #expect(model.overviewSnapshot?.totals.totalTokens == 12)
}

@MainActor
@Test
func viewerStartupOpensPostStartupOverviewRecoveryBeforeRetryAfterLaunchAgentRefresh() async throws {
    let recoveryGate = RecordingOverviewRecoveryGate()
    let client = GateRecordingResultOverviewClient(
        results: [
            .failure(StartupTestError.transient),
            .success(
                overviewSnapshot(totalTokens: 12)
            )
        ],
        isGateEnabled: { recoveryGate.isEnabled }
    )
    let registry = RecordingStartupLaunchAgentRegistry(status: .enabled)
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: registry),
        shouldRefreshLaunchAgentAfterStartupOverviewError: { _ in true },
        enablePostStartupOverviewRecovery: recoveryGate.enable
    )

    try await model.start()

    #expect(registry.unregisteredPlistNames == [DaemonLaunchAgent.plistName])
    #expect(registry.registeredPlistNames == [DaemonLaunchAgent.plistName])
    #expect(client.gateStates == [false, true])
    #expect(recoveryGate.enableCount == 1)
    #expect(model.overviewSnapshot?.totals.totalTokens == 12)
}

@MainActor
@Test
func viewerStartupDoesNotRefreshDaemonRegistrationForNonRecoverableOverviewFailure() async throws {
    let client = RecordingResultOverviewClient(
        results: [
            .failure(StartupTestError.transient)
        ]
    )
    let registry = RecordingStartupLaunchAgentRegistry(status: .enabled)
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: registry)
    )

    do {
        try await model.start()
        Issue.record("expected startup overview failure")
    } catch {
        #expect(error.localizedDescription == "transient")
    }

    #expect(model.launchAgentOutcome == .alreadyRegistered)
    #expect(registry.unregisteredPlistNames.isEmpty)
    #expect(registry.registeredPlistNames.isEmpty)
    #expect(client.queries.count == 1)
}

@MainActor
@Test
func successfulOverviewRefreshClearsPreviousError() async throws {
    let client = RecordingStartupOverviewClient(
        snapshot: overviewSnapshot()
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
func selectingRepoReloadsOverviewForCurrentRange() async throws {
    let now = Date(timeIntervalSince1970: 1_782_259_200)
    let repo = OverviewRepoBucket.repo(
        OverviewRepoIdentity(
            name: OverviewRepoName("kvasir"),
            path: OverviewRepoPath("/repos/kvasir")
        )!
    )
    let client = RecordingStartupOverviewClient(
        snapshot: overviewSnapshot(totalTokens: 13, costUsdNanos: 12, toolCalls: 3, selectedRepo: repo)
    )
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled)),
        now: { now }
    )

    try await model.selectRepo(repo)

    #expect(model.selectedRepo == repo)
    #expect(model.overviewSnapshot?.selectedRepo == repo)
    #expect(client.queries == [
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC).query(repo: repo)
    ])
}

@MainActor
@Test
func selectingModelReloadsOverviewForCurrentRange() async throws {
    let now = Date(timeIntervalSince1970: 1_782_259_200)
    let selectedModel = OverviewModelName("claude-sonnet-4-20250514")
    let client = RecordingStartupOverviewClient(
        snapshot: overviewSnapshot(totalTokens: 13, costUsdNanos: 12, selectedModel: selectedModel)
    )
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled)),
        now: { now }
    )

    try await model.selectModel(selectedModel)

    #expect(model.selectedModel == selectedModel)
    #expect(model.overviewSnapshot?.selectedModel == selectedModel)
    #expect(client.queries == [
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC).query(model: selectedModel)
    ])
}

@MainActor
@Test
func selectingModelKeepsActiveRepoScope() async throws {
    let now = Date(timeIntervalSince1970: 1_782_259_200)
    let repo = OverviewRepoBucket.repo(
        OverviewRepoIdentity(
            name: OverviewRepoName("kvasir"),
            path: OverviewRepoPath("/repos/kvasir")
        )!
    )
    let selectedModel = OverviewModelName("claude-sonnet-4-20250514")
    let client = RecordingResultOverviewClient(
        results: [
            .success(overviewSnapshot(totalTokens: 21, selectedRepo: repo)),
            .success(overviewSnapshot(totalTokens: 13, selectedRepo: repo, selectedModel: selectedModel))
        ]
    )
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled)),
        now: { now }
    )

    try await model.selectRepo(repo)
    try await model.selectModel(selectedModel)

    #expect(model.selectedRepo == repo)
    #expect(model.selectedModel == selectedModel)
    #expect(client.queries == [
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC).query(repo: repo),
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC).query(repo: repo, model: selectedModel)
    ])
}

@MainActor
@Test
func selectingRepoKeepsActiveModelScope() async throws {
    let now = Date(timeIntervalSince1970: 1_782_259_200)
    let repo = OverviewRepoBucket.repo(
        OverviewRepoIdentity(
            name: OverviewRepoName("kvasir"),
            path: OverviewRepoPath("/repos/kvasir")
        )!
    )
    let selectedModel = OverviewModelName("claude-sonnet-4-20250514")
    let client = RecordingResultOverviewClient(
        results: [
            .success(overviewSnapshot(totalTokens: 21, selectedModel: selectedModel)),
            .success(overviewSnapshot(totalTokens: 13, selectedRepo: repo, selectedModel: selectedModel))
        ]
    )
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled)),
        now: { now }
    )

    try await model.selectModel(selectedModel)
    try await model.selectRepo(repo)

    #expect(model.selectedRepo == repo)
    #expect(model.selectedModel == selectedModel)
    #expect(client.queries == [
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC).query(model: selectedModel),
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC).query(repo: repo, model: selectedModel)
    ])
}

@MainActor
@Test
func drillDownProgressesFromRepoToModelToSessionToPrompt() async throws {
    let now = Date(timeIntervalSince1970: 1_782_259_200)
    let repo = OverviewRepoBucket.repo(
        OverviewRepoIdentity(
            name: OverviewRepoName("kvasir"),
            path: OverviewRepoPath("/repos/kvasir")
        )!
    )
    let modelName = OverviewModelName("claude-sonnet-4-20250514")
    let session = OverviewSessionRoute(
        harness: OverviewHarnessName("claude_code"),
        sessionID: OverviewSessionID("session-12")
    )
    let prompt = OverviewPromptRoute(
        session: session,
        promptID: OverviewPromptID("prompt-7")
    )
    let client = RecordingResultOverviewClient(
        results: [
            .success(overviewSnapshot(totalTokens: 21, selectedRepo: repo)),
            .success(overviewSnapshot(totalTokens: 13, selectedRepo: repo, selectedModel: modelName)),
            .success(overviewSnapshot(totalTokens: 8, selectedRepo: repo, selectedModel: modelName, selectedSession: session)),
            .success(overviewSnapshot(totalTokens: 5, selectedRepo: repo, selectedModel: modelName, selectedSession: session, selectedPrompt: prompt))
        ]
    )
    let viewer = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled)),
        now: { now }
    )

    try await viewer.drillDown(to: .repo(repo))
    try await viewer.drillDown(to: .model(modelName))
    try await viewer.drillDown(to: .session(session))
    try await viewer.drillDown(to: .prompt(prompt))

    #expect(viewer.selectedRepo == repo)
    #expect(viewer.selectedModel == modelName)
    #expect(viewer.selectedSession == session)
    #expect(viewer.selectedPrompt == prompt)
    #expect(client.queries == [
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC).query(repo: repo),
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC).query(repo: repo, model: modelName),
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC)
            .query(repo: repo, model: modelName, session: session),
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC)
            .query(repo: repo, model: modelName, session: session, prompt: prompt)
    ])
}

@MainActor
@Test
func promptDrillDownLoadsTraceInspectorSnapshot() async throws {
    let session = OverviewSessionRoute(
        harness: OverviewHarnessName("opencode"),
        sessionID: OverviewSessionID("opencode-session-1")
    )
    let prompt = OverviewPromptRoute(
        session: session,
        promptID: OverviewPromptID("opencode-turn-1")
    )
    let inspectorSnapshot = TraceInspectorSnapshot(
        prompt: prompt,
        traces: [
            TraceInspectorTrace(
                traceID: TraceInspectorTraceID("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                spans: [
                    TraceInspectorSpan(
                        spanID: TraceInspectorSpanID("1111111111111111"),
                        parentSpanID: nil,
                        kind: .interaction,
                        name: TraceInspectorSpanName("opencode.interaction"),
                        startedAt: Date(timeIntervalSince1970: 1_781_956_800),
                        endedAt: Date(timeIntervalSince1970: 1_781_956_802),
                        durationMilliseconds: 2_000,
                        toolName: nil
                    )
                ],
                durations: TraceInspectorDurations(
                    timeToFirstTokenMilliseconds: 250,
                    requestMilliseconds: 1_500,
                    toolMilliseconds: nil
                )
            )
        ],
        content: [
            TraceInspectorContentItem(
                occurredAt: Date(timeIntervalSince1970: 1_781_956_801),
                harness: OverviewHarnessName("opencode"),
                kind: .userPrompt,
                content: TraceInspectorContentText("summarize README.md")
            )
        ],
        contentAvailability: .captured(
            harness: OverviewHarnessName("opencode"),
            kinds: [
                .captured(.userPrompt)
            ]
        )
    )
    let overviewClient = RecordingResultOverviewClient(
        results: [
            .success(overviewSnapshot(totalTokens: 5, selectedSession: session, selectedPrompt: prompt))
        ]
    )
    let traceInspectorClient = RecordingTraceInspectorClient(
        snapshot: inspectorSnapshot
    )
    let viewer = KvasirViewerModel(
        dashboard: OverviewDashboard(client: overviewClient),
        traceInspector: TraceInspector(client: traceInspectorClient),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled))
    )

    try await viewer.drillDown(to: .prompt(prompt))

    #expect(traceInspectorClient.queries == [
        TraceInspectorQuery(prompt: prompt)
    ])
    #expect(viewer.traceInspectorSnapshot == inspectorSnapshot)
    #expect(viewer.traceInspectorErrorMessage == nil)
}

@MainActor
@Test
func failedSessionDrillDownKeepsPreviousPromptAndSnapshot() async throws {
    let now = Date(timeIntervalSince1970: 1_782_259_200)
    let session = OverviewSessionRoute(
        harness: OverviewHarnessName("claude_code"),
        sessionID: OverviewSessionID("session-12")
    )
    let prompt = OverviewPromptRoute(
        session: session,
        promptID: OverviewPromptID("prompt-7")
    )
    let otherSession = OverviewSessionRoute(
        harness: OverviewHarnessName("codex"),
        sessionID: OverviewSessionID("session-99")
    )
    let client = RecordingResultOverviewClient(
        results: [
            .success(overviewSnapshot(totalTokens: 8, selectedSession: session)),
            .success(overviewSnapshot(totalTokens: 5, selectedSession: session, selectedPrompt: prompt)),
            .failure(StartupTestError.transient)
        ]
    )
    let viewer = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled)),
        now: { now }
    )

    try await viewer.drillDown(to: .session(session))
    try await viewer.drillDown(to: .prompt(prompt))
    let previousSnapshot = viewer.overviewSnapshot

    do {
        try await viewer.drillDown(to: .session(otherSession))
        Issue.record("expected failed session drill-down")
    } catch {
        #expect(error.localizedDescription == "transient")
    }

    #expect(viewer.selectedSession == session)
    #expect(viewer.selectedPrompt == prompt)
    #expect(viewer.overviewSnapshot == previousSnapshot)
    #expect(client.queries == [
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC)
            .query(session: session),
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC)
            .query(session: session, prompt: prompt),
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC)
            .query(session: otherSession)
    ])
}

@MainActor
@Test
func clearingPromptReloadsSessionScopeAndKeepsSessionSelected() async throws {
    let now = Date(timeIntervalSince1970: 1_782_259_200)
    let session = OverviewSessionRoute(
        harness: OverviewHarnessName("claude_code"),
        sessionID: OverviewSessionID("session-12")
    )
    let prompt = OverviewPromptRoute(
        session: session,
        promptID: OverviewPromptID("prompt-7")
    )
    let client = RecordingResultOverviewClient(
        results: [
            .success(overviewSnapshot(totalTokens: 5, selectedSession: session, selectedPrompt: prompt)),
            .success(overviewSnapshot(totalTokens: 8, selectedSession: session))
        ]
    )
    let viewer = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled)),
        now: { now }
    )

    try await viewer.drillDown(to: .prompt(prompt))
    try await viewer.clearPrompt()

    #expect(viewer.selectedSession == session)
    #expect(viewer.selectedPrompt == nil)
    #expect(client.queries == [
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC)
            .query(session: session, prompt: prompt),
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC)
            .query(session: session)
    ])
}

@MainActor
@Test
func failedClearSessionAndPromptKeepsPreviousPromptAndSnapshot() async throws {
    let now = Date(timeIntervalSince1970: 1_782_259_200)
    let session = OverviewSessionRoute(
        harness: OverviewHarnessName("claude_code"),
        sessionID: OverviewSessionID("session-12")
    )
    let prompt = OverviewPromptRoute(
        session: session,
        promptID: OverviewPromptID("prompt-7")
    )
    let client = RecordingResultOverviewClient(
        results: [
            .success(overviewSnapshot(totalTokens: 5, selectedSession: session, selectedPrompt: prompt)),
            .failure(StartupTestError.transient)
        ]
    )
    let viewer = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled)),
        now: { now }
    )

    try await viewer.drillDown(to: .prompt(prompt))
    let previousSnapshot = viewer.overviewSnapshot

    do {
        try await viewer.clearSessionAndPrompt()
        Issue.record("expected failed session/prompt clear")
    } catch {
        #expect(error.localizedDescription == "transient")
    }

    #expect(viewer.selectedSession == session)
    #expect(viewer.selectedPrompt == prompt)
    #expect(viewer.overviewSnapshot == previousSnapshot)
    #expect(client.queries == [
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC)
            .query(session: session, prompt: prompt),
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC)
            .query(harness: session.harness)
    ])
}

@MainActor
@Test
func failedRepoSelectionKeepsPreviousRepoAndSnapshot() async throws {
    let now = Date(timeIntervalSince1970: 1_782_259_200)
    let kvasirRepo = OverviewRepoBucket.repo(
        OverviewRepoIdentity(
            name: OverviewRepoName("kvasir"),
            path: OverviewRepoPath("/repos/kvasir")
        )!
    )
    let otherRepo = OverviewRepoBucket.repo(
        OverviewRepoIdentity(
            name: OverviewRepoName("other"),
            path: OverviewRepoPath("/repos/other")
        )!
    )
    let client = RecordingResultOverviewClient(
        results: [
            .success(
                overviewSnapshot(totalTokens: 13, selectedRepo: kvasirRepo)
            ),
            .failure(StartupTestError.transient)
        ]
    )
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled)),
        now: { now }
    )

    try await model.selectRepo(kvasirRepo)
    let previousSnapshot = model.overviewSnapshot

    do {
        try await model.selectRepo(otherRepo)
        Issue.record("expected failed repo selection")
    } catch {
        #expect(error.localizedDescription == "transient")
    }

    #expect(model.selectedRepo == kvasirRepo)
    #expect(model.overviewSnapshot == previousSnapshot)
    #expect(model.overviewSnapshot?.selectedRepo == kvasirRepo)
    #expect(client.queries == [
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC).query(repo: kvasirRepo),
        OverviewRangePreset.lastSevenDays.range(containing: now, calendar: .kvasirRollupUTC).query(repo: otherRepo)
    ])
}

@MainActor
@Test
func staleOverviewLoadsCannotOverwriteNewerRangeSelection() async throws {
    var calendar = Calendar(identifier: .gregorian)
    calendar.timeZone = TimeZone(secondsFromGMT: 0)!
    let client = OrderedOverviewClient(
        responses: [
            overviewSnapshot(totalTokens: 1),
            overviewSnapshot(totalTokens: 2)
        ]
    )
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: client),
        launchAgent: DaemonLaunchAgent(registry: RecordingStartupLaunchAgentRegistry(status: .enabled)),
        now: { Date(timeIntervalSince1970: 1_782_259_200) },
        calendar: calendar
    )

    let older = Task { try await model.selectRangePreset(.today) }
    await client.waitForPendingLoads(count: 1)
    let newer = Task { try await model.selectRangePreset(.lastThirtyDays) }
    await client.waitForPendingLoads(count: 2)
    client.completeLoad(at: 1)
    try await newer.value
    #expect(model.overviewSnapshot?.totals.totalTokens == 2)

    client.completeLoad(at: 0)
    try await older.value
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
                overviewSnapshot(totalTokens: 2)
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
    private let snapshot: OverviewSnapshot
    private(set) var queries: [OverviewQuery] = []

    init(snapshot: OverviewSnapshot) {
        self.snapshot = snapshot
    }

    func loadOverviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot {
        queries.append(query)
        return snapshot
    }
}

private final class RecordingResultOverviewClient: OverviewClient, @unchecked Sendable {
    private let results: [Result<OverviewSnapshot, any Error>]
    private(set) var queries: [OverviewQuery] = []
    private var queryWaiters: [(Int, CheckedContinuation<Void, Never>)] = []

    init(results: [Result<OverviewSnapshot, any Error>]) {
        self.results = results
    }

    func loadOverviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot {
        queries.append(query)
        resumeQueryWaiters()
        return try results[queries.count - 1].get()
    }

    func waitForQueries(count: Int) async {
        guard queries.count < count else {
            return
        }
        await withCheckedContinuation { continuation in
            queryWaiters.append((count, continuation))
        }
    }

    private func resumeQueryWaiters() {
        let readyWaiters = queryWaiters.filter { queries.count >= $0.0 }
        queryWaiters.removeAll { queries.count >= $0.0 }
        for waiter in readyWaiters {
            waiter.1.resume()
        }
    }
}

private final class RecordingTraceInspectorClient: TraceInspectorClient, @unchecked Sendable {
    private let snapshot: TraceInspectorSnapshot
    private(set) var queries: [TraceInspectorQuery] = []

    init(snapshot: TraceInspectorSnapshot) {
        self.snapshot = snapshot
    }

    func loadTraceInspector(query: TraceInspectorQuery) async throws -> TraceInspectorSnapshot {
        queries.append(query)
        return snapshot
    }
}

private final class GateRecordingResultOverviewClient: OverviewClient, @unchecked Sendable {
    private let results: [Result<OverviewSnapshot, any Error>]
    private let isGateEnabled: @Sendable () -> Bool
    private(set) var queries: [OverviewQuery] = []
    private(set) var gateStates: [Bool] = []

    init(
        results: [Result<OverviewSnapshot, any Error>],
        isGateEnabled: @escaping @Sendable () -> Bool
    ) {
        self.results = results
        self.isGateEnabled = isGateEnabled
    }

    func loadOverviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot {
        queries.append(query)
        gateStates.append(isGateEnabled())
        return try results[queries.count - 1].get()
    }
}

private final class OrderedOverviewClient: OverviewClient, @unchecked Sendable {
    private let lock = NSLock()
    private let responses: [OverviewSnapshot]
    private var pendingContinuations: [CheckedContinuation<Void, Never>?] = []

    init(responses: [OverviewSnapshot]) {
        self.responses = responses
    }

    func loadOverviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot {
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
    private let results: [Result<OverviewSnapshot, any Error>]
    private var pendingContinuations: [CheckedContinuation<Void, Never>?] = []

    init(results: [Result<OverviewSnapshot, any Error>]) {
        self.results = results
    }

    func loadOverviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot {
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

private final class RecordingOverviewRecoveryGate: @unchecked Sendable {
    private let lock = NSLock()
    private var enabled = false
    private(set) var enableCount = 0

    var isEnabled: Bool {
        lock.withLock {
            enabled
        }
    }

    func enable() {
        lock.withLock {
            enabled = true
            enableCount += 1
        }
    }
}

private final class WeakKvasirViewerModelReference {
    weak var model: KvasirViewerModel?

    init(_ model: KvasirViewerModel?) {
        self.model = model
    }
}

private final class ManualOverviewUpdateSource: OverviewUpdateSource, @unchecked Sendable {
    private let continuation: AsyncStream<Void>.Continuation
    private let stream: AsyncStream<Void>

    init() {
        var continuation: AsyncStream<Void>.Continuation!
        stream = AsyncStream { continuation = $0 }
        self.continuation = continuation
    }

    func overviewRefreshEvents() -> AsyncStream<Void> {
        stream
    }

    func send() {
        continuation.yield(())
    }
}

private enum StartupEvent: Equatable {
    case configuredTelemetry
    case registeredLaunchAgent
}

private func overviewSnapshot(
    totalTokens: UInt64 = 0,
    costUsdNanos: UInt64 = 0,
    toolCalls: UInt64 = 0,
    selectedRepo: OverviewRepoBucket? = nil,
    selectedModel: OverviewModelName? = nil,
    selectedSession: OverviewSessionRoute? = nil,
    selectedPrompt: OverviewPromptRoute? = nil
) -> OverviewSnapshot {
    OverviewSnapshot(
        totals: OverviewTotals(
            totalTokens: totalTokens,
            costUsdNanos: costUsdNanos,
            toolCalls: toolCalls
        ),
        series: [],
        repoBreakdown: selectedRepo.map {
            [OverviewRepoSummary(
                repo: $0,
                totals: OverviewTotals(
                    totalTokens: totalTokens,
                    costUsdNanos: costUsdNanos,
                    toolCalls: toolCalls
                )
            )]
        } ?? [],
        modelBreakdown: selectedModel.map {
            [OverviewModelSummary(
                model: $0,
                totals: OverviewTotals(
                    totalTokens: totalTokens,
                    costUsdNanos: costUsdNanos,
                    toolCalls: toolCalls
                )
            )]
        } ?? [],
        selectedRepo: selectedRepo,
        selectedModel: selectedModel,
        selectedSession: selectedSession,
        selectedPrompt: selectedPrompt
    )
}

private extension OverviewTimeRange {
    var query: OverviewQuery {
        OverviewQuery(start: start, end: end)
    }

    func query(repo: OverviewRepoBucket?) -> OverviewQuery {
        OverviewQuery(start: start, end: end, repo: repo)
    }

    func query(model: OverviewModelName?) -> OverviewQuery {
        OverviewQuery(start: start, end: end, model: model)
    }

    func query(repo: OverviewRepoBucket?, model: OverviewModelName?) -> OverviewQuery {
        OverviewQuery(start: start, end: end, repo: repo, model: model)
    }

    func query(
        repo: OverviewRepoBucket? = nil,
        model: OverviewModelName? = nil,
        harness: OverviewHarnessName? = nil,
        session: OverviewSessionRoute? = nil,
        prompt: OverviewPromptRoute? = nil
    ) -> OverviewQuery {
        let effectiveHarness = prompt?.session.harness ?? session?.harness ?? harness
        return OverviewQuery(
            start: start,
            end: end,
            repo: repo,
            model: model,
            harness: effectiveHarness,
            session: session,
            prompt: prompt
        )
    }
}

private enum StartupTestError: LocalizedError {
    case transient

    var errorDescription: String? {
        "transient"
    }
}

@MainActor
private func waitUntil(
    _ condition: @autoclosure () -> Bool,
    sourceLocation: SourceLocation = #_sourceLocation
) async {
    for _ in 0..<1_000 where !condition() {
        await Task.yield()
    }
    if !condition() {
        Issue.record("condition was not met", sourceLocation: sourceLocation)
    }
}
