import Foundation
import Testing

@testable import KvasirViewerCore

@MainActor
@Test
func usageRollupExplorerLoadsPanelSnapshotThroughClient() async throws {
    let range = OverviewTimeRange(
        start: Date(timeIntervalSince1970: 1_782_000_000),
        end: Date(timeIntervalSince1970: 1_782_259_200)
    )
    let repo = OverviewRepoBucket.repo(OverviewRepoIdentity(
        name: OverviewRepoName("kvasir"),
        path: OverviewRepoPath("/repos/kvasir")
    )!)
    let filters = [ExplorerFilter.repo(repo)]
    let savedPanel = usageRollupExplorerSavedPanel(filters: [.model(OverviewModelName("claude-opus-4-20250514"))])
    let snapshot = usageRollupExplorerSnapshot(
        range: range,
        panel: usageRollupExplorerSavedPanel(filters: filters),
        filters: filters
    )
    let client = RecordingUsageRollupExplorerClient(snapshot: snapshot)
    let explorer = UsageRollupExplorer(client: client)

    let loaded = try await explorer.load(
        range: range,
        filters: filters,
        savedPanel: savedPanel
    )

    #expect(loaded == snapshot)
    #expect(client.requests == [
        RecordedUsageRollupExplorerPanelRequest(
            range: range,
            filters: filters,
            savedPanel: savedPanel
        )
    ])
}

private final class RecordingUsageRollupExplorerClient: UsageRollupExplorerClient, @unchecked Sendable {
    private let snapshot: UsageRollupExplorerPanelSnapshot
    private(set) var requests: [RecordedUsageRollupExplorerPanelRequest] = []

    init(snapshot: UsageRollupExplorerPanelSnapshot) {
        self.snapshot = snapshot
    }

    func loadUsageRollupExplorerPanel(
        range: OverviewTimeRange,
        filters: [ExplorerFilter],
        savedPanel: ExplorerSavedPanelDefinition?
    ) async throws -> UsageRollupExplorerPanelSnapshot {
        requests.append(RecordedUsageRollupExplorerPanelRequest(
            range: range,
            filters: filters,
            savedPanel: savedPanel
        ))
        return snapshot
    }
}

private struct RecordedUsageRollupExplorerPanelRequest: Equatable {
    var range: OverviewTimeRange
    var filters: [ExplorerFilter]
    var savedPanel: ExplorerSavedPanelDefinition?
}

private func usageRollupExplorerSnapshot(
    range: OverviewTimeRange,
    panel: ExplorerSavedPanelDefinition,
    filters: [ExplorerFilter]
) -> UsageRollupExplorerPanelSnapshot {
    UsageRollupExplorerPanelSnapshot(
        panel: panel,
        query: ExplorerQuery(
            dataset: .usageRollups,
            timeRange: ExplorerTimeRange(start: range.start, end: range.end),
            measures: panel.measures,
            groupBy: panel.groupBy,
            filters: filters,
            visualization: .table,
            limit: panel.limit
        ),
        result: ExplorerResult(
            dataset: .usageRollups,
            visualization: .table,
            rows: []
        ),
        table: ExplorerTablePresentation(
            columns: [
                .dimension(.day),
                .dimension(.repo),
                .dimension(.model),
                .totalTokens,
                .costUsd,
                .costSource,
            ],
            rows: []
        )
    )
}

private func usageRollupExplorerSavedPanel(
    filters: [ExplorerFilter] = []
) -> ExplorerSavedPanelDefinition {
    ExplorerSavedPanelDefinition(
        panel: .usageRollupsOverview,
        dataset: .usageRollups,
        measures: [.totalTokens, .costUsd],
        groupBy: [.day, .repo, .model],
        filters: filters,
        visualization: .table,
        limit: 50
    )
}
