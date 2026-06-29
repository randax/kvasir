import Foundation

public struct ExplorerCatalog: Equatable, Sendable {
    public var datasets: [ExplorerDatasetCatalog]
    public var savedPanels: [ExplorerSavedPanelDefinition]

    public init(datasets: [ExplorerDatasetCatalog], savedPanels: [ExplorerSavedPanelDefinition]) {
        self.datasets = datasets
        self.savedPanels = savedPanels
    }
}

public struct ExplorerDatasetCatalog: Equatable, Sendable {
    public var dataset: ExplorerDataset
    public var measures: [ExplorerMeasure]
    public var dimensions: [ExplorerDimension]
    public var filters: [ExplorerDimension]
    public var visualizations: [ExplorerVisualization]
    public var defaultMeasures: [ExplorerMeasure]
    public var defaultGroupBy: [ExplorerDimension]
    public var defaultVisualization: ExplorerVisualization
    public var defaultLimit: UInt64
    public var maxLimit: UInt64
    public var maxGroupingDepth: UInt8

    public init(
        dataset: ExplorerDataset,
        measures: [ExplorerMeasure],
        dimensions: [ExplorerDimension],
        filters: [ExplorerDimension],
        visualizations: [ExplorerVisualization],
        defaultMeasures: [ExplorerMeasure],
        defaultGroupBy: [ExplorerDimension],
        defaultVisualization: ExplorerVisualization,
        defaultLimit: UInt64,
        maxLimit: UInt64,
        maxGroupingDepth: UInt8
    ) {
        self.dataset = dataset
        self.measures = measures
        self.dimensions = dimensions
        self.filters = filters
        self.visualizations = visualizations
        self.defaultMeasures = defaultMeasures
        self.defaultGroupBy = defaultGroupBy
        self.defaultVisualization = defaultVisualization
        self.defaultLimit = defaultLimit
        self.maxLimit = maxLimit
        self.maxGroupingDepth = maxGroupingDepth
    }
}

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

public struct ExplorerSavedPanelRun: Equatable, Sendable {
    public var panel: ExplorerSavedPanel
    public var timeRange: ExplorerTimeRange
    public var filters: [ExplorerFilter]

    public init(
        panel: ExplorerSavedPanel,
        timeRange: ExplorerTimeRange,
        filters: [ExplorerFilter]
    ) {
        self.panel = panel
        self.timeRange = timeRange
        self.filters = filters
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
    public var panel: UsageRollupExplorerPanelState
    public var query: ExplorerQuery
    public var result: ExplorerResult

    public init(panel: UsageRollupExplorerPanelState, query: ExplorerQuery, result: ExplorerResult) {
        self.panel = panel
        self.query = query
        self.result = result
    }

    public var table: ExplorerTablePresentation {
        ExplorerTablePresentation(
            columns: query.groupBy.map(\.title) + query.measures.flatMap(\.tableColumns),
            rows: result.rows.map { row in
                ExplorerTableRowPresentation(cells: row.cells(for: query.measures))
            }
        )
    }
}

public struct UsageRollupExplorerPanelState: Equatable, Sendable {
    public var savedPanel: ExplorerSavedPanel
    public var title: String
    public var dataset: ExplorerDataset
    public var measures: [ExplorerMeasure]
    public var groupBy: [ExplorerDimension]
    public var filters: [ExplorerFilter]
    public var visualization: ExplorerVisualization
    public var limit: UInt64

    public init(
        savedPanel: ExplorerSavedPanel,
        title: String,
        dataset: ExplorerDataset,
        measures: [ExplorerMeasure],
        groupBy: [ExplorerDimension],
        filters: [ExplorerFilter],
        visualization: ExplorerVisualization,
        limit: UInt64
    ) {
        self.savedPanel = savedPanel
        self.title = title
        self.dataset = dataset
        self.measures = measures
        self.groupBy = groupBy
        self.filters = filters
        self.visualization = visualization
        self.limit = limit
    }

    public func applying(filters: [ExplorerFilter]) -> UsageRollupExplorerPanelState {
        var panel = self
        panel.filters = filters
        return panel
    }

    public func query(range: OverviewTimeRange) -> ExplorerQuery {
        ExplorerQuery(
            dataset: dataset,
            timeRange: ExplorerTimeRange(start: range.start, end: range.end),
            measures: measures,
            groupBy: groupBy,
            filters: filters,
            visualization: visualization,
            limit: limit
        )
    }

    public func savedPanelRun(range: OverviewTimeRange) -> ExplorerSavedPanelRun {
        ExplorerSavedPanelRun(
            panel: savedPanel,
            timeRange: ExplorerTimeRange(start: range.start, end: range.end),
            filters: filters
        )
    }
}

public struct ExplorerTablePresentation: Equatable, Sendable {
    public var columns: [String]
    public var rows: [ExplorerTableRowPresentation]

    public init(columns: [String], rows: [ExplorerTableRowPresentation]) {
        self.columns = columns
        self.rows = rows
    }
}

public struct ExplorerTableRowPresentation: Equatable, Sendable {
    public var cells: [String]

    public init(cells: [String]) {
        self.cells = cells
    }
}

public protocol UsageRollupExplorerClient: Sendable {
    func loadExplorerCatalog() async throws -> ExplorerCatalog
    func loadExplorerSavedPanel(_ panel: ExplorerSavedPanel) async throws -> ExplorerSavedPanelDefinition
    func runExplorerQuery(_ query: ExplorerQuery) async throws -> ExplorerResult
    func runExplorerSavedPanel(_ run: ExplorerSavedPanelRun) async throws -> ExplorerResult
}

public struct UsageRollupExplorer: Sendable {
    private let client: any UsageRollupExplorerClient

    public init(client: any UsageRollupExplorerClient) {
        self.client = client
    }

    public func loadDefaultPanel(
        range: OverviewTimeRange,
        filters: [ExplorerFilter]
    ) async throws -> UsageRollupExplorerPanelSnapshot {
        let catalog = try await client.loadExplorerCatalog()
        guard catalog.savedPanels.contains(where: { $0.panel == .usageRollupsOverview }) else {
            throw UsageRollupExplorerError.panelUnavailable
        }
        let definition = try await client.loadExplorerSavedPanel(.usageRollupsOverview)
        guard definition.panel == .usageRollupsOverview else {
            throw UsageRollupExplorerError.panelUnavailable
        }
        let panel = try panelState(from: definition).applying(filters: filters)
        return try await load(panel: panel, range: range, catalog: catalog)
    }

    public func load(
        panel: UsageRollupExplorerPanelState,
        range: OverviewTimeRange
    ) async throws -> UsageRollupExplorerPanelSnapshot {
        let catalog = try await client.loadExplorerCatalog()
        return try await load(panel: panel, range: range, catalog: catalog)
    }

    private func load(
        panel: UsageRollupExplorerPanelState,
        range: OverviewTimeRange,
        catalog: ExplorerCatalog
    ) async throws -> UsageRollupExplorerPanelSnapshot {
        guard let dataset = catalog.datasets.first(where: { $0.dataset == panel.dataset }) else {
            throw UsageRollupExplorerError.datasetUnavailable
        }
        try validate(panel: panel, against: dataset)
        let query = panel.query(range: range)
        let result = try await client.runExplorerQuery(query)
        return UsageRollupExplorerPanelSnapshot(panel: panel, query: query, result: result)
    }

    private func panelState(from definition: ExplorerSavedPanelDefinition) throws -> UsageRollupExplorerPanelState {
        guard definition.panel == .usageRollupsOverview else {
            throw UsageRollupExplorerError.panelUnavailable
        }
        return UsageRollupExplorerPanelState(
            savedPanel: definition.panel,
            title: definition.panel.title,
            dataset: definition.dataset,
            measures: definition.measures,
            groupBy: definition.groupBy,
            filters: definition.filters,
            visualization: definition.visualization,
            limit: definition.limit
        )
    }

    private func validate(
        panel: UsageRollupExplorerPanelState,
        against dataset: ExplorerDatasetCatalog
    ) throws {
        guard dataset.dataset == panel.dataset,
              dataset.visualizations.contains(panel.visualization),
              panel.visualization == .table,
              !panel.measures.isEmpty,
              panel.measures.allSatisfy(dataset.measures.contains),
              panel.groupBy.allSatisfy(dataset.dimensions.contains),
              panel.groupBy.count <= Int(dataset.maxGroupingDepth),
              panel.filters.map(\.dimension).allSatisfy(dataset.filters.contains),
              panel.limit > 0,
              panel.limit <= dataset.maxLimit
        else {
            throw UsageRollupExplorerError.invalidPanel
        }
    }
}

public enum UsageRollupExplorerError: LocalizedError {
    case datasetUnavailable
    case panelUnavailable
    case invalidPanel

    public var errorDescription: String? {
        switch self {
        case .datasetUnavailable:
            return "Usage rollups explorer dataset is unavailable"
        case .panelUnavailable:
            return "Usage rollups explorer panel is unavailable"
        case .invalidPanel:
            return "Usage rollups explorer panel is invalid"
        }
    }
}

private extension ExplorerSavedPanel {
    var title: String {
        switch self {
        case .usageRollupsOverview:
            return "Usage rollups"
        }
    }
}

private extension ExplorerResultRow {
    func cells(for selectedMeasures: [ExplorerMeasure]) -> [String] {
        group.map(\.displayName) + selectedMeasures.flatMap { measure in
            measure.cells(from: measures)
        }
    }
}

private extension ExplorerFilter {
    var dimension: ExplorerDimension {
        switch self {
        case .repo:
            return .repo
        case .model:
            return .model
        case .harness:
            return .harness
        }
    }
}

private extension ExplorerDimension {
    var title: String {
        switch self {
        case .day:
            return "Day"
        case .repo:
            return "Repo"
        case .model:
            return "Model"
        case .harness:
            return "Harness"
        }
    }
}

private extension ExplorerMeasure {
    var tableColumns: [String] {
        switch self {
        case .totalTokens:
            return ["Tokens"]
        case .costUsd:
            return ["Cost", "Source"]
        }
    }

    func cells(from measures: UsageRollupExplorerMeasures) -> [String] {
        switch self {
        case .totalTokens:
            return [measures.totalTokens.map(groupedDecimal) ?? ""]
        case .costUsd:
            return [
                measures.costUsdNanos.map(formattedUsd) ?? "",
                measures.costSource?.estimateLabel ?? "",
            ]
        }
    }
}

private extension ExplorerGroupValue {
    var displayName: String {
        switch self {
        case .day(let day):
            return String(format: "%04d-%02d-%02d", day.year, day.month, day.day)
        case .repo(let repo):
            return repo.displayName
        case .model(let model):
            return model.displayName()
        case .harness(let harness):
            return harness.displayName()
        }
    }
}

private func groupedDecimal(_ value: UInt64) -> String {
    let digits = Array(String(value).reversed())
    var grouped: [Character] = []
    for (index, digit) in digits.enumerated() {
        if index > 0, index.isMultiple(of: 3) {
            grouped.append(",")
        }
        grouped.append(digit)
    }
    return String(grouped.reversed())
}

private func formattedUsd(_ nanos: UInt64) -> String {
    String(format: "$%.3f", Double(nanos) / 1_000_000_000)
}
