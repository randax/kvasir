#if canImport(kvasir_client)
import kvasir_client
import KvasirViewerCore

struct KvasirClientRollupSource: OverviewRollupSource {
    let socketPath: String

    func overviewRollups(query: OverviewQuery) async throws -> OverviewRollups {
        try await Task.detached(priority: .userInitiated) { [self] in
            let client = try KvasirClient.connect(socketPath: socketPath)
            let rollups = try client.overviewRollups(query: query.kvasirRollupQuery)
            return OverviewRollups(
                tokenRollups: rollups.tokenRollups.map { rollup in
                    OverviewTokenRollup(
                        day: rollup.day.overviewDay,
                        repo: rollup.repo.overviewRepo,
                        inputTokens: rollup.inputTokens,
                        outputTokens: rollup.outputTokens,
                        cacheTokens: rollup.cacheTokens
                    )
                },
                costRollups: rollups.costRollups.map { rollup in
                    OverviewCostRollup(
                        day: rollup.day.overviewDay,
                        repo: rollup.repo.overviewRepo,
                        costUsdNanos: rollup.costUsd.nanos
                    )
                },
                toolCallRollups: rollups.toolCallRollups.map { rollup in
                    OverviewToolCallRollup(
                        day: rollup.day.overviewDay,
                        repo: rollup.repo.overviewRepo,
                        callCount: rollup.callCount
                    )
                }
            )
        }.value
    }
}

private extension OverviewQuery {
    var kvasirRollupQuery: KvasirRollupQuery {
        KvasirRollupQuery(
            start: KvasirTimestampMillis(value: Int64(start.timeIntervalSince1970 * 1_000)),
            end: KvasirTimestampMillis(value: Int64(end.timeIntervalSince1970 * 1_000)),
            repo: repo?.kvasirRepoBucket
        )
    }
}

private extension OverviewRepoBucket {
    var kvasirRepoBucket: KvasirRepoBucket {
        switch self {
        case .noRepo:
            return KvasirRepoBucket(kind: .noRepo, name: nil, path: nil)
        case .repo(let identity):
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
            return .repo(
                OverviewRepoIdentity(
                    name: name.map(OverviewRepoName.init),
                    path: path.map(OverviewRepoPath.init)
                )
            )
        }
    }
}

private extension KvasirRollupDay {
    var overviewDay: OverviewRollupDay {
        OverviewRollupDay(year: Int(year), month: Int(month), day: Int(day))
    }
}
#endif
