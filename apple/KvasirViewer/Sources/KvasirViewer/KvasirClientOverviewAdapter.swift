#if canImport(kvasir_client)
import Foundation
import kvasir_client
import KvasirViewerCore

struct KvasirClientRollupSource: OverviewRollupSource, TraceInspectorSource {
    let socketPath: String
    let setupConfig: HarnessTelemetrySetupConfig

    func overviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot {
        try await Task.detached(priority: .userInitiated) { [self] in
            let client = try KvasirClient.connect(socketPath: socketPath)
            return try overviewSnapshotFromKvasir(
                client.overviewSnapshot(query: kvasirRollupQuery(from: query))
            )
        }.value
    }

    func traceInspectorSnapshot(query: TraceInspectorQuery) async throws -> TraceInspectorSnapshot {
        try await Task.detached(priority: .userInitiated) { [self] in
            let client = try KvasirClient.connect(socketPath: socketPath)
            let traces = try client.trace(query: kvasirTraceQuery(from: query))
            let replay = try loadKvasirContentReplay(
                socketPath: socketPath,
                config: kvasirHarnessTelemetrySetup(from: setupConfig),
                query: kvasirContentReplayQuery(from: query)
            )
            return traceInspectorSnapshotFromKvasir(
                prompt: query.prompt,
                traces: traces,
                replay: replay
            )
        }.value
    }
}

struct KvasirClientUsageUpdateSource: OverviewUpdateSource {
    let socketPath: String

    func overviewRefreshEvents() -> AsyncStream<Void> {
        AsyncStream { continuation in
            let subscriptionBox = KvasirOverviewRefreshSubscriptionBox()
            let task = Task.detached(priority: .background) {
                do {
                    let subscription = try KvasirOverviewRefreshSubscription.connect(socketPath: socketPath)
                    subscriptionBox.replace(with: subscription)
                    while !Task.isCancelled {
                        try subscription.next()
                        continuation.yield(())
                    }
                } catch {
                    subscriptionBox.clear()
                }
                continuation.finish()
            }
            continuation.onTermination = { _ in
                task.cancel()
                subscriptionBox.close()
            }
        }
    }
}

private final class KvasirOverviewRefreshSubscriptionBox: @unchecked Sendable {
    private let lock = NSLock()
    private var subscription: KvasirOverviewRefreshSubscription?

    func replace(with subscription: KvasirOverviewRefreshSubscription) {
        lock.withLock {
            self.subscription = subscription
        }
    }

    func clear() {
        lock.withLock {
            subscription = nil
        }
    }

    func close() {
        let subscription = lock.withLock {
            let subscription = self.subscription
            self.subscription = nil
            return subscription
        }
        try? subscription?.close()
    }
}

func kvasirRollupQuery(from query: OverviewQuery) -> KvasirRollupQuery {
    KvasirRollupQuery(
        start: KvasirTimestampMillis(value: Int64(query.start.timeIntervalSince1970 * 1_000)),
        end: KvasirTimestampMillis(value: Int64(query.end.timeIntervalSince1970 * 1_000)),
        repo: query.repo?.kvasirRepoBucket,
        harness: query.harness?.displayName(),
        model: query.model?.displayName(),
        session: query.session?.kvasirOverviewSessionRoute,
        prompt: query.prompt?.kvasirOverviewPromptRoute
    )
}

func overviewSnapshotFromKvasir(_ snapshot: KvasirOverviewSnapshot) -> OverviewSnapshot {
    snapshot.overviewSnapshot
}

func traceInspectorSnapshotFromKvasir(
    prompt: OverviewPromptRoute,
    traces: [KvasirTrace],
    replay: KvasirContentReplay
) -> TraceInspectorSnapshot {
    TraceInspectorSnapshot(
        prompt: prompt,
        traces: traces.map(\.traceInspectorTrace),
        content: replay.items.map(\.traceInspectorContentItem),
        contentAvailability: replay.availability.traceInspectorContentAvailability
    )
}

func kvasirTraceQuery(from query: TraceInspectorQuery) -> KvasirTraceQuery {
    KvasirTraceQuery(
        harness: query.prompt.session.harness.displayName(),
        sessionId: query.prompt.session.sessionID.displayName(),
        promptId: query.prompt.promptID.displayName()
    )
}

func kvasirContentReplayQuery(from query: TraceInspectorQuery) -> KvasirContentReplayQuery {
    KvasirContentReplayQuery(
        harness: query.prompt.session.harness.displayName(),
        sessionId: query.prompt.session.sessionID.displayName(),
        promptId: query.prompt.promptID.displayName()
    )
}

func kvasirHarnessTelemetrySetup(from config: HarnessTelemetrySetupConfig) -> KvasirHarnessTelemetrySetup {
    KvasirHarnessTelemetrySetup(
        codexConfigPath: config.codexConfigPath,
        claudeSettingsPath: config.claudeSettingsPath,
        copilotProfilePath: config.copilotProfilePath,
        opencodeConfigPath: config.opencodeConfigPath,
        opencodeEnvPath: config.opencodeEnvPath,
        zshProfilePath: config.zshProfilePath,
        bashProfilePath: config.bashProfilePath,
        zshRepoHookPath: config.zshRepoHookPath,
        bashRepoHookPath: config.bashRepoHookPath,
        rawBodyDirectory: config.rawBodyDirectory,
        otlpEndpoint: config.otlpEndpoint
    )
}

private extension OverviewRepoBucket {
    var kvasirRepoBucket: KvasirRepoBucket? {
        switch self {
        case .noRepo:
            return KvasirRepoBucket(kind: .noRepo, name: nil, path: nil)
        case .repo(let identity):
            guard identity.name != nil || identity.path != nil else {
                return nil
            }
            return KvasirRepoBucket(
                kind: .repo,
                name: identity.name?.rawValue,
                path: identity.path?.rawValue
            )
        }
    }
}

private extension OverviewSessionRoute {
    var kvasirOverviewSessionRoute: KvasirOverviewSessionRoute {
        KvasirOverviewSessionRoute(
            harness: harness.displayName(),
            sessionId: sessionID.displayName()
        )
    }
}

private extension OverviewPromptRoute {
    var kvasirOverviewPromptRoute: KvasirOverviewPromptRoute {
        KvasirOverviewPromptRoute(
            session: session.kvasirOverviewSessionRoute,
            promptId: promptID.displayName()
        )
    }
}

private extension KvasirRepoBucket {
    var overviewRepo: OverviewRepoBucket {
        switch kind {
        case .noRepo:
            return .noRepo
        case .repo:
            guard let identity = OverviewRepoIdentity(
                name: name.map(OverviewRepoName.init),
                path: path.map(OverviewRepoPath.init)
            ) else {
                return .noRepo
            }
            return .repo(identity)
        }
    }
}

private extension KvasirOverviewSessionRoute {
    var overviewSessionRoute: OverviewSessionRoute {
        OverviewSessionRoute(
            harness: OverviewHarnessName(harness),
            sessionID: OverviewSessionID(sessionId)
        )
    }
}

private extension KvasirOverviewPromptRoute {
    var overviewPromptRoute: OverviewPromptRoute {
        OverviewPromptRoute(
            session: session.overviewSessionRoute,
            promptID: OverviewPromptID(promptId)
        )
    }
}

private extension KvasirOverviewSnapshot {
    var overviewSnapshot: OverviewSnapshot {
        OverviewSnapshot(
            totals: totals.overviewTotals,
            series: series.map { $0.overviewSeriesPoint },
            repoBreakdown: repoBreakdown.map { $0.overviewRepoSummary },
            modelBreakdown: modelBreakdown.map { $0.overviewModelSummary },
            harnessBreakdown: harnessBreakdown.map { $0.overviewHarnessSummary },
            sessionBreakdown: sessionBreakdown.map { $0.overviewSessionSummary },
            sessionBreakdownMoreAvailable: sessionBreakdownMoreAvailable,
            promptBreakdown: promptBreakdown.map { $0.overviewPromptSummary },
            promptBreakdownMoreAvailable: promptBreakdownMoreAvailable,
            selectedRepo: selectedRepo?.overviewRepo,
            selectedHarness: selectedHarness.map(OverviewHarnessName.init),
            selectedModel: selectedModel.map(OverviewModelName.init),
            selectedSession: selectedSession?.overviewSessionRoute,
            selectedPrompt: selectedPrompt?.overviewPromptRoute,
            dimensions: dimensions.map { $0.overviewDimensionFilter }
        )
    }
}

private extension KvasirOverviewTotals {
    var overviewTotals: OverviewTotals {
        OverviewTotals(
            totalTokens: totalTokens,
            costUsdNanos: costUsdNanos,
            costSource: costSource?.overviewCostSource,
            toolCalls: toolCalls
        )
    }
}

private extension KvasirOverviewSeriesPoint {
    var overviewSeriesPoint: OverviewSeriesPoint {
        OverviewSeriesPoint(
            day: day.overviewDay,
            totalTokens: totalTokens,
            costUsdNanos: costUsdNanos,
            costSource: costSource?.overviewCostSource,
            toolCalls: toolCalls
        )
    }
}

private extension KvasirOverviewRepoSummary {
    var overviewRepoSummary: OverviewRepoSummary {
        OverviewRepoSummary(repo: repo.overviewRepo, totals: totals.overviewTotals)
    }
}

private extension KvasirOverviewModelSummary {
    var overviewModelSummary: OverviewModelSummary {
        OverviewModelSummary(model: OverviewModelName(model), totals: totals.overviewTotals)
    }
}

private extension KvasirOverviewHarnessSummary {
    var overviewHarnessSummary: OverviewHarnessSummary {
        OverviewHarnessSummary(
            harness: OverviewHarnessName(harness),
            totals: totals.overviewTotals,
            lastActivity: Date(timeIntervalSince1970: TimeInterval(lastActivity.value) / 1_000)
        )
    }
}

private extension KvasirOverviewSessionSummary {
    var overviewSessionSummary: OverviewSessionSummary {
        OverviewSessionSummary(
            route: route.overviewSessionRoute,
            totals: totals.overviewTotals,
            attributionStatus: attributionStatus.overviewAttributionStatus,
            lastActivity: lastActivity.overviewDate
        )
    }
}

private extension KvasirOverviewPromptSummary {
    var overviewPromptSummary: OverviewPromptSummary {
        OverviewPromptSummary(
            route: route.overviewPromptRoute,
            totals: totals.overviewTotals,
            attributionStatus: attributionStatus.overviewAttributionStatus,
            lastActivity: lastActivity.overviewDate
        )
    }
}

private extension KvasirAttributionStatus {
    var overviewAttributionStatus: OverviewAttributionStatus {
        switch self {
        case .direct:
            return .direct
        case .traceDerived:
            return .traceDerived
        case .partial:
            return .partial
        case .unavailable:
            return .unavailable
        }
    }
}

private extension KvasirOverviewDimensionFilter {
    var overviewDimensionFilter: OverviewDimensionFilter {
        OverviewDimensionFilter(
            kind: kind.overviewDimensionKind,
            value: OverviewDimensionValue(value)
        )
    }
}

private extension KvasirOverviewDimensionKind {
    var overviewDimensionKind: OverviewDimensionKind {
        switch self {
        case .subagent:
            return .subagent
        case .skill:
            return .skill
        case .plugin:
            return .plugin
        case .mcpServer:
            return .mcpServer
        case .mcpTool:
            return .mcpTool
        case .effort:
            return .effort
        case .speed:
            return .speed
        case .querySource:
            return .querySource
        case .accountOrg:
            return .accountOrg
        }
    }
}

private extension KvasirRollupDay {
    var overviewDay: OverviewRollupDay {
        OverviewRollupDay(year: Int(year), month: Int(month), day: Int(day))
    }
}

private extension KvasirTimestampMillis {
    var overviewDate: Date {
        Date(timeIntervalSince1970: Double(value) / 1_000)
    }
}

private extension KvasirCostSource {
    var overviewCostSource: OverviewCostSource {
        switch self {
        case .native:
            return .native
        case .estimated:
            return .estimated
        case .mixed:
            return .mixed
        }
    }
}

private extension KvasirTrace {
    var traceInspectorTrace: TraceInspectorTrace {
        TraceInspectorTrace(
            traceID: TraceInspectorTraceID(traceId),
            spans: spans.map(\.traceInspectorSpan),
            durations: durations.traceInspectorDurations
        )
    }
}

private extension KvasirTraceSpan {
    var traceInspectorSpan: TraceInspectorSpan {
        TraceInspectorSpan(
            spanID: TraceInspectorSpanID(spanId),
            parentSpanID: parentSpanId.map(TraceInspectorSpanID.init),
            kind: kind.traceInspectorSpanKind,
            name: TraceInspectorSpanName(name),
            startedAt: startedAt.overviewDate,
            endedAt: endedAt.overviewDate,
            durationMilliseconds: durationMs,
            toolName: toolName.map(TraceInspectorToolName.init)
        )
    }
}

private extension KvasirTraceSpanKind {
    var traceInspectorSpanKind: TraceInspectorSpanKind {
        switch self {
        case .interaction:
            return .interaction
        case .llmRequest:
            return .llmRequest
        case .toolCall:
            return .toolCall
        }
    }
}

private extension KvasirTraceDurationMeasures {
    var traceInspectorDurations: TraceInspectorDurations {
        TraceInspectorDurations(
            timeToFirstTokenMilliseconds: ttftMs,
            requestMilliseconds: requestMs,
            toolMilliseconds: toolMs
        )
    }
}

private extension KvasirContentReplayItem {
    var traceInspectorContentItem: TraceInspectorContentItem {
        TraceInspectorContentItem(
            occurredAt: occurredAt.overviewDate,
            harness: OverviewHarnessName(harness),
            kind: kind.traceInspectorContentKind,
            content: TraceInspectorContentText(content)
        )
    }
}

private extension KvasirContentKind {
    var traceInspectorContentKind: TraceInspectorContentKind {
        switch self {
        case .userPrompt:
            return .userPrompt
        case .assistantMessage:
            return .assistantMessage
        case .toolInput:
            return .toolInput
        case .toolOutput:
            return .toolOutput
        case .rawApiRequest:
            return .rawApiRequest
        case .rawApiResponse:
            return .rawApiResponse
        }
    }
}

private extension KvasirContentAvailability {
    var traceInspectorContentAvailability: TraceInspectorContentAvailability {
        switch self {
        case .captured(let harness, let kinds):
            return .captured(
                harness: OverviewHarnessName(harness),
                kinds: kinds.map(\.traceInspectorContentKindAvailability)
            )
        case .unavailable(let reason):
            return .unavailable(reason: reason.traceInspectorContentUnavailableReason)
        }
    }
}

private extension KvasirContentKindAvailability {
    var traceInspectorContentKindAvailability: TraceInspectorContentKindAvailability {
        switch self {
        case .captured(let kind):
            return .captured(kind.traceInspectorContentKind)
        case .unavailable(let kind, let reason):
            return .unavailable(
                kind: kind.traceInspectorContentKind,
                reason: reason.traceInspectorContentUnavailableReason
            )
        }
    }
}

private extension KvasirContentUnavailableReason {
    var traceInspectorContentUnavailableReason: TraceInspectorContentUnavailableReason {
        switch self {
        case .notProvidedByHarness:
            return .notProvidedByHarness
        case .notCapturedForPrompt:
            return .notCapturedForPrompt
        case .promptNotFound:
            return .promptNotFound
        }
    }
}
#endif
