#if canImport(kvasir_client)
import Foundation
import kvasir_client
import KvasirViewerCore

struct KvasirClientRollupSource: OverviewRollupSource {
    let socketPath: String

    func overviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot {
        try await Task.detached(priority: .userInitiated) { [self] in
            let client = try KvasirClient.connect(socketPath: socketPath)
            return try overviewSnapshotFromKvasir(
                client.overviewSnapshot(query: kvasirRollupQuery(from: query))
            )
        }.value
    }
}

func kvasirRollupQuery(from query: OverviewQuery) -> KvasirRollupQuery {
    KvasirRollupQuery(
        start: KvasirTimestampMillis(value: Int64(query.start.timeIntervalSince1970 * 1_000)),
        end: KvasirTimestampMillis(value: Int64(query.end.timeIntervalSince1970 * 1_000)),
        repo: query.repo?.kvasirRepoBucket,
        model: query.model?.displayName(),
        session: query.session?.kvasirOverviewSessionRoute,
        prompt: query.prompt?.kvasirOverviewPromptRoute
    )
}

func overviewSnapshotFromKvasir(_ snapshot: KvasirOverviewSnapshot) -> OverviewSnapshot {
    snapshot.overviewSnapshot
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
            sessionBreakdown: sessionBreakdown.map { $0.overviewSessionSummary },
            sessionBreakdownMoreAvailable: sessionBreakdownMoreAvailable,
            promptBreakdown: promptBreakdown.map { $0.overviewPromptSummary },
            promptBreakdownMoreAvailable: promptBreakdownMoreAvailable,
            selectedRepo: selectedRepo?.overviewRepo,
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
#endif
