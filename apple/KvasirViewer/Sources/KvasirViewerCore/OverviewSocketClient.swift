public protocol OverviewRollupSource: Sendable {
    func overviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot
}

public struct OverviewSocketClient: OverviewClient, Sendable {
    private let source: any OverviewRollupSource

    public init(source: any OverviewRollupSource) {
        self.source = source
    }

    public func loadOverviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot {
        try await source.overviewSnapshot(query: query)
    }
}
