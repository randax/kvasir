#if canImport(kvasir_client)
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
        model: query.model?.displayName()
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

private extension KvasirOverviewSnapshot {
    var overviewSnapshot: OverviewSnapshot {
        OverviewSnapshot(
            totals: totals.overviewTotals,
            series: series.map { $0.overviewSeriesPoint },
            repoBreakdown: repoBreakdown.map { $0.overviewRepoSummary },
            modelBreakdown: modelBreakdown.map { $0.overviewModelSummary },
            selectedRepo: selectedRepo?.overviewRepo,
            selectedModel: selectedModel.map(OverviewModelName.init)
        )
    }
}

private extension KvasirOverviewTotals {
    var overviewTotals: OverviewTotals {
        OverviewTotals(
            totalTokens: totalTokens,
            costUsdNanos: costUsdNanos,
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

private extension KvasirRollupDay {
    var overviewDay: OverviewRollupDay {
        OverviewRollupDay(year: Int(year), month: Int(month), day: Int(day))
    }
}
#endif
