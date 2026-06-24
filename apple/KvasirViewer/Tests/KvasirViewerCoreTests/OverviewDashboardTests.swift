import Foundation
import Testing

@testable import KvasirViewerCore

@Test
@MainActor
func overviewLoadsSnapshotForSelectedRange() async throws {
    let snapshot = OverviewSnapshot(
        totals: .init(totalTokens: 5_050, costUsdNanos: 3_250_000_000, toolCalls: 10),
        series: [
            .init(day: .init(year: 2026, month: 6, day: 20), totalTokens: 2_150, costUsdNanos: 1_250_000_000, toolCalls: 4),
            .init(day: .init(year: 2026, month: 6, day: 21), totalTokens: 2_900, costUsdNanos: 2_000_000_000, toolCalls: 6)
        ],
        repoBreakdown: [],
        selectedRepo: nil
    )
    let client = RecordingOverviewClient(snapshot: snapshot)
    let range = OverviewTimeRange(
        start: Date(timeIntervalSince1970: 1_782_000_000),
        end: Date(timeIntervalSince1970: 1_782_259_200)
    )
    let dashboard = OverviewDashboard(client: client)

    let loaded = try await dashboard.load(range: range)

    #expect(client.queries == [.init(start: range.start, end: range.end)])
    #expect(loaded == snapshot)
}

@Test
@MainActor
func overviewForwardsScopedRepoQuery() async throws {
    let kvasirRepo = OverviewRepoBucket.repo(
        OverviewRepoIdentity(
            name: OverviewRepoName("kvasir"),
            path: OverviewRepoPath("/repos/kvasir")
        )!
    )
    let snapshot = OverviewSnapshot(
        totals: .init(totalTokens: 1_750, costUsdNanos: 1_250_000_000, toolCalls: 4),
        series: [
            .init(day: .init(year: 2026, month: 6, day: 20), totalTokens: 1_750, costUsdNanos: 1_250_000_000, toolCalls: 4)
        ],
        repoBreakdown: [
            .init(repo: kvasirRepo, totals: .init(totalTokens: 1_750, costUsdNanos: 1_250_000_000, toolCalls: 4))
        ],
        selectedRepo: kvasirRepo
    )
    let client = RecordingOverviewClient(snapshot: snapshot)
    let range = OverviewTimeRange(
        start: Date(timeIntervalSince1970: 1_782_000_000),
        end: Date(timeIntervalSince1970: 1_782_259_200)
    )
    let dashboard = OverviewDashboard(client: client)

    let loaded = try await dashboard.load(range: range, repo: kvasirRepo)

    #expect(client.queries == [
        .init(start: range.start, end: range.end, repo: kvasirRepo)
    ])
    #expect(loaded == snapshot)
}

@Test
@MainActor
func overviewForwardsScopedModelQuery() async throws {
    let selectedModel = OverviewModelName("claude-sonnet-4-20250514")
    let snapshot = OverviewSnapshot(
        totals: .init(totalTokens: 3_300, costUsdNanos: 218_015_000, toolCalls: 0),
        series: [
            .init(day: .init(year: 2026, month: 6, day: 21), totalTokens: 2_850, costUsdNanos: 18_015_000, toolCalls: 0)
        ],
        repoBreakdown: [],
        modelBreakdown: [
            .init(model: selectedModel, totals: .init(totalTokens: 3_300, costUsdNanos: 218_015_000, toolCalls: 0))
        ],
        selectedRepo: nil,
        selectedModel: selectedModel
    )
    let client = RecordingOverviewClient(snapshot: snapshot)
    let range = OverviewTimeRange(
        start: Date(timeIntervalSince1970: 1_782_000_000),
        end: Date(timeIntervalSince1970: 1_782_259_200)
    )
    let dashboard = OverviewDashboard(client: client)

    let loaded = try await dashboard.load(range: range, model: selectedModel)

    #expect(client.queries == [
        .init(start: range.start, end: range.end, model: selectedModel)
    ])
    #expect(loaded == snapshot)
}

@Test
func overviewRepoIdentityRejectsEmptyIdentity() {
    #expect(OverviewRepoIdentity(name: nil, path: nil) == nil)
    #expect(OverviewRepoIdentity(name: OverviewRepoName("kvasir"), path: nil) != nil)
    #expect(OverviewRepoIdentity(name: nil, path: OverviewRepoPath("/repos/kvasir")) != nil)
}

private final class RecordingOverviewClient: OverviewClient, @unchecked Sendable {
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
