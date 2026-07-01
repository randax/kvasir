import Foundation

public enum ExplorerDataset: Equatable, Sendable {
    case usageRollups
}

public enum ExplorerMeasure: Equatable, Sendable {
    case totalTokens
    case costUsd
}

public enum ExplorerDimension: Equatable, Sendable {
    case day
    case repo
    case model
    case harness
}

public enum ExplorerVisualization: Equatable, Sendable {
    case table
    case lineChart
}

public enum ExplorerSavedPanel: Equatable, Sendable {
    case usageRollupsOverview
}

public struct ExplorerTimeRange: Equatable, Sendable {
    public var start: Date
    public var end: Date

    public init(start: Date, end: Date) {
        self.start = start
        self.end = end
    }
}

public struct ExplorerQuery: Equatable, Sendable {
    public var dataset: ExplorerDataset
    public var timeRange: ExplorerTimeRange
    public var measures: [ExplorerMeasure]
    public var groupBy: [ExplorerDimension]
    public var filters: [ExplorerFilter]
    public var visualization: ExplorerVisualization
    public var limit: UInt64

    public init(
        dataset: ExplorerDataset,
        timeRange: ExplorerTimeRange,
        measures: [ExplorerMeasure],
        groupBy: [ExplorerDimension],
        filters: [ExplorerFilter],
        visualization: ExplorerVisualization,
        limit: UInt64
    ) {
        self.dataset = dataset
        self.timeRange = timeRange
        self.measures = measures
        self.groupBy = groupBy
        self.filters = filters
        self.visualization = visualization
        self.limit = limit
    }
}

public enum ExplorerFilter: Equatable, Sendable {
    case repo(OverviewRepoBucket)
    case model(OverviewModelName)
    case harness(OverviewHarnessName)
}

public struct ExplorerSavedPanelDefinition: Equatable, Sendable {
    public var panel: ExplorerSavedPanel
    public var dataset: ExplorerDataset
    public var measures: [ExplorerMeasure]
    public var groupBy: [ExplorerDimension]
    public var filters: [ExplorerFilter]
    public var visualization: ExplorerVisualization
    public var limit: UInt64

    public init(
        panel: ExplorerSavedPanel,
        dataset: ExplorerDataset,
        measures: [ExplorerMeasure],
        groupBy: [ExplorerDimension],
        filters: [ExplorerFilter],
        visualization: ExplorerVisualization,
        limit: UInt64
    ) {
        self.panel = panel
        self.dataset = dataset
        self.measures = measures
        self.groupBy = groupBy
        self.filters = filters
        self.visualization = visualization
        self.limit = limit
    }
}

public struct ExplorerResult: Equatable, Sendable {
    public var dataset: ExplorerDataset
    public var visualization: ExplorerVisualization
    public var rows: [ExplorerResultRow]

    public init(
        dataset: ExplorerDataset,
        visualization: ExplorerVisualization,
        rows: [ExplorerResultRow]
    ) {
        self.dataset = dataset
        self.visualization = visualization
        self.rows = rows
    }
}

public struct ExplorerResultRow: Equatable, Sendable {
    public var group: [ExplorerGroupValue]
    public var measures: UsageRollupExplorerMeasures

    public init(group: [ExplorerGroupValue], measures: UsageRollupExplorerMeasures) {
        self.group = group
        self.measures = measures
    }
}

public enum ExplorerGroupValue: Equatable, Sendable {
    case day(OverviewRollupDay)
    case repo(OverviewRepoBucket)
    case model(OverviewModelName)
    case harness(OverviewHarnessName)
}

public struct UsageRollupExplorerMeasures: Equatable, Sendable {
    public var totalTokens: UInt64?
    public var costUsdNanos: UInt64?
    public var costSource: OverviewCostSource?

    public init(
        totalTokens: UInt64? = nil,
        costUsdNanos: UInt64? = nil,
        costSource: OverviewCostSource? = nil
    ) {
        self.totalTokens = totalTokens
        self.costUsdNanos = costUsdNanos
        self.costSource = costSource
    }
}

public struct UsageRollupExplorerPanelSnapshot: Equatable, Sendable {
    public var panel: ExplorerSavedPanelDefinition
    public var query: ExplorerQuery
    public var result: ExplorerResult
    public var table: ExplorerTablePresentation

    public init(
        panel: ExplorerSavedPanelDefinition,
        query: ExplorerQuery,
        result: ExplorerResult,
        table: ExplorerTablePresentation
    ) {
        self.panel = panel
        self.query = query
        self.result = result
        self.table = table
    }
}

public struct ExplorerTablePresentation: Equatable, Sendable {
    public var columns: [ExplorerTableColumn]
    public var rows: [ExplorerTableRowPresentation]

    public init(columns: [ExplorerTableColumn], rows: [ExplorerTableRowPresentation]) {
        self.columns = columns
        self.rows = rows
    }
}

public struct ExplorerTableRowPresentation: Equatable, Sendable {
    public var cells: [ExplorerTableCell]

    public init(cells: [ExplorerTableCell]) {
        self.cells = cells
    }
}

public enum ExplorerTableColumn: Equatable, Sendable {
    case dimension(ExplorerDimension)
    case totalTokens
    case costUsd
    case costSource
}

public enum ExplorerTableCell: Equatable, Sendable {
    case day(OverviewRollupDay)
    case repo(OverviewRepoBucket)
    case model(OverviewModelName)
    case harness(OverviewHarnessName)
    case totalTokens(UInt64)
    case emptyTotalTokens
    case costUsd(UInt64)
    case emptyCostUsd
    case costSource(OverviewCostSource)
    case emptyCostSource
}

public protocol UsageRollupExplorerClient: Sendable {
    func loadUsageRollupExplorerPanel(
        range: OverviewTimeRange,
        filters: [ExplorerFilter],
        savedPanel: ExplorerSavedPanelDefinition?
    ) async throws -> UsageRollupExplorerPanelSnapshot
}

public struct UsageRollupExplorer: Sendable {
    private let client: any UsageRollupExplorerClient

    public init(client: any UsageRollupExplorerClient) {
        self.client = client
    }

    public func load(
        range: OverviewTimeRange,
        filters: [ExplorerFilter],
        savedPanel: ExplorerSavedPanelDefinition?
    ) async throws -> UsageRollupExplorerPanelSnapshot {
        try await client.loadUsageRollupExplorerPanel(
            range: range,
            filters: filters,
            savedPanel: savedPanel
        )
    }
}
