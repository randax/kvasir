public protocol OverviewRollupSource: Sendable {
    func overviewRollups(query: OverviewQuery) async throws -> OverviewRollups
}

public struct OverviewSocketClient: OverviewClient, Sendable {
    private let source: any OverviewRollupSource

    public init(source: any OverviewRollupSource) {
        self.source = source
    }

    public func loadOverviewRollups(query: OverviewQuery) async throws -> OverviewRollups {
        try await source.overviewRollups(query: query)
    }
}
