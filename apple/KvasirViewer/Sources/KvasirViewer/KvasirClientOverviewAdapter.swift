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
                        inputTokens: rollup.inputTokens,
                        outputTokens: rollup.outputTokens,
                        cacheTokens: rollup.cacheTokens
                    )
                },
                costRollups: rollups.costRollups.map { rollup in
                    OverviewCostRollup(
                        day: rollup.day.overviewDay,
                        costUsdNanos: rollup.costUsd.nanos
                    )
                },
                toolCallRollups: rollups.toolCallRollups.map { rollup in
                    OverviewToolCallRollup(
                        day: rollup.day.overviewDay,
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
            repo: nil
        )
    }
}

private extension KvasirRollupDay {
    var overviewDay: OverviewRollupDay {
        OverviewRollupDay(year: Int(year), month: Int(month), day: Int(day))
    }
}
#endif
