import Foundation
import Testing

@testable import KvasirViewerCore

@MainActor
@Test
func overviewSocketClientLoadsAllRollupMeasuresForTheSameQuery() async throws {
    let query = OverviewQuery(
        start: Date(timeIntervalSince1970: 1_782_000_000),
        end: Date(timeIntervalSince1970: 1_782_259_200)
    )
    let source = RecordingOverviewRollupSource(
        tokenRollups: [
            .init(day: .init(year: 2026, month: 6, day: 21), inputTokens: 1, outputTokens: 2, cacheTokens: 3)
        ],
        costRollups: [
            .init(day: .init(year: 2026, month: 6, day: 21), costUsdNanos: 4)
        ],
        toolCallRollups: [
            .init(day: .init(year: 2026, month: 6, day: 21), callCount: 5)
        ]
    )
    let client = OverviewSocketClient(source: source)

    let rollups = try await client.loadOverviewRollups(query: query)

    #expect(source.queries == [query])
    #expect(rollups == OverviewRollups(
        tokenRollups: source.tokenRollups,
        costRollups: source.costRollups,
        toolCallRollups: source.toolCallRollups
    ))
}

@MainActor
@Test
func overviewSocketClientDefersSourceConnectionUntilLoad() async throws {
    let source = DeferredOverviewRollupSource {
        OverviewRollups(
            tokenRollups: [
                .init(day: .init(year: 2026, month: 6, day: 21), inputTokens: 1, outputTokens: 1, cacheTokens: 0)
            ],
            costRollups: [],
            toolCallRollups: []
        )
    }
    let client = OverviewSocketClient(source: source)

    #expect(source.loadCount == 0)

    _ = try await client.loadOverviewRollups(
        query: OverviewQuery(
            start: Date(timeIntervalSince1970: 1_782_000_000),
            end: Date(timeIntervalSince1970: 1_782_259_200)
        )
    )

    #expect(source.loadCount == 1)
}

private final class RecordingOverviewRollupSource: OverviewRollupSource, @unchecked Sendable {
    let tokenRollups: [OverviewTokenRollup]
    let costRollups: [OverviewCostRollup]
    let toolCallRollups: [OverviewToolCallRollup]
    private let lock = NSLock()
    private var recordedQueries: [OverviewQuery] = []

    init(
        tokenRollups: [OverviewTokenRollup],
        costRollups: [OverviewCostRollup],
        toolCallRollups: [OverviewToolCallRollup]
    ) {
        self.tokenRollups = tokenRollups
        self.costRollups = costRollups
        self.toolCallRollups = toolCallRollups
    }

    func overviewRollups(query: OverviewQuery) async throws -> OverviewRollups {
        lock.withLock {
            recordedQueries.append(query)
        }
        return OverviewRollups(
            tokenRollups: tokenRollups,
            costRollups: costRollups,
            toolCallRollups: toolCallRollups
        )
    }

    var queries: [OverviewQuery] {
        lock.withLock { recordedQueries }
    }
}

private final class DeferredOverviewRollupSource: OverviewRollupSource, @unchecked Sendable {
    private let lock = NSLock()
    private let load: () -> OverviewRollups
    private var recordedLoadCount = 0

    init(load: @escaping () -> OverviewRollups) {
        self.load = load
    }

    func overviewRollups(query: OverviewQuery) async throws -> OverviewRollups {
        lock.withLock {
            recordedLoadCount += 1
        }
        return load()
    }

    var loadCount: Int {
        lock.withLock { recordedLoadCount }
    }
}
