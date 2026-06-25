import Foundation

public struct OverviewTimeRange: Equatable, Sendable {
    public var start: Date
    public var end: Date

    public init(start: Date, end: Date) {
        self.start = start
        self.end = end
    }
}

public struct OverviewQuery: Equatable, Sendable {
    public var start: Date
    public var end: Date
    public var repo: OverviewRepoBucket?
    public var model: OverviewModelName?

    public init(start: Date, end: Date, repo: OverviewRepoBucket? = nil, model: OverviewModelName? = nil) {
        self.start = start
        self.end = end
        self.repo = repo
        self.model = model
    }
}

public struct OverviewModelName: Hashable, Comparable, Sendable {
    private let value: String

    public init(_ value: String) {
        self.value = value
    }

    public func displayName() -> String {
        value
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        lhs.value < rhs.value
    }
}

public struct OverviewRepoName: Hashable, Comparable, Sendable {
    public var rawValue: String

    public init(_ rawValue: String) {
        self.rawValue = rawValue
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        lhs.rawValue < rhs.rawValue
    }
}

public struct OverviewRepoPath: Hashable, Comparable, Sendable {
    public var rawValue: String

    public init(_ rawValue: String) {
        self.rawValue = rawValue
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        lhs.rawValue < rhs.rawValue
    }
}

public struct OverviewRepoIdentity: Hashable, Comparable, Sendable {
    public let name: OverviewRepoName?
    public let path: OverviewRepoPath?

    public init?(name: OverviewRepoName?, path: OverviewRepoPath?) {
        guard name != nil || path != nil else {
            return nil
        }
        self.name = name
        self.path = path
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        (lhs.name?.rawValue ?? "", lhs.path?.rawValue ?? "") <
            (rhs.name?.rawValue ?? "", rhs.path?.rawValue ?? "")
    }
}

public enum OverviewRepoBucket: Hashable, Comparable, Sendable {
    case noRepo
    case repo(OverviewRepoIdentity)

    public static func < (lhs: Self, rhs: Self) -> Bool {
        lhs.sortKey < rhs.sortKey
    }

    public var displayName: String {
        switch self {
        case .noRepo:
            return "<no-repo>"
        case .repo(let identity):
            return identity.name?.rawValue ?? identity.path?.rawValue ?? "Unknown repo"
        }
    }

    private var sortKey: String {
        switch self {
        case .noRepo:
            return "\u{10ffff}"
        case .repo(let identity):
            return identity.name?.rawValue ?? identity.path?.rawValue ?? ""
        }
    }
}

public struct OverviewRollupDay: Comparable, Hashable, Sendable {
    public var year: Int
    public var month: Int
    public var day: Int

    public init(year: Int, month: Int, day: Int) {
        self.year = year
        self.month = month
        self.day = day
    }

    public static func < (lhs: Self, rhs: Self) -> Bool {
        (lhs.year, lhs.month, lhs.day) < (rhs.year, rhs.month, rhs.day)
    }
}

public enum OverviewCostSource: Equatable, Sendable {
    case native
    case estimated
    case mixed

    public var estimateLabel: String? {
        switch self {
        case .native:
            return nil
        case .estimated:
            return "Estimated"
        case .mixed:
            return "Partly estimated"
        }
    }

    public var chartMarkerLabel: String? {
        switch self {
        case .native:
            return nil
        case .estimated:
            return "Est."
        case .mixed:
            return "Partly est."
        }
    }

    public var usesEstimatedCost: Bool {
        switch self {
        case .native:
            return false
        case .estimated, .mixed:
            return true
        }
    }
}

public struct OverviewCostDisplay: Equatable, Sendable {
    public static let estimateBadgeSystemImage = "plusminus"

    public var costUsdNanos: UInt64
    public var source: OverviewCostSource?

    public init(costUsdNanos: UInt64, source: OverviewCostSource?) {
        self.costUsdNanos = costUsdNanos
        self.source = source
    }

    public var estimateLabel: String? {
        source?.estimateLabel
    }

    public var chartMarkerLabel: String? {
        source?.chartMarkerLabel
    }

    public var usesEstimatedCost: Bool {
        source?.usesEstimatedCost ?? false
    }
}

public struct OverviewCostDashboardPresentation: Equatable, Sendable {
    public var total: OverviewCostDisplay
    public var series: [OverviewCostDisplay]
    public var repos: [OverviewCostDisplay]
    public var models: [OverviewCostDisplay]

    public init(
        total: OverviewCostDisplay,
        series: [OverviewCostDisplay],
        repos: [OverviewCostDisplay],
        models: [OverviewCostDisplay]
    ) {
        self.total = total
        self.series = series
        self.repos = repos
        self.models = models
    }
}

public struct OverviewTotals: Equatable, Sendable {
    public var totalTokens: UInt64
    public var costUsdNanos: UInt64
    public var costSource: OverviewCostSource?
    public var toolCalls: UInt64

    public init(
        totalTokens: UInt64,
        costUsdNanos: UInt64,
        costSource: OverviewCostSource? = nil,
        toolCalls: UInt64
    ) {
        self.totalTokens = totalTokens
        self.costUsdNanos = costUsdNanos
        self.costSource = costSource
        self.toolCalls = toolCalls
    }

    public var costDisplay: OverviewCostDisplay {
        OverviewCostDisplay(costUsdNanos: costUsdNanos, source: costSource)
    }
}

public struct OverviewSeriesPoint: Equatable, Sendable {
    public var day: OverviewRollupDay
    public var totalTokens: UInt64
    public var costUsdNanos: UInt64
    public var costSource: OverviewCostSource?
    public var toolCalls: UInt64

    public init(
        day: OverviewRollupDay,
        totalTokens: UInt64,
        costUsdNanos: UInt64,
        costSource: OverviewCostSource? = nil,
        toolCalls: UInt64
    ) {
        self.day = day
        self.totalTokens = totalTokens
        self.costUsdNanos = costUsdNanos
        self.costSource = costSource
        self.toolCalls = toolCalls
    }

    public var costDisplay: OverviewCostDisplay {
        OverviewCostDisplay(costUsdNanos: costUsdNanos, source: costSource)
    }
}

public struct OverviewSnapshot: Equatable, Sendable {
    public var totals: OverviewTotals
    public var series: [OverviewSeriesPoint]
    public var repoBreakdown: [OverviewRepoSummary]
    public var modelBreakdown: [OverviewModelSummary]
    public var selectedRepo: OverviewRepoBucket?
    public var selectedModel: OverviewModelName?

    public init(
        totals: OverviewTotals,
        series: [OverviewSeriesPoint],
        repoBreakdown: [OverviewRepoSummary],
        modelBreakdown: [OverviewModelSummary] = [],
        selectedRepo: OverviewRepoBucket?,
        selectedModel: OverviewModelName? = nil
    ) {
        self.totals = totals
        self.series = series
        self.repoBreakdown = repoBreakdown
        self.modelBreakdown = modelBreakdown
        self.selectedRepo = selectedRepo
        self.selectedModel = selectedModel
    }

    public var costDashboardPresentation: OverviewCostDashboardPresentation {
        OverviewCostDashboardPresentation(
            total: totals.costDisplay,
            series: series.map(\.costDisplay),
            repos: repoBreakdown.map(\.totals.costDisplay),
            models: modelBreakdown.map(\.totals.costDisplay)
        )
    }
}

public struct OverviewRepoSummary: Equatable, Sendable {
    public var repo: OverviewRepoBucket
    public var totals: OverviewTotals

    public init(repo: OverviewRepoBucket, totals: OverviewTotals) {
        self.repo = repo
        self.totals = totals
    }
}

public struct OverviewModelSummary: Equatable, Sendable {
    public var model: OverviewModelName
    public var totals: OverviewTotals

    public init(model: OverviewModelName, totals: OverviewTotals) {
        self.model = model
        self.totals = totals
    }
}

public protocol OverviewClient: Sendable {
    func loadOverviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot
}

public struct OverviewDashboard: Sendable {
    private let client: any OverviewClient

    public init(client: any OverviewClient) {
        self.client = client
    }

    public func load(
        range: OverviewTimeRange,
        repo: OverviewRepoBucket? = nil,
        model: OverviewModelName? = nil
    ) async throws -> OverviewSnapshot {
        let query = OverviewQuery(start: range.start, end: range.end, repo: repo, model: model)
        return try await client.loadOverviewSnapshot(query: query)
    }
}
