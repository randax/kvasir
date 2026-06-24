import Foundation
import Testing

@testable import KvasirViewerCore

@Test
@MainActor
func overviewLoadsTotalsAndChartSeriesForSelectedRange() async throws {
    let client = RecordingOverviewClient(
        tokenRollups: [
            .init(day: .init(year: 2026, month: 6, day: 20), inputTokens: 1_000, outputTokens: 500, cacheTokens: 250),
            .init(day: .init(year: 2026, month: 6, day: 20), inputTokens: 300, outputTokens: 100, cacheTokens: 0),
            .init(day: .init(year: 2026, month: 6, day: 21), inputTokens: 2_000, outputTokens: 800, cacheTokens: 100)
        ],
        costRollups: [
            .init(day: .init(year: 2026, month: 6, day: 20), costUsdNanos: 1_250_000_000),
            .init(day: .init(year: 2026, month: 6, day: 21), costUsdNanos: 2_000_000_000)
        ],
        toolCallRollups: [
            .init(day: .init(year: 2026, month: 6, day: 20), callCount: 4),
            .init(day: .init(year: 2026, month: 6, day: 21), callCount: 6)
        ]
    )
    let range = OverviewTimeRange(
        start: Date(timeIntervalSince1970: 1_782_000_000),
        end: Date(timeIntervalSince1970: 1_782_259_200)
    )
    let dashboard = OverviewDashboard(client: client)

    let snapshot = try await dashboard.load(range: range)

    #expect(client.queries == [.init(start: range.start, end: range.end)])
    #expect(snapshot.totals == .init(totalTokens: 5_050, costUsdNanos: 3_250_000_000, toolCalls: 10))
    #expect(snapshot.series == [
        .init(day: .init(year: 2026, month: 6, day: 20), totalTokens: 2_150, costUsdNanos: 1_250_000_000, toolCalls: 4),
        .init(day: .init(year: 2026, month: 6, day: 21), totalTokens: 2_900, costUsdNanos: 2_000_000_000, toolCalls: 6)
    ])
}

@Test
@MainActor
func overviewBreaksMeasuresDownByRepoAndCanLoadAScopedRepo() async throws {
    let kvasirRepo = OverviewRepoBucket.repo(
        OverviewRepoIdentity(
            name: OverviewRepoName("kvasir"),
            path: OverviewRepoPath("/repos/kvasir")
        )
    )
    let noRepo = OverviewRepoBucket.noRepo
    let client = RecordingOverviewClient(
        tokenRollups: [
            .init(
                day: .init(year: 2026, month: 6, day: 20),
                repo: kvasirRepo,
                inputTokens: 1_000,
                outputTokens: 500,
                cacheTokens: 250
            ),
            .init(
                day: .init(year: 2026, month: 6, day: 20),
                repo: noRepo,
                inputTokens: 300,
                outputTokens: 100,
                cacheTokens: 0
            )
        ],
        costRollups: [
            .init(day: .init(year: 2026, month: 6, day: 20), repo: kvasirRepo, costUsdNanos: 1_250_000_000),
            .init(day: .init(year: 2026, month: 6, day: 20), repo: noRepo, costUsdNanos: 75_000_000)
        ],
        toolCallRollups: [
            .init(day: .init(year: 2026, month: 6, day: 20), repo: kvasirRepo, callCount: 4),
            .init(day: .init(year: 2026, month: 6, day: 20), repo: noRepo, callCount: 2)
        ]
    )
    let range = OverviewTimeRange(
        start: Date(timeIntervalSince1970: 1_782_000_000),
        end: Date(timeIntervalSince1970: 1_782_259_200)
    )
    let dashboard = OverviewDashboard(client: client)

    let allReposSnapshot = try await dashboard.load(range: range)
    let scopedSnapshot = try await dashboard.load(range: range, repo: kvasirRepo)

    #expect(client.queries == [
        .init(start: range.start, end: range.end),
        .init(start: range.start, end: range.end, repo: kvasirRepo)
    ])
    #expect(allReposSnapshot.repoBreakdown == [
        .init(repo: kvasirRepo, totals: .init(totalTokens: 1_750, costUsdNanos: 1_250_000_000, toolCalls: 4)),
        .init(repo: noRepo, totals: .init(totalTokens: 400, costUsdNanos: 75_000_000, toolCalls: 2))
    ])
    #expect(scopedSnapshot.selectedRepo == kvasirRepo)
    #expect(scopedSnapshot.totals == .init(totalTokens: 1_750, costUsdNanos: 1_250_000_000, toolCalls: 4))
    #expect(scopedSnapshot.series == [
        .init(day: .init(year: 2026, month: 6, day: 20), totalTokens: 1_750, costUsdNanos: 1_250_000_000, toolCalls: 4)
    ])
    #expect(scopedSnapshot.repoBreakdown == [
        .init(repo: kvasirRepo, totals: .init(totalTokens: 1_750, costUsdNanos: 1_250_000_000, toolCalls: 4))
    ])
}

private final class RecordingOverviewClient: OverviewClient, @unchecked Sendable {
    private let tokenRollups: [OverviewTokenRollup]
    private let costRollups: [OverviewCostRollup]
    private let toolCallRollups: [OverviewToolCallRollup]
    private(set) var queries: [OverviewQuery] = []

    init(
        tokenRollups: [OverviewTokenRollup],
        costRollups: [OverviewCostRollup],
        toolCallRollups: [OverviewToolCallRollup]
    ) {
        self.tokenRollups = tokenRollups
        self.costRollups = costRollups
        self.toolCallRollups = toolCallRollups
    }

    func loadOverviewRollups(query: OverviewQuery) async throws -> OverviewRollups {
        queries.append(query)
        if let repo = query.repo {
            return OverviewRollups(
                tokenRollups: tokenRollups.filter { $0.repo == repo },
                costRollups: costRollups.filter { $0.repo == repo },
                toolCallRollups: toolCallRollups.filter { $0.repo == repo }
            )
        }
        return OverviewRollups(
            tokenRollups: tokenRollups,
            costRollups: costRollups,
            toolCallRollups: toolCallRollups
        )
    }
}
