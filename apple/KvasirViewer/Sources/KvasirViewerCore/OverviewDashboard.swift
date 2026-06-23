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

    public init(start: Date, end: Date) {
        self.start = start
        self.end = end
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
    public var inputTokens: UInt64
    public var outputTokens: UInt64
    public var cacheTokens: UInt64

    public init(
        day: OverviewRollupDay,
        inputTokens: UInt64,
        outputTokens: UInt64,
        cacheTokens: UInt64
    ) {
        self.day = day
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
    public var costUsdNanos: UInt64

    public init(day: OverviewRollupDay, costUsdNanos: UInt64) {
        self.day = day
        self.costUsdNanos = costUsdNanos
    }
}

public struct OverviewToolCallRollup: Equatable, Sendable {
    public var day: OverviewRollupDay
    public var callCount: UInt64

    public init(day: OverviewRollupDay, callCount: UInt64) {
        self.day = day
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

    public init(totals: OverviewTotals, series: [OverviewSeriesPoint]) {
        self.totals = totals
        self.series = series
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

    public func load(range: OverviewTimeRange) async throws -> OverviewSnapshot {
        let query = OverviewQuery(start: range.start, end: range.end)
        let rollups = try await client.loadOverviewRollups(query: query)
        return Self.snapshot(from: rollups)
    }

    private static func snapshot(from rollups: OverviewRollups) -> OverviewSnapshot {
        var totalTokens: UInt64 = 0
        var totalCostUsdNanos: UInt64 = 0
        var totalToolCalls: UInt64 = 0
        var pointsByDay: [OverviewRollupDay: OverviewSeriesPoint] = [:]

        for rollup in rollups.tokenRollups {
            totalTokens += rollup.totalTokens
            pointsByDay[rollup.day, default: .empty(day: rollup.day)].totalTokens += rollup.totalTokens
        }

        for rollup in rollups.costRollups {
            totalCostUsdNanos += rollup.costUsdNanos
            pointsByDay[rollup.day, default: .empty(day: rollup.day)].costUsdNanos += rollup.costUsdNanos
        }

        for rollup in rollups.toolCallRollups {
            totalToolCalls += rollup.callCount
            pointsByDay[rollup.day, default: .empty(day: rollup.day)].toolCalls += rollup.callCount
        }

        return OverviewSnapshot(
            totals: OverviewTotals(
                totalTokens: totalTokens,
                costUsdNanos: totalCostUsdNanos,
                toolCalls: totalToolCalls
            ),
            series: pointsByDay.values.sorted { $0.day < $1.day }
        )
    }
}

private extension OverviewSeriesPoint {
    static func empty(day: OverviewRollupDay) -> Self {
        Self(day: day, totalTokens: 0, costUsdNanos: 0, toolCalls: 0)
    }
}
