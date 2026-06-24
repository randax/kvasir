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

    public init(start: Date, end: Date, repo: OverviewRepoBucket? = nil) {
        self.start = start
        self.end = end
        self.repo = repo
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
    public var name: OverviewRepoName?
    public var path: OverviewRepoPath?

    public init(name: OverviewRepoName?, path: OverviewRepoPath?) {
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

public struct OverviewTokenRollup: Equatable, Sendable {
    public var day: OverviewRollupDay
    public var repo: OverviewRepoBucket
    public var inputTokens: UInt64
    public var outputTokens: UInt64
    public var cacheTokens: UInt64

    public init(
        day: OverviewRollupDay,
        repo: OverviewRepoBucket = .noRepo,
        inputTokens: UInt64,
        outputTokens: UInt64,
        cacheTokens: UInt64
    ) {
        self.day = day
        self.repo = repo
        self.inputTokens = inputTokens
        self.outputTokens = outputTokens
        self.cacheTokens = cacheTokens
    }

    public var totalTokens: UInt64 {
        inputTokens + outputTokens + cacheTokens
    }
}

public struct OverviewCostRollup: Equatable, Sendable {
    public var day: OverviewRollupDay
    public var repo: OverviewRepoBucket
    public var costUsdNanos: UInt64

    public init(day: OverviewRollupDay, repo: OverviewRepoBucket = .noRepo, costUsdNanos: UInt64) {
        self.day = day
        self.repo = repo
        self.costUsdNanos = costUsdNanos
    }
}

public struct OverviewToolCallRollup: Equatable, Sendable {
    public var day: OverviewRollupDay
    public var repo: OverviewRepoBucket
    public var callCount: UInt64

    public init(day: OverviewRollupDay, repo: OverviewRepoBucket = .noRepo, callCount: UInt64) {
        self.day = day
        self.repo = repo
        self.callCount = callCount
    }
}

public struct OverviewRollups: Equatable, Sendable {
    public var tokenRollups: [OverviewTokenRollup]
    public var costRollups: [OverviewCostRollup]
    public var toolCallRollups: [OverviewToolCallRollup]

    public init(
        tokenRollups: [OverviewTokenRollup],
        costRollups: [OverviewCostRollup],
        toolCallRollups: [OverviewToolCallRollup]
    ) {
        self.tokenRollups = tokenRollups
        self.costRollups = costRollups
        self.toolCallRollups = toolCallRollups
    }
}

public struct OverviewTotals: Equatable, Sendable {
    public var totalTokens: UInt64
    public var costUsdNanos: UInt64
    public var toolCalls: UInt64

    public init(totalTokens: UInt64, costUsdNanos: UInt64, toolCalls: UInt64) {
        self.totalTokens = totalTokens
        self.costUsdNanos = costUsdNanos
        self.toolCalls = toolCalls
    }
}

public struct OverviewSeriesPoint: Equatable, Sendable {
    public var day: OverviewRollupDay
    public var totalTokens: UInt64
    public var costUsdNanos: UInt64
    public var toolCalls: UInt64

    public init(
        day: OverviewRollupDay,
        totalTokens: UInt64,
        costUsdNanos: UInt64,
        toolCalls: UInt64
    ) {
        self.day = day
        self.totalTokens = totalTokens
        self.costUsdNanos = costUsdNanos
        self.toolCalls = toolCalls
    }
}

public struct OverviewSnapshot: Equatable, Sendable {
    public var totals: OverviewTotals
    public var series: [OverviewSeriesPoint]
    public var repoBreakdown: [OverviewRepoSummary]
    public var selectedRepo: OverviewRepoBucket?

    public init(
        totals: OverviewTotals,
        series: [OverviewSeriesPoint],
        repoBreakdown: [OverviewRepoSummary],
        selectedRepo: OverviewRepoBucket?
    ) {
        self.totals = totals
        self.series = series
        self.repoBreakdown = repoBreakdown
        self.selectedRepo = selectedRepo
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

public protocol OverviewClient: Sendable {
    func loadOverviewRollups(query: OverviewQuery) async throws -> OverviewRollups
}

public struct OverviewDashboard: Sendable {
    private let client: any OverviewClient

    public init(client: any OverviewClient) {
        self.client = client
    }

    public func load(range: OverviewTimeRange, repo: OverviewRepoBucket? = nil) async throws -> OverviewSnapshot {
        let query = OverviewQuery(start: range.start, end: range.end, repo: repo)
        let rollups = try await client.loadOverviewRollups(query: query)
        return Self.snapshot(from: rollups, selectedRepo: repo)
    }

    private static func snapshot(from rollups: OverviewRollups, selectedRepo: OverviewRepoBucket?) -> OverviewSnapshot {
        var totalTokens: UInt64 = 0
        var totalCostUsdNanos: UInt64 = 0
        var totalToolCalls: UInt64 = 0
        var pointsByDay: [OverviewRollupDay: OverviewSeriesPoint] = [:]
        var totalsByRepo: [OverviewRepoBucket: OverviewTotals] = [:]

        for rollup in rollups.tokenRollups {
            totalTokens += rollup.totalTokens
            pointsByDay[rollup.day, default: .empty(day: rollup.day)].totalTokens += rollup.totalTokens
            totalsByRepo[rollup.repo, default: .zero].totalTokens += rollup.totalTokens
        }

        for rollup in rollups.costRollups {
            totalCostUsdNanos += rollup.costUsdNanos
            pointsByDay[rollup.day, default: .empty(day: rollup.day)].costUsdNanos += rollup.costUsdNanos
            totalsByRepo[rollup.repo, default: .zero].costUsdNanos += rollup.costUsdNanos
        }

        for rollup in rollups.toolCallRollups {
            totalToolCalls += rollup.callCount
            pointsByDay[rollup.day, default: .empty(day: rollup.day)].toolCalls += rollup.callCount
            totalsByRepo[rollup.repo, default: .zero].toolCalls += rollup.callCount
        }

        return OverviewSnapshot(
            totals: OverviewTotals(
                totalTokens: totalTokens,
                costUsdNanos: totalCostUsdNanos,
                toolCalls: totalToolCalls
            ),
            series: pointsByDay.values.sorted { $0.day < $1.day },
            repoBreakdown: repoBreakdown(from: totalsByRepo),
            selectedRepo: selectedRepo
        )
    }

    private static func repoBreakdown(from totalsByRepo: [OverviewRepoBucket: OverviewTotals]) -> [OverviewRepoSummary] {
        totalsByRepo
            .map { OverviewRepoSummary(repo: $0.key, totals: $0.value) }
            .sorted { lhs, rhs in
                if lhs.totals.totalTokens != rhs.totals.totalTokens {
                    return lhs.totals.totalTokens > rhs.totals.totalTokens
                }
                if lhs.totals.costUsdNanos != rhs.totals.costUsdNanos {
                    return lhs.totals.costUsdNanos > rhs.totals.costUsdNanos
                }
                if lhs.totals.toolCalls != rhs.totals.toolCalls {
                    return lhs.totals.toolCalls > rhs.totals.toolCalls
                }
                return lhs.repo < rhs.repo
            }
    }
}

private extension OverviewTotals {
    static var zero: Self {
        Self(totalTokens: 0, costUsdNanos: 0, toolCalls: 0)
    }
}

private extension OverviewSeriesPoint {
    static func empty(day: OverviewRollupDay) -> Self {
        Self(day: day, totalTokens: 0, costUsdNanos: 0, toolCalls: 0)
    }
}
