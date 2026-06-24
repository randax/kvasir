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
    func loadOverviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot
}

public struct OverviewDashboard: Sendable {
    private let client: any OverviewClient

    public init(client: any OverviewClient) {
        self.client = client
    }

    public func load(range: OverviewTimeRange, repo: OverviewRepoBucket? = nil) async throws -> OverviewSnapshot {
        let query = OverviewQuery(start: range.start, end: range.end, repo: repo)
        return try await client.loadOverviewSnapshot(query: query)
    }
}
