import Foundation
import Testing

@testable import KvasirViewerCore

@MainActor
@Test
func usageRollupExplorerBuildsPanelQueryFromCatalogDefaults() async throws {
    let range = OverviewTimeRange(
        start: Date(timeIntervalSince1970: 1_782_000_000),
        end: Date(timeIntervalSince1970: 1_782_259_200)
    )
    let query = ExplorerQuery(
        dataset: .usageRollups,
        timeRange: ExplorerTimeRange(start: range.start, end: range.end),
        measures: [.totalTokens, .costUsd],
        groupBy: [.day, .repo, .model],
        filters: [],
        visualization: .table,
        limit: 50
    )
    let expectedPanel = UsageRollupExplorerPanelState(
        savedPanel: .usageRollupsOverview,
        title: "Usage rollups",
        dataset: .usageRollups,
        measures: [.totalTokens, .costUsd],
        groupBy: [.day, .repo, .model],
        filters: [],
        visualization: .table,
        limit: 50
    )
    let result = ExplorerResult(
        dataset: .usageRollups,
        visualization: .table,
        rows: [
            ExplorerResultRow(
                group: [
                    .day(OverviewRollupDay(year: 2026, month: 6, day: 20)),
                    .repo(.repo(OverviewRepoIdentity(
                        name: OverviewRepoName("kvasir"),
                        path: OverviewRepoPath("/repos/kvasir")
                    )!)),
                    .model(OverviewModelName("claude-opus-4-20250514")),
                ],
                measures: UsageRollupExplorerMeasures(
                    totalTokens: 1_700,
                    costUsdNanos: 54_150_000,
                    costSource: .estimated
                )
            )
        ]
    )
    let client = RecordingUsageRollupExplorerClient(
        catalog: usageRollupExplorerCatalog(),
        savedPanel: usageRollupExplorerSavedPanel(),
        result: result
    )
    let explorer = UsageRollupExplorer(client: client)

    let panel = try await explorer.loadDefaultPanel(range: range, filters: [])

    #expect(client.loadedPanels == [.usageRollupsOverview])
    #expect(client.queries == [query])
    #expect(client.savedPanelRuns.isEmpty)
    #expect(panel.panel == expectedPanel)
    #expect(panel.query == query)
    #expect(panel.result == result)
    #expect(panel.table.rows == [
        .init(cells: ["2026-06-20", "kvasir", "claude-opus-4-20250514", "1,700", "$0.054", "Estimated"])
    ])
}

@MainActor
@Test
func usageRollupExplorerTableFollowsSelectedMeasureOrder() async throws {
    let range = OverviewTimeRange(
        start: Date(timeIntervalSince1970: 1_782_000_000),
        end: Date(timeIntervalSince1970: 1_782_259_200)
    )
    let repo = OverviewRepoBucket.repo(OverviewRepoIdentity(
        name: OverviewRepoName("kvasir"),
        path: OverviewRepoPath("/repos/kvasir")
    )!)
    let panelState = UsageRollupExplorerPanelState(
        savedPanel: .usageRollupsOverview,
        title: "Usage rollups",
        dataset: .usageRollups,
        measures: [.costUsd, .totalTokens],
        groupBy: [.day, .repo],
        filters: [.repo(repo), .model(OverviewModelName("claude-opus-4-20250514"))],
        visualization: .table,
        limit: 25
    )
    let expectedQuery = ExplorerQuery(
        dataset: .usageRollups,
        timeRange: ExplorerTimeRange(start: range.start, end: range.end),
        measures: [.costUsd, .totalTokens],
        groupBy: [.day, .repo],
        filters: panelState.filters,
        visualization: .table,
        limit: 25
    )
    let result = ExplorerResult(
        dataset: .usageRollups,
        visualization: .table,
        rows: [
            ExplorerResultRow(
                group: [
                    .day(OverviewRollupDay(year: 2026, month: 6, day: 20)),
                    .repo(repo),
                ],
                measures: UsageRollupExplorerMeasures(
                    totalTokens: 1_700,
                    costUsdNanos: 54_150_000,
                    costSource: .estimated
                )
            )
        ]
    )
    let panel = UsageRollupExplorerPanelSnapshot(
        panel: panelState,
        query: expectedQuery,
        result: result
    )

    #expect(panel.panel == panelState)
    #expect(panel.query == expectedQuery)
    #expect(panel.table.columns == ["Day", "Repo", "Cost", "Source", "Tokens"])
    #expect(panel.table.rows == [
        .init(cells: ["2026-06-20", "kvasir", "$0.054", "Estimated", "1,700"])
    ])
}

private final class RecordingUsageRollupExplorerClient: UsageRollupExplorerClient, @unchecked Sendable {
    let catalog: ExplorerCatalog
    let savedPanel: ExplorerSavedPanelDefinition
    let result: ExplorerResult
    private(set) var loadedPanels: [ExplorerSavedPanel] = []
    private(set) var savedPanelRuns: [ExplorerSavedPanelRun] = []
    private(set) var queries: [ExplorerQuery] = []

    init(
        catalog: ExplorerCatalog,
        savedPanel: ExplorerSavedPanelDefinition,
        result: ExplorerResult
    ) {
        self.catalog = catalog
        self.savedPanel = savedPanel
        self.result = result
    }

    func loadExplorerCatalog() async throws -> ExplorerCatalog {
        catalog
    }

    func loadExplorerSavedPanel(_ panel: ExplorerSavedPanel) async throws -> ExplorerSavedPanelDefinition {
        loadedPanels.append(panel)
        return savedPanel
    }

    func runExplorerQuery(_ query: ExplorerQuery) async throws -> ExplorerResult {
        queries.append(query)
        return result
    }

    func runExplorerSavedPanel(_ run: ExplorerSavedPanelRun) async throws -> ExplorerResult {
        savedPanelRuns.append(run)
        return result
    }
}

private func usageRollupExplorerCatalog() -> ExplorerCatalog {
    ExplorerCatalog(
        datasets: [
            ExplorerDatasetCatalog(
                dataset: .usageRollups,
                measures: [.totalTokens, .costUsd],
                dimensions: [.day, .repo, .model],
                filters: [.repo, .model, .harness],
                visualizations: [.table],
                defaultMeasures: [.totalTokens, .costUsd],
                defaultGroupBy: [.day, .repo, .model],
                defaultVisualization: .table,
                defaultLimit: 50,
                maxLimit: 500,
                maxGroupingDepth: 3
            )
        ],
        savedPanels: [usageRollupExplorerSavedPanel()]
    )
}

private func usageRollupExplorerSavedPanel() -> ExplorerSavedPanelDefinition {
    ExplorerSavedPanelDefinition(
        panel: .usageRollupsOverview,
        dataset: .usageRollups,
        measures: [.totalTokens, .costUsd],
        groupBy: [.day, .repo, .model],
        filters: [],
        visualization: .table,
        limit: 50
    )
}
