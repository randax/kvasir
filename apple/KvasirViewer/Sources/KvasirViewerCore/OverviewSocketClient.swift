public protocol OverviewRollupSource: Sendable {
    func overviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot
}

public protocol TraceInspectorSource: Sendable {
    func traceInspectorSnapshot(query: TraceInspectorQuery) async throws -> TraceInspectorSnapshot
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

public struct TraceInspectorSocketClient: TraceInspectorClient, Sendable {
    private let source: any TraceInspectorSource

    public init(source: any TraceInspectorSource) {
        self.source = source
    }

    public func loadTraceInspector(query: TraceInspectorQuery) async throws -> TraceInspectorSnapshot {
        try await source.traceInspectorSnapshot(query: query)
    }
}
