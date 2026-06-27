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

    #if canImport(kvasir_client)
    #expect(model.isTraceInspectorEnabled)
    #else
    #expect(!model.isTraceInspectorEnabled)
    do {
        try await model.refreshOverview()
        Issue.record("expected missing kvasir-client error from package-test build")
    } catch {
        #expect(error.localizedDescription.contains("kvasir-client"))
    }
    #endif
}

@MainActor
@Test
func productionFactoryUsesInjectedTraceInspectorClientForPromptDrillDown() async throws {
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
        traces: [],
        content: [],
        contentAvailability: .unavailable(reason: .notCapturedForPrompt)
    )
    let overviewClient = SequenceOverviewClient(results: [
        .success(overviewSnapshot(totalTokens: 9, selectedSession: session, selectedPrompt: prompt))
    ])
    let traceClient = RecordingTraceInspectorClient(snapshot: inspectorSnapshot)
    let model = ProductionModelFactory.make(
        overviewClient: overviewClient,
        traceInspectorClient: traceClient
    )

    try await model.drillDown(to: .prompt(prompt))

    #expect(await traceClient.queries == [TraceInspectorQuery(prompt: prompt)])
    #expect(model.traceInspectorSnapshot == inspectorSnapshot)
    #expect(model.traceInspectorErrorMessage == nil)
    #expect(model.errorMessage == nil)
}

@MainActor
@Test
func overviewScreenShowsTraceInspectorOnlyWhenEnabledAndPromptSelected() {
    let prompt = OverviewPromptRoute(
        session: OverviewSessionRoute(
            harness: OverviewHarnessName("opencode"),
            sessionID: OverviewSessionID("opencode-session-1")
        ),
        promptID: OverviewPromptID("opencode-turn-1")
    )

    #expect(OverviewScreen.showsTraceInspectorDashboard(
        isTraceInspectorEnabled: true,
        selectedPrompt: prompt
    ))
    #expect(!OverviewScreen.showsTraceInspectorDashboard(
        isTraceInspectorEnabled: false,
        selectedPrompt: prompt
    ))
    #expect(!OverviewScreen.showsTraceInspectorDashboard(
        isTraceInspectorEnabled: true,
        selectedPrompt: nil
    ))
}

@Test
func overviewScreenCostPresentationCarriesVisibleEstimateMarkers() {
    let snapshot = OverviewSnapshot(
        totals: .init(totalTokens: 30, costUsdNanos: 9_000, costSource: .mixed, toolCalls: 1),
        series: [
            .init(
                day: .init(year: 2026, month: 6, day: 20),
                totalTokens: 10,
                costUsdNanos: 2_000,
                costSource: .native,
                toolCalls: 0
            ),
            .init(
                day: .init(year: 2026, month: 6, day: 21),
                totalTokens: 20,
                costUsdNanos: 7_000,
                costSource: .estimated,
                toolCalls: 1
            ),
        ],
        repoBreakdown: [
            .init(
                repo: .noRepo,
                totals: .init(totalTokens: 30, costUsdNanos: 9_000, costSource: .mixed, toolCalls: 1)
            )
        ],
        modelBreakdown: [
            .init(
                model: OverviewModelName("claude-sonnet-4"),
                totals: .init(totalTokens: 30, costUsdNanos: 9_000, costSource: .estimated, toolCalls: 0)
            )
        ],
        selectedRepo: nil
    )

    let presentation = OverviewScreen.costDashboardPresentation(for: snapshot)

    #expect(presentation.total.estimateLabel == "Partly estimated")
    #expect(presentation.series.map(\.chartMarkerLabel) == [nil, "Est."])
    #expect(presentation.repos.map(\.estimateLabel) == ["Partly estimated"])
    #expect(presentation.models.map(\.estimateLabel) == ["Estimated"])
}

#if canImport(kvasir_client)
@Test
func kvasirTraceInspectorMappingPreservesTraceAndCapturedReplay() {
    let prompt = OverviewPromptRoute(
        session: OverviewSessionRoute(
            harness: OverviewHarnessName("opencode"),
            sessionID: OverviewSessionID("opencode-session-1")
        ),
        promptID: OverviewPromptID("opencode-turn-1")
    )
    let mapped = traceInspectorSnapshotFromKvasir(
        prompt: prompt,
        traces: [
            KvasirTrace(
                sessionId: "opencode-session-1",
                promptId: "opencode-turn-1",
                traceId: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                spans: [
                    KvasirTraceSpan(
                        spanId: "span-root",
                        parentSpanId: nil,
                        kind: .interaction,
                        name: "opencode.interaction",
                        startedAt: KvasirTimestampMillis(value: 1_781_956_800_000),
                        endedAt: KvasirTimestampMillis(value: 1_781_956_803_000),
                        durationMs: 3_000,
                        toolName: nil
                    ),
                    KvasirTraceSpan(
                        spanId: "span-request",
                        parentSpanId: "span-root",
                        kind: .llmRequest,
                        name: "opencode.generate_text",
                        startedAt: KvasirTimestampMillis(value: 1_781_956_801_000),
                        endedAt: KvasirTimestampMillis(value: 1_781_956_802_000),
                        durationMs: 1_000,
                        toolName: nil
                    ),
                ],
                durations: KvasirTraceDurationMeasures(
                    ttftMs: 250,
                    requestMs: 1_000,
                    toolMs: nil
                )
            )
        ],
        replay: KvasirContentReplay(
            sessionId: "opencode-session-1",
            promptId: "opencode-turn-1",
            items: [
                KvasirContentReplayItem(
                    occurredAt: KvasirTimestampMillis(value: 1_781_956_801_000),
                    harness: "opencode",
                    kind: .rawApiRequest,
                    content: "{\"messages\":[]}"
                ),
                KvasirContentReplayItem(
                    occurredAt: KvasirTimestampMillis(value: 1_781_956_802_000),
                    harness: "opencode",
                    kind: .rawApiResponse,
                    content: "{\"text\":\"done\"}"
                ),
            ],
            availability: .captured(
                harness: "opencode",
                kinds: [
                    .captured(kind: .rawApiRequest),
                    .captured(kind: .rawApiResponse),
                    .unavailable(kind: .toolOutput, reason: .notCapturedForPrompt),
                ]
            )
        )
    )

    #expect(mapped == TraceInspectorSnapshot(
        prompt: prompt,
        traces: [
            TraceInspectorTrace(
                traceID: TraceInspectorTraceID("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
                spans: [
                    TraceInspectorSpan(
                        spanID: TraceInspectorSpanID("span-root"),
                        parentSpanID: nil,
                        kind: .interaction,
                        name: TraceInspectorSpanName("opencode.interaction"),
                        startedAt: Date(timeIntervalSince1970: 1_781_956_800),
                        endedAt: Date(timeIntervalSince1970: 1_781_956_803),
                        durationMilliseconds: 3_000,
                        toolName: nil
                    ),
                    TraceInspectorSpan(
                        spanID: TraceInspectorSpanID("span-request"),
                        parentSpanID: TraceInspectorSpanID("span-root"),
                        kind: .llmRequest,
                        name: TraceInspectorSpanName("opencode.generate_text"),
                        startedAt: Date(timeIntervalSince1970: 1_781_956_801),
                        endedAt: Date(timeIntervalSince1970: 1_781_956_802),
                        durationMilliseconds: 1_000,
                        toolName: nil
                    ),
                ],
                durations: TraceInspectorDurations(
                    timeToFirstTokenMilliseconds: 250,
                    requestMilliseconds: 1_000,
                    toolMilliseconds: nil
                )
            )
        ],
        content: [
            TraceInspectorContentItem(
                occurredAt: Date(timeIntervalSince1970: 1_781_956_801),
                harness: OverviewHarnessName("opencode"),
                kind: .rawApiRequest,
                content: TraceInspectorContentText("{\"messages\":[]}")
            ),
            TraceInspectorContentItem(
                occurredAt: Date(timeIntervalSince1970: 1_781_956_802),
                harness: OverviewHarnessName("opencode"),
                kind: .rawApiResponse,
                content: TraceInspectorContentText("{\"text\":\"done\"}")
            ),
        ],
        contentAvailability: .captured(
            harness: OverviewHarnessName("opencode"),
            kinds: [
                .captured(.rawApiRequest),
                .captured(.rawApiResponse),
                .unavailable(kind: .toolOutput, reason: .notCapturedForPrompt),
            ]
        )
    ))
}

@Test
func kvasirTraceInspectorMappingPreservesUnavailableReplayReason() {
    let prompt = OverviewPromptRoute(
        session: OverviewSessionRoute(
            harness: OverviewHarnessName("opencode"),
            sessionID: OverviewSessionID("opencode-session-404")
        ),
        promptID: OverviewPromptID("missing-turn")
    )
    let mapped = traceInspectorSnapshotFromKvasir(
        prompt: prompt,
        traces: [],
        replay: KvasirContentReplay(
            sessionId: "opencode-session-404",
            promptId: "missing-turn",
            items: [],
            availability: .unavailable(reason: .promptNotFound)
        )
    )

    #expect(mapped.contentAvailability == .unavailable(reason: .promptNotFound))
    #expect(mapped.content.isEmpty)
}

@Test
func kvasirOverviewSnapshotMappingPreservesAggregatedSnapshot() {
    let repo = KvasirRepoBucket(kind: .repo, name: "kvasir", path: "/repos/kvasir")
    let model = "claude-sonnet-4-20250514"
    let mapped = overviewSnapshotFromKvasir(
        KvasirOverviewSnapshot(
            totals: KvasirOverviewTotals(
                totalTokens: 13,
                costUsdNanos: 21,
                costSource: .estimated,
                toolCalls: 3
            ),
            series: [
                KvasirOverviewSeriesPoint(
                    day: KvasirRollupDay(year: 2026, month: 6, day: 24),
                    totalTokens: 13,
                    costUsdNanos: 21,
                    costSource: .estimated,
                    toolCalls: 3
                )
            ],
            repoBreakdown: [
                KvasirOverviewRepoSummary(
                    repo: repo,
                    totals: KvasirOverviewTotals(
                        totalTokens: 13,
                        costUsdNanos: 21,
                        costSource: .estimated,
                        toolCalls: 3
                    )
                )
            ],
            modelBreakdown: [
                KvasirOverviewModelSummary(
                    model: model,
                    totals: KvasirOverviewTotals(
                        totalTokens: 13,
                        costUsdNanos: 21,
                        costSource: .estimated,
                        toolCalls: 0
                    )
                )
            ],
            sessionBreakdown: [],
            sessionBreakdownMoreAvailable: 0,
            promptBreakdown: [],
            promptBreakdownMoreAvailable: 0,
            selectedRepo: repo,
            selectedModel: model
        )
    )
    let expectedRepo = OverviewRepoBucket.repo(
        OverviewRepoIdentity(
            name: OverviewRepoName("kvasir"),
            path: OverviewRepoPath("/repos/kvasir")
        )!
    )

    #expect(mapped == OverviewSnapshot(
        totals: OverviewTotals(
            totalTokens: 13,
            costUsdNanos: 21,
            costSource: .estimated,
            toolCalls: 3
        ),
        series: [
            OverviewSeriesPoint(
                day: OverviewRollupDay(year: 2026, month: 6, day: 24),
                totalTokens: 13,
                costUsdNanos: 21,
                costSource: .estimated,
                toolCalls: 3
            )
        ],
        repoBreakdown: [
            OverviewRepoSummary(
                repo: expectedRepo,
                totals: OverviewTotals(
                    totalTokens: 13,
                    costUsdNanos: 21,
                    costSource: .estimated,
                    toolCalls: 3
                )
            )
        ],
        modelBreakdown: [
            OverviewModelSummary(
                model: OverviewModelName(model),
                totals: OverviewTotals(
                    totalTokens: 13,
                    costUsdNanos: 21,
                    costSource: .estimated,
                    toolCalls: 0
                )
            )
        ],
        selectedRepo: expectedRepo,
        selectedModel: OverviewModelName(model)
    ))
}

@Test
func kvasirOverviewSnapshotMappingPreservesCostSourceVariants() {
    let nativeRepo = KvasirRepoBucket(kind: .repo, name: "native", path: "/repos/native")
    let mixedRepo = KvasirRepoBucket(kind: .repo, name: "mixed", path: "/repos/mixed")
    let mapped = overviewSnapshotFromKvasir(
        KvasirOverviewSnapshot(
            totals: KvasirOverviewTotals(
                totalTokens: 30,
                costUsdNanos: 9_000,
                costSource: .mixed,
                toolCalls: 1
            ),
            series: [
                KvasirOverviewSeriesPoint(
                    day: KvasirRollupDay(year: 2026, month: 6, day: 20),
                    totalTokens: 10,
                    costUsdNanos: 2_000,
                    costSource: .native,
                    toolCalls: 0
                ),
                KvasirOverviewSeriesPoint(
                    day: KvasirRollupDay(year: 2026, month: 6, day: 21),
                    totalTokens: 20,
                    costUsdNanos: 7_000,
                    costSource: .mixed,
                    toolCalls: 1
                ),
                KvasirOverviewSeriesPoint(
                    day: KvasirRollupDay(year: 2026, month: 6, day: 22),
                    totalTokens: 0,
                    costUsdNanos: 0,
                    costSource: nil,
                    toolCalls: 0
                ),
            ],
            repoBreakdown: [
                KvasirOverviewRepoSummary(
                    repo: nativeRepo,
                    totals: KvasirOverviewTotals(
                        totalTokens: 10,
                        costUsdNanos: 2_000,
                        costSource: .native,
                        toolCalls: 0
                    )
                ),
                KvasirOverviewRepoSummary(
                    repo: mixedRepo,
                    totals: KvasirOverviewTotals(
                        totalTokens: 20,
                        costUsdNanos: 7_000,
                        costSource: .mixed,
                        toolCalls: 1
                    )
                ),
            ],
            modelBreakdown: [
                KvasirOverviewModelSummary(
                    model: "claude-opus-4",
                    totals: KvasirOverviewTotals(
                        totalTokens: 10,
                        costUsdNanos: 2_000,
                        costSource: .native,
                        toolCalls: 0
                    )
                ),
                KvasirOverviewModelSummary(
                    model: "claude-sonnet-4",
                    totals: KvasirOverviewTotals(
                        totalTokens: 20,
                        costUsdNanos: 7_000,
                        costSource: nil,
                        toolCalls: 0
                    )
                ),
            ],
            sessionBreakdown: [],
            sessionBreakdownMoreAvailable: 0,
            promptBreakdown: [],
            promptBreakdownMoreAvailable: 0,
            selectedRepo: mixedRepo,
            selectedModel: "claude-sonnet-4"
        )
    )

    #expect(mapped.totals.costSource == .mixed)
    #expect(mapped.series.map(\.costSource) == [.native, .mixed, nil])
    #expect(mapped.repoBreakdown.map(\.totals.costSource) == [.native, .mixed])
    #expect(mapped.modelBreakdown.map(\.totals.costSource) == [.native, nil])
    #expect(mapped.selectedRepo?.displayName == "mixed")
    #expect(mapped.selectedModel == OverviewModelName("claude-sonnet-4"))
}

@Test
func kvasirOverviewSnapshotMappingNormalizesInvalidRepoBuckets() {
    let invalidRepo = KvasirRepoBucket(kind: .repo, name: nil, path: nil)
    let mapped = overviewSnapshotFromKvasir(
        KvasirOverviewSnapshot(
            totals: KvasirOverviewTotals(totalTokens: 1, costUsdNanos: 2, costSource: nil, toolCalls: 3),
            series: [],
            repoBreakdown: [
                KvasirOverviewRepoSummary(
                    repo: invalidRepo,
                    totals: KvasirOverviewTotals(totalTokens: 1, costUsdNanos: 2, costSource: nil, toolCalls: 3)
                )
            ],
            modelBreakdown: [],
            sessionBreakdown: [],
            sessionBreakdownMoreAvailable: 0,
            promptBreakdown: [],
            promptBreakdownMoreAvailable: 0,
            selectedRepo: invalidRepo,
            selectedModel: nil
        )
    )

    #expect(mapped.selectedRepo == .noRepo)
    #expect(mapped.repoBreakdown == [
        OverviewRepoSummary(
            repo: .noRepo,
            totals: OverviewTotals(totalTokens: 1, costUsdNanos: 2, toolCalls: 3)
        )
    ])
}
#endif

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
        config.opencodeConfigPath == home.appendingPathComponent(".config", isDirectory: true)
            .appendingPathComponent("opencode", isDirectory: true)
            .appendingPathComponent("opencode.json").path
    )
    #expect(
        config.opencodeEnvPath == home.appendingPathComponent(".config", isDirectory: true)
            .appendingPathComponent("opencode", isDirectory: true)
            .appendingPathComponent("kvasir.env").path
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

@MainActor
@Test
func viewerStartupShowsFriendlyHarnessTelemetryWarningAndContinues() async throws {
    let rawError = RawHarnessTelemetrySetupError()
    let warning = HarnessTelemetrySetupWarning(reason: .invalidClaudeSettings)
    let primary = SequenceOverviewClient(results: [
        .success(overviewSnapshot(totalTokens: 12))
    ])
    let model = KvasirViewerModel(
        dashboard: OverviewDashboard(client: primary),
        telemetrySetup: ConfiguringHarnessTelemetrySetup(
            config: ProductionModelFactory.resolvedHarnessTelemetrySetupConfig(environment: [:]),
            configure: { _ in throw rawError },
            warningForError: { error in
                error.localizedDescription == rawError.localizedDescription ? warning : nil
            }
        ),
        launchAgent: DaemonLaunchAgent(registry: RecordingLaunchAgentRegistry(status: .enabled))
    )

    try await model.start()

    #expect(model.setupWarningMessage == warning.localizedDescription)
    #expect(model.setupWarningMessage != rawError.localizedDescription)
    #expect(await primary.loadCount == 1)
    #expect(model.overviewSnapshot?.totals.totalTokens == 12)
}

@Test
func daemonFallbackOverviewClientStartsDaemonAndRetriesRecoverableFailure() async throws {
    let expected = overviewSnapshot(totalTokens: 6)
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

    let snapshot = try await client.loadOverviewSnapshot(
        query: .init(start: Date(timeIntervalSince1970: 0), end: Date(timeIntervalSince1970: 1))
    )

    #expect(snapshot == expected)
    #expect(await primary.loadCount == 2)
    #expect(starter.startCount == 1)
}

@Test
func daemonFallbackOverviewClientRetriesRecoverableFailuresWhileSpawnedDaemonBecomesReady() async throws {
    let expected = overviewSnapshot(totalTokens: 26)
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

    let snapshot = try await client.loadOverviewSnapshot(
        query: .init(start: Date(timeIntervalSince1970: 0), end: Date(timeIntervalSince1970: 1))
    )

    #expect(snapshot == expected)
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
        _ = try await client.loadOverviewSnapshot(
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
            overviewSnapshot(totalTokens: 31)
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
func productionFactoryOpensDaemonFallbackAfterSuccessfulStartupForLaterRefreshFailures() async throws {
    let primary = SequenceOverviewClient(results: [
        .success(
            overviewSnapshot(totalTokens: 3)
        ),
        .failure(DaemonFallbackTestError.recoverable),
        .success(
            overviewSnapshot(totalTokens: 68)
        ),
    ])
    let starter = RecordingDaemonProcessStarter()
    let model = ProductionModelFactory.make(
        overviewClient: primary,
        daemonStarter: starter,
        launchAgent: DaemonLaunchAgent(registry: RecordingLaunchAgentRegistry(status: .enabled)),
        shouldStartBundledDaemonAfterOverviewError: { error in
            error as? DaemonFallbackTestError == .recoverable
        }
    )

    try await model.start()
    #expect(model.overviewSnapshot?.totals.totalTokens == 3)

    try await model.refreshOverview()

    #expect(await primary.loadCount == 3)
    #expect(starter.startCount == 1)
    #expect(model.overviewSnapshot?.totals.totalTokens == 68)
}

@MainActor
@Test
func productionFactoryWiresLiveOverviewUpdateSourceIntoModel() async throws {
    let primary = SequenceOverviewClient(results: [
        .success(overviewSnapshot(totalTokens: 3)),
        .success(overviewSnapshot(totalTokens: 68)),
    ])
    let updateSource = ManualOverviewUpdateSource()
    let model = ProductionModelFactory.make(
        overviewClient: primary,
        overviewUpdateSource: updateSource,
        launchAgent: DaemonLaunchAgent(registry: RecordingLaunchAgentRegistry(status: .enabled))
    )

    try await model.start()
    #expect(model.overviewSnapshot?.totals.totalTokens == 3)

    updateSource.send()
    await waitForLoadCount(primary, 2)
    await waitUntil(model.overviewSnapshot?.totals.totalTokens == 68)

    #expect(model.overviewSnapshot?.totals.totalTokens == 68)
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

@Test
func daemonFallbackOverviewClientStopsRetryingAfterBoundedRecoverableFailures() async throws {
    let primary = SequenceOverviewClient(results: [
        .failure(DaemonFallbackTestError.recoverable),
        .failure(DaemonFallbackTestError.recoverable),
        .failure(DaemonFallbackTestError.recoverable),
        .failure(DaemonFallbackTestError.recoverable),
    ])
    let starter = RecordingDaemonProcessStarter()
    let retryDelay = RecordingRetryDelay()
    let client = DaemonFallbackOverviewClient(
        primary: primary,
        starter: starter,
        shouldStartDaemonAfterError: { error in
            error as? DaemonFallbackTestError == .recoverable
        },
        maximumRetryCount: 2,
        retryDelay: retryDelay.sleep
    )

    do {
        _ = try await client.loadOverviewSnapshot(
            query: .init(start: Date(timeIntervalSince1970: 0), end: Date(timeIntervalSince1970: 1))
        )
        Issue.record("expected bounded recoverable failure")
    } catch {
        #expect(error as? DaemonFallbackTestError == .recoverable)
    }

    #expect(await primary.loadCount == 4)
    #expect(starter.startCount == 1)
    #expect(await retryDelay.attempts == [1, 2])
}

#if canImport(kvasir_client)
@MainActor
@Test
func productionFactoryRefreshesLaunchAgentForRpcSerializationDuringStartup() async throws {
    let primary = SequenceOverviewClient(results: [
        .failure(KvasirClientError.RpcSerialization),
        .success(
            overviewSnapshot(totalTokens: 15)
        ),
    ])
    let model = ProductionModelFactory.make(
        overviewClient: primary,
        launchAgent: DaemonLaunchAgent(registry: RecordingLaunchAgentRegistry(status: .enabled))
    )

    try await model.start()

    #expect(await primary.loadCount == 2)
    #expect(model.overviewSnapshot?.totals.totalTokens == 15)
}

@MainActor
@Test
func productionFactoryStartsBundledDaemonForSocketIoAfterStartupGateOpens() async throws {
    let primary = SequenceOverviewClient(results: [
        .failure(KvasirClientError.SocketIo),
        .success(
            overviewSnapshot(totalTokens: 16)
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
func productionFactoryStartsBundledDaemonForRpcSerializationAfterStartupGateOpens() async throws {
    let primary = SequenceOverviewClient(results: [
        .failure(KvasirClientError.RpcSerialization),
        .success(
            overviewSnapshot(totalTokens: 17)
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
    #expect(model.overviewSnapshot?.totals.totalTokens == 17)
}

@MainActor
@Test
func productionFactoryDoesNotStartBundledDaemonForNonrecoverableClientError() async throws {
    let primary = SequenceOverviewClient(results: [
        .failure(KvasirClientError.DaemonError),
    ])
    let starter = RecordingDaemonProcessStarter()
    let model = ProductionModelFactory.make(
        overviewClient: primary,
        daemonStarter: starter,
        daemonFallbackGate: DaemonFallbackGate(enabled: true)
    )

    do {
        try await model.refreshOverview()
        Issue.record("expected nonrecoverable client error")
    } catch {
        #expect(error as? KvasirClientError == .DaemonError)
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
    private var results: [Result<OverviewSnapshot, any Error>]
    private(set) var loadCount = 0

    init(results: [Result<OverviewSnapshot, any Error>]) {
        self.results = results
    }

    func loadOverviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot {
        loadCount += 1
        guard !results.isEmpty else {
            throw DaemonFallbackTestError.nonrecoverable
        }
        return try results.removeFirst().get()
    }
}

private actor RecordingTraceInspectorClient: TraceInspectorClient {
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

private struct RawHarnessTelemetrySetupError: LocalizedError {
    var errorDescription: String? {
        "Kvasir_client.KvasirClientError:harnessTelemetrySetup RPC serialization error"
    }
}

private func overviewSnapshot(
    totalTokens: UInt64 = 0,
    selectedSession: OverviewSessionRoute? = nil,
    selectedPrompt: OverviewPromptRoute? = nil
) -> OverviewSnapshot {
    OverviewSnapshot(
        totals: OverviewTotals(totalTokens: totalTokens, costUsdNanos: 0, toolCalls: 0),
        series: [],
        repoBreakdown: [],
        selectedRepo: nil,
        selectedSession: selectedSession,
        selectedPrompt: selectedPrompt
    )
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

private final class RecordingLaunchAgentRegistry: LaunchAgentRegistry {
    private let launchAgentStatus: LaunchAgentStatus

    init(status: LaunchAgentStatus) {
        self.launchAgentStatus = status
    }

    func status(plistName: String) -> LaunchAgentStatus {
        launchAgentStatus
    }

    func register(plistName: String) throws {}

    func unregister(plistName: String) throws {}
}

private actor RecordingRetryDelay {
    private(set) var attempts: [Int] = []

    func sleep(attempt: Int) async {
        attempts.append(attempt)
    }
}

@MainActor
private func waitForLoadCount(
    _ client: SequenceOverviewClient,
    _ count: Int,
    sourceLocation: SourceLocation = #_sourceLocation
) async {
    for _ in 0..<1_000 {
        if await client.loadCount >= count {
            return
        }
        await Task.yield()
    }
    Issue.record("load count was not reached", sourceLocation: sourceLocation)
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
