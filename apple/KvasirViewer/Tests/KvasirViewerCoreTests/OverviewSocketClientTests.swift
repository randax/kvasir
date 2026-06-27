import Foundation
import Testing

@testable import KvasirViewerCore

@MainActor
@Test
func overviewSocketClientLoadsSnapshotForTheSameQuery() async throws {
    let query = OverviewQuery(
        start: Date(timeIntervalSince1970: 1_782_000_000),
        end: Date(timeIntervalSince1970: 1_782_259_200)
    )
    let snapshot = OverviewSnapshot(
        totals: .init(totalTokens: 6, costUsdNanos: 4, toolCalls: 5),
        series: [
            .init(day: .init(year: 2026, month: 6, day: 21), totalTokens: 6, costUsdNanos: 4, toolCalls: 5)
        ],
        repoBreakdown: [],
        selectedRepo: nil
    )
    let source = RecordingOverviewSnapshotSource(snapshot: snapshot)
    let client = OverviewSocketClient(source: source)

    let loaded = try await client.loadOverviewSnapshot(query: query)

    #expect(source.queries == [query])
    #expect(loaded == snapshot)
}

@MainActor
@Test
func overviewSocketClientDefersSourceConnectionUntilLoad() async throws {
    let source = DeferredOverviewSnapshotSource {
        OverviewSnapshot(
            totals: .init(totalTokens: 2, costUsdNanos: 0, toolCalls: 0),
            series: [],
            repoBreakdown: [],
            selectedRepo: nil
        )
    }
    let client = OverviewSocketClient(source: source)

    #expect(source.loadCount == 0)

    _ = try await client.loadOverviewSnapshot(
        query: OverviewQuery(
            start: Date(timeIntervalSince1970: 1_782_000_000),
            end: Date(timeIntervalSince1970: 1_782_259_200)
        )
    )

    #expect(source.loadCount == 1)
}

@MainActor
@Test
func traceInspectorSocketClientLoadsSnapshotForTheSameQuery() async throws {
    let prompt = OverviewPromptRoute(
        session: OverviewSessionRoute(
            harness: OverviewHarnessName("opencode"),
            sessionID: OverviewSessionID("opencode-session-1")
        ),
        promptID: OverviewPromptID("opencode-turn-1")
    )
    let query = TraceInspectorQuery(prompt: prompt)
    let snapshot = TraceInspectorSnapshot(
        prompt: prompt,
        traces: [],
        content: [
            TraceInspectorContentItem(
                occurredAt: Date(timeIntervalSince1970: 1_781_956_801),
                harness: OverviewHarnessName("opencode"),
                kind: .assistantMessage,
                content: TraceInspectorContentText("I need to read it first.")
            )
        ],
        contentAvailability: .captured(
            harness: OverviewHarnessName("opencode"),
            kinds: [.captured(.assistantMessage)]
        )
    )
    let source = RecordingTraceInspectorSnapshotSource(snapshot: snapshot)
    let client = TraceInspectorSocketClient(source: source)

    let loaded = try await client.loadTraceInspector(query: query)

    #expect(source.queries == [query])
    #expect(loaded == snapshot)
}

private final class RecordingOverviewSnapshotSource: OverviewRollupSource, @unchecked Sendable {
    let snapshot: OverviewSnapshot
    private let lock = NSLock()
    private var recordedQueries: [OverviewQuery] = []

    init(snapshot: OverviewSnapshot) {
        self.snapshot = snapshot
    }

    func overviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot {
        lock.withLock {
            recordedQueries.append(query)
        }
        return snapshot
    }

    var queries: [OverviewQuery] {
        lock.withLock { recordedQueries }
    }
}

private final class RecordingTraceInspectorSnapshotSource: TraceInspectorSource, @unchecked Sendable {
    let snapshot: TraceInspectorSnapshot
    private let lock = NSLock()
    private var recordedQueries: [TraceInspectorQuery] = []

    init(snapshot: TraceInspectorSnapshot) {
        self.snapshot = snapshot
    }

    func traceInspectorSnapshot(query: TraceInspectorQuery) async throws -> TraceInspectorSnapshot {
        lock.withLock {
            recordedQueries.append(query)
        }
        return snapshot
    }

    var queries: [TraceInspectorQuery] {
        lock.withLock { recordedQueries }
    }
}

private final class DeferredOverviewSnapshotSource: OverviewRollupSource, @unchecked Sendable {
    private let lock = NSLock()
    private let load: () -> OverviewSnapshot
    private var recordedLoadCount = 0

    init(load: @escaping () -> OverviewSnapshot) {
        self.load = load
    }

    func overviewSnapshot(query: OverviewQuery) async throws -> OverviewSnapshot {
        lock.withLock {
            recordedLoadCount += 1
        }
        return load()
    }

    var loadCount: Int {
        lock.withLock { recordedLoadCount }
    }
}
